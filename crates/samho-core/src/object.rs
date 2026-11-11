use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

pub type ObjectId = String;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Agent,
    Model,
    Tool,
    Skill,
    McpServer,
    Session,
    Task,
    Plugin,
    Custom,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectEnvelope {
    pub id: ObjectId,
    pub kind: ObjectKind,
    pub version: u32,
    pub description: String,
    #[serde(default)]
    pub dependencies: Vec<ObjectId>,
    #[serde(default)]
    pub capabilities: BTreeMap<String, Value>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub spec: Value,
}
impl ObjectEnvelope {
    pub fn new(id: impl Into<String>, kind: ObjectKind, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind,
            version: 1,
            description: description.into(),
            dependencies: Vec::new(),
            capabilities: BTreeMap::new(),
            labels: BTreeMap::new(),
            spec: Value::Null,
        }
    }
}
#[derive(Clone)]
pub struct InvocationContext {
    pub session_id: String,
    pub trace_id: String,
    pub span_id: String,
    pub cancel: CancellationToken,
}
#[async_trait]
pub trait Object: Send + Sync {
    fn envelope(&self) -> ObjectEnvelope;
    async fn invoke(&self, method: &str, input: Value, context: InvocationContext)
    -> Result<Value>;
    async fn activate(&self) -> Result<()> {
        Ok(())
    }
    async fn dispose(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectIdentity {
    pub id: ObjectId,
    pub generation: u64,
    pub scope: String,
}
struct Entry {
    identity: ObjectIdentity,
    object: Arc<dyn Object>,
    accepting: AtomicBool,
    active: AtomicBool,
    in_flight: AtomicUsize,
    drained: Notify,
}
impl Entry {
    fn new(object: Arc<dyn Object>, generation: u64, scope: String) -> Self {
        Self {
            identity: ObjectIdentity {
                id: object.envelope().id,
                generation,
                scope,
            },
            object,
            accepting: AtomicBool::new(false),
            active: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            drained: Notify::new(),
        }
    }
}
/// A lease pins one generation; dropping it releases the drain barrier.
pub struct ObjectLease {
    entry: Arc<Entry>,
}
impl ObjectLease {
    pub fn identity(&self) -> &ObjectIdentity {
        &self.entry.identity
    }
    pub fn object(&self) -> &Arc<dyn Object> {
        &self.entry.object
    }
    pub async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        self.entry.object.invoke(method, input, context).await
    }
}
impl Drop for ObjectLease {
    fn drop(&mut self) {
        if self.entry.in_flight.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.entry.drained.notify_waiters();
        }
    }
}
/// Ordered middleware: before hooks may validate/transform; errors stop dispatch.
/// After hooks run in reverse order after either an object result or object error.
#[async_trait]
pub trait InvocationMiddleware: Send + Sync {
    async fn before(
        &self,
        _identity: &ObjectIdentity,
        _method: &str,
        _input: &mut Value,
        _context: &InvocationContext,
    ) -> Result<()> {
        Ok(())
    }
    async fn after(
        &self,
        _identity: &ObjectIdentity,
        _method: &str,
        _result: &Result<Value>,
        _context: &InvocationContext,
    ) -> Result<()> {
        Ok(())
    }
}
#[derive(Clone)]
pub struct ObjectRegistry {
    objects: Arc<RwLock<BTreeMap<ObjectId, Arc<Entry>>>>,
    active: Arc<Mutex<Vec<ObjectId>>>,
    generation: Arc<AtomicU64>,
    scopes: Arc<RwLock<BTreeMap<String, Option<String>>>>,
    middleware: Arc<RwLock<Vec<Arc<dyn InvocationMiddleware>>>>,
}
impl Default for ObjectRegistry {
    fn default() -> Self {
        Self {
            objects: Arc::new(RwLock::new(BTreeMap::new())),
            active: Arc::new(Mutex::new(Vec::new())),
            generation: Arc::new(AtomicU64::new(1)),
            scopes: Arc::new(RwLock::new(BTreeMap::from([("root".into(), None)]))),
            middleware: Arc::new(RwLock::new(Vec::new())),
        }
    }
}
impl ObjectRegistry {
    pub fn register_scope(&self, name: &str, parent: &str) -> Result<()> {
        if name.is_empty() || name == "root" {
            bail!("invalid scope name");
        }
        let mut scopes = self
            .scopes
            .write()
            .map_err(|_| anyhow::anyhow!("scope registry poisoned"))?;
        if !scopes.contains_key(parent) {
            bail!("unknown parent scope {parent}");
        }
        if scopes.contains_key(name) {
            bail!("duplicate scope {name}");
        }
        scopes.insert(name.into(), Some(parent.into()));
        Ok(())
    }
    fn scope_sees(&self, scope: &str, owner: &str) -> bool {
        let scopes = self.scopes.read().expect("scope registry poisoned");
        let mut current = Some(scope);
        while let Some(name) = current {
            if name == owner {
                return true;
            }
            current = scopes.get(name).and_then(|p| p.as_deref());
        }
        false
    }
    pub fn register(&self, object: Arc<dyn Object>) -> Result<()> {
        self.register_in_scope(object, "root")
    }
    pub fn register_in_scope(&self, object: Arc<dyn Object>, scope: &str) -> Result<()> {
        let descriptor = object.envelope();
        if descriptor.id.is_empty() {
            bail!("Object id cannot be empty");
        }
        if !self
            .scopes
            .read()
            .map_err(|_| anyhow::anyhow!("scope registry poisoned"))?
            .contains_key(scope)
        {
            bail!("unknown scope {scope}");
        }
        let mut objects = self
            .objects
            .write()
            .map_err(|_| anyhow::anyhow!("Object registry poisoned"))?;
        if objects.contains_key(&descriptor.id) {
            bail!("duplicate Object {}", descriptor.id);
        }
        let generation = self.generation.fetch_add(1, Ordering::Relaxed);
        objects.insert(
            descriptor.id,
            Arc::new(Entry::new(object, generation, scope.into())),
        );
        Ok(())
    }
    fn entry(&self, id: &str) -> Result<Arc<Entry>> {
        self.objects
            .read()
            .map_err(|_| anyhow::anyhow!("Object registry poisoned"))?
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown Object {id}"))
    }
    /// Trusted compatibility access. Use acquire/invoke_registered for managed calls.
    pub fn get(&self, id: &str) -> Result<Arc<dyn Object>> {
        Ok(self.entry(id)?.object.clone())
    }
    pub fn get_in_scope(&self, id: &str, scope: &str) -> Result<Arc<dyn Object>> {
        let e = self.entry(id)?;
        if !self.scope_sees(scope, &e.identity.scope) {
            bail!("Object is outside scope visibility");
        }
        Ok(e.object.clone())
    }
    pub fn identity(&self, id: &str) -> Result<ObjectIdentity> {
        Ok(self.entry(id)?.identity.clone())
    }
    pub fn list(&self) -> Vec<ObjectEnvelope> {
        self.objects
            .read()
            .expect("Object registry poisoned")
            .values()
            .map(|e| e.object.envelope())
            .collect()
    }
    pub fn list_in_scope(&self, scope: &str) -> Vec<ObjectEnvelope> {
        self.objects
            .read()
            .expect("Object registry poisoned")
            .values()
            .filter(|e| self.scope_sees(scope, &e.identity.scope))
            .map(|e| e.object.envelope())
            .collect()
    }
    pub fn add_middleware(&self, middleware: Arc<dyn InvocationMiddleware>) -> Result<()> {
        self.middleware
            .write()
            .map_err(|_| anyhow::anyhow!("middleware registry poisoned"))?
            .push(middleware);
        Ok(())
    }
    pub fn acquire(&self, id: &str) -> Result<ObjectLease> {
        // Read lock serializes lease admission against publication of replacement.
        let objects = self
            .objects
            .read()
            .map_err(|_| anyhow::anyhow!("Object registry poisoned"))?;
        let entry = objects
            .get(id)
            .context(format!("unknown Object {id}"))?
            .clone();
        if !entry.active.load(Ordering::Acquire) || !entry.accepting.load(Ordering::Acquire) {
            bail!("Object {id} is inactive or draining");
        }
        entry.in_flight.fetch_add(1, Ordering::AcqRel);
        if !entry.accepting.load(Ordering::Acquire) {
            if entry.in_flight.fetch_sub(1, Ordering::AcqRel) == 1 {
                entry.drained.notify_waiters();
            }
            bail!("Object {id} is draining");
        }
        Ok(ObjectLease { entry })
    }
    pub async fn invoke_registered(
        &self,
        id: &str,
        method: &str,
        mut input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        if context.cancel.is_cancelled() {
            bail!("Object invocation cancelled");
        }
        let lease = self.acquire(id)?;
        let middleware = self
            .middleware
            .read()
            .map_err(|_| anyhow::anyhow!("middleware registry poisoned"))?
            .clone();
        let mut applied = Vec::new();
        for hook in &middleware {
            if let Err(error) = hook
                .before(lease.identity(), method, &mut input, &context)
                .await
            {
                let result = Err(error);
                for previous in applied.into_iter().rev() {
                    let previous: &Arc<dyn InvocationMiddleware> = previous;
                    let _ = previous
                        .after(lease.identity(), method, &result, &context)
                        .await;
                }
                return result;
            }
            applied.push(hook);
        }
        let mut result = lease.invoke(method, input, context.clone()).await;
        for hook in applied.into_iter().rev() {
            if let Err(error) = hook
                .after(lease.identity(), method, &result, &context)
                .await
                && result.is_ok()
            {
                result = Err(error);
            }
        }
        result
    }
    pub fn activation_order(&self) -> Result<Vec<ObjectId>> {
        let objects = self
            .objects
            .read()
            .map_err(|_| anyhow::anyhow!("Object registry poisoned"))?;
        let descriptors: BTreeMap<_, _> = objects
            .iter()
            .map(|(id, e)| (id.clone(), e.object.envelope()))
            .collect();
        for (id, e) in objects.iter() {
            for dependency in &descriptors[id].dependencies {
                let owner = objects
                    .get(dependency)
                    .context(format!("missing Object dependency {dependency}"))?;
                if !self.scope_sees(&e.identity.scope, &owner.identity.scope) {
                    bail!("Object dependency escapes scope");
                }
            }
        }
        drop(objects);
        Self::order(&descriptors)
    }
    fn order(descriptors: &BTreeMap<String, ObjectEnvelope>) -> Result<Vec<String>> {
        fn visit(
            id: &str,
            d: &BTreeMap<String, ObjectEnvelope>,
            visiting: &mut BTreeSet<String>,
            visited: &mut BTreeSet<String>,
            out: &mut Vec<String>,
        ) -> Result<()> {
            if visited.contains(id) {
                return Ok(());
            }
            if !visiting.insert(id.into()) {
                bail!("cyclic Object dependency at {id}");
            }
            for dep in &d
                .get(id)
                .context(format!("missing Object dependency {id}"))?
                .dependencies
            {
                visit(dep, d, visiting, visited, out)?;
            }
            visiting.remove(id);
            visited.insert(id.into());
            out.push(id.into());
            Ok(())
        }
        let mut visiting = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut out = Vec::new();
        for id in descriptors.keys() {
            visit(id, descriptors, &mut visiting, &mut visited, &mut out)?;
        }
        Ok(out)
    }
    pub async fn activate(&self) -> Result<()> {
        let mut active = self.active.lock().await;
        let order = self.activation_order()?;
        let base = active.len();
        for id in order {
            if active.contains(&id) {
                continue;
            }
            let entry = self.entry(&id)?;
            if let Err(error) = entry.object.activate().await {
                // Preserve the legacy contract: failed activate must clean its own partial resources.
                for previous in active[base..].iter().rev() {
                    let e = self.entry(previous)?;
                    e.accepting.store(false, Ordering::Release);
                    e.active.store(false, Ordering::Release);
                    let _ = e.object.dispose().await;
                }
                active.truncate(base);
                return Err(error.context(format!("activate Object {id}")));
            }
            entry.active.store(true, Ordering::Release);
            active.push(id);
        }
        // Publish lease admission only after every activation in the transaction succeeded.
        for id in &active[base..] {
            self.entry(id)?.accepting.store(true, Ordering::Release);
        }
        Ok(())
    }
    async fn drain(entry: &Entry, timeout: Duration) -> Result<()> {
        tokio::time::timeout(timeout, async {
            loop {
                let notified = entry.drained.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if entry.in_flight.load(Ordering::Acquire) == 0 {
                    break;
                }
                notified.await;
            }
        })
        .await
        .context("Object generation drain timed out")
    }
    pub async fn hot_replace(
        &self,
        id: &str,
        object: Arc<dyn Object>,
        timeout: Duration,
    ) -> Result<ObjectIdentity> {
        let _transaction = self.active.lock().await;
        let old = self.entry(id)?;
        if !old.active.load(Ordering::Acquire) {
            bail!("cannot hot replace inactive Object");
        }
        let descriptor = object.envelope();
        let before = old.object.envelope();
        if descriptor.id != id || descriptor.kind != before.kind {
            bail!("replacement identity or kind mismatch");
        }
        if descriptor.dependencies != before.dependencies {
            bail!("replacement must preserve dependency edges");
        }
        // Active dependents may hold untracked native references; replace a whole dependent scope first.
        if self
            .list()
            .iter()
            .any(|e| e.id != id && e.dependencies.iter().any(|d| d == id))
        {
            bail!("cannot replace Object with registered dependents");
        }
        let candidate = Arc::new(Entry::new(
            object,
            self.generation.fetch_add(1, Ordering::Relaxed),
            old.identity.scope.clone(),
        ));
        if let Err(error) = candidate.object.activate().await {
            let _ = candidate.object.dispose().await;
            return Err(error.context("activate replacement"));
        }
        candidate.active.store(true, Ordering::Release);
        old.accepting.store(false, Ordering::Release);
        if let Err(error) = Self::drain(&old, timeout).await {
            old.accepting.store(true, Ordering::Release);
            candidate.active.store(false, Ordering::Release);
            let _ = candidate.object.dispose().await;
            return Err(error);
        }
        {
            let mut objects = self
                .objects
                .write()
                .map_err(|_| anyhow::anyhow!("Object registry poisoned"))?;
            if objects
                .get(id)
                .is_none_or(|e| e.identity.generation != old.identity.generation)
            {
                old.accepting.store(true, Ordering::Release);
                bail!("Object generation changed during replacement");
            }
            candidate.accepting.store(true, Ordering::Release);
            objects.insert(id.into(), candidate.clone());
        }
        old.active.store(false, Ordering::Release);
        old.object
            .dispose()
            .await
            .context("new generation published; old generation disposal failed")?;
        Ok(candidate.identity.clone())
    }
    pub async fn remove_inactive(&self, id: &str) -> Result<()> {
        let active = self.active.lock().await;
        if active.iter().any(|a| a == id) {
            bail!("cannot remove an active Object");
        }
        if self
            .list()
            .iter()
            .any(|e| e.dependencies.iter().any(|d| d == id))
        {
            bail!("Object has dependents");
        }
        self.objects
            .write()
            .map_err(|_| anyhow::anyhow!("registry poisoned"))?
            .remove(id);
        Ok(())
    }
    pub async fn dispose_scope(&self, scope: &str, timeout: Duration) -> Result<()> {
        if scope == "root" {
            return self.dispose_with_timeout(timeout).await;
        }
        let mut active = self.active.lock().await;
        let entries: Vec<_> = active
            .iter()
            .rev()
            .filter_map(|id| self.entry(id).ok())
            .filter(|e| self.scope_sees(&e.identity.scope, scope))
            .collect();
        for e in &entries {
            e.accepting.store(false, Ordering::Release);
        }
        for e in &entries {
            if let Err(error) = Self::drain(e, timeout).await {
                for restore in &entries {
                    restore.accepting.store(true, Ordering::Release);
                }
                return Err(error);
            }
        }
        let mut first = None;
        for e in &entries {
            if let Err(error) = e.object.dispose().await {
                first.get_or_insert(error);
            }
            e.active.store(false, Ordering::Release);
        }
        let ids: BTreeSet<_> = entries.iter().map(|e| e.identity.id.clone()).collect();
        active.retain(|id| !ids.contains(id));
        self.objects
            .write()
            .map_err(|_| anyhow::anyhow!("registry poisoned"))?
            .retain(|id, _| !ids.contains(id));
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    pub async fn dispose(&self) -> Result<()> {
        self.dispose_with_timeout(Duration::from_secs(30)).await
    }
    pub async fn dispose_with_timeout(&self, timeout: Duration) -> Result<()> {
        let mut active = self.active.lock().await;
        let entries: Vec<_> = active
            .iter()
            .rev()
            .map(|id| self.entry(id))
            .collect::<Result<_>>()?;
        for e in &entries {
            e.accepting.store(false, Ordering::Release);
        }
        for e in &entries {
            if let Err(error) = Self::drain(e, timeout).await {
                for restore in &entries {
                    restore.accepting.store(true, Ordering::Release);
                }
                return Err(error);
            }
        }
        let mut first = None;
        for e in entries {
            if let Err(error) = e.object.dispose().await {
                first.get_or_insert(error);
            }
            e.active.store(false, Ordering::Release);
        }
        active.clear();
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
pub struct DescriptorObject(pub ObjectEnvelope);
#[async_trait]
impl Object for DescriptorObject {
    fn envelope(&self) -> ObjectEnvelope {
        self.0.clone()
    }
    async fn invoke(
        &self,
        method: &str,
        _input: Value,
        _context: InvocationContext,
    ) -> Result<Value> {
        match method {
            "inspect" => Ok(serde_json::to_value(&self.0)?),
            _ => bail!("Object {} has no method {method}", self.0.id),
        }
    }
}
