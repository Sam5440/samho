use crate::budget::{BudgetTracker, Consumption};
use crate::config::RuntimeConfig;
use crate::context::{ContextProjector, DeterministicContextProjector};
use crate::events::{EventLog, EventLogOptions, Redactor, TraceEvent, now_ms};
use crate::mcp::{McpClientHooks, McpRegistry};
use crate::model::{ModelRouter, TaskProfile, TaskRouter};
use crate::object::{InvocationContext, Object, ObjectEnvelope, ObjectKind, ObjectRegistry};
use crate::projects::ProjectStore;
use crate::provider::{
    ModelObject, ModelProvider, ModelResponse, Provider, ProviderError, ProviderErrorKind,
    ProviderStreamEvent, Usage,
};
use crate::session::{JournalBackend, Session, SessionStore, SessionSummary, SqliteJournalBackend};
use crate::skills::SkillRegistry;
use crate::tools::{NetworkPolicy, ToolPolicy, ToolRegistry};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, Semaphore, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRequest {
    pub session_id: Option<String>,
    pub prompt: String,
    #[serde(default)]
    pub profile: TaskProfile,
    #[serde(default)]
    pub skills: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: String,
    pub session_id: String,
    pub trace_id: String,
    pub model_id: String,
    pub output: String,
    pub turns: u32,
    pub usage: Usage,
    pub status: String,
    pub cost_usd: f64,
    pub reserved_consumption: Consumption,
}
/// Ephemeral cumulative, redacted snapshots. Content is not persisted unless log_content is enabled.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskProgress {
    pub task_id: String,
    pub session_id: String,
    pub trace_id: String,
    pub model_id: String,
    pub turn: u32,
    pub attempt: u32,
    pub status: String,
    pub text: String,
    pub reasoning: String,
    pub tool_calls: Vec<Value>,
    pub usage: Usage,
    pub revision: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct QueuedControl {
    pub sequence: u64,
    pub kind: String,
    pub status: String,
}
struct Control {
    sequence: u64,
    prompt: String,
}
#[derive(Default)]
struct ControlState {
    steering: VecDeque<Control>,
    followups: VecDeque<Control>,
    sequence: u64,
    closed: bool,
}
struct ActiveTask {
    cancel: CancellationToken,
    controls: Mutex<ControlState>,
    progress: RwLock<TaskProgress>,
}
/// All extension boundaries can be injected without changing the execution loop.
pub struct RuntimeBuilder {
    config: RuntimeConfig,
    provider: Option<Arc<dyn ModelProvider>>,
    router: Option<Arc<dyn TaskRouter>>,
    context: Option<Arc<dyn ContextProjector>>,
    session_backend: Option<Arc<dyn JournalBackend>>,
    event_backend: Option<Arc<dyn JournalBackend>>,
    tool_policy: Option<Arc<dyn ToolPolicy>>,
    network: Option<Arc<dyn NetworkPolicy>>,
    mcp_hooks: Option<Arc<dyn McpClientHooks>>,
}
impl RuntimeBuilder {
    pub fn new(config: RuntimeConfig) -> Self {
        Self {
            config,
            provider: None,
            router: None,
            context: None,
            session_backend: None,
            event_backend: None,
            tool_policy: None,
            network: None,
            mcp_hooks: None,
        }
    }
    pub fn provider(mut self, value: Arc<dyn ModelProvider>) -> Self {
        self.provider = Some(value);
        self
    }
    pub fn router(mut self, value: Arc<dyn TaskRouter>) -> Self {
        self.router = Some(value);
        self
    }
    pub fn context(mut self, value: Arc<dyn ContextProjector>) -> Self {
        self.context = Some(value);
        self
    }
    pub fn session_backend(mut self, value: Arc<dyn JournalBackend>) -> Self {
        self.session_backend = Some(value);
        self
    }
    pub fn event_backend(mut self, value: Arc<dyn JournalBackend>) -> Self {
        self.event_backend = Some(value);
        self
    }
    pub fn tool_policy(mut self, value: Arc<dyn ToolPolicy>) -> Self {
        self.tool_policy = Some(value);
        self
    }
    pub fn network_policy(mut self, value: Arc<dyn NetworkPolicy>) -> Self {
        self.network = Some(value);
        self
    }
    pub fn mcp_hooks(mut self, value: Arc<dyn McpClientHooks>) -> Self {
        self.mcp_hooks = Some(value);
        self
    }
    pub async fn build(self) -> Result<Arc<AgentRuntime>> {
        AgentRuntime::build(self).await
    }
}
pub struct AgentRuntime {
    pub config: RuntimeConfig,
    pub objects: ObjectRegistry,
    pub events: EventLog,
    pub models: ModelRouter,
    pub skills: SkillRegistry,
    pub mcp: McpRegistry,
    pub tools: ToolRegistry,
    pub sessions_store: SessionStore,
    pub projects: ProjectStore,
    provider: Provider,
    router: Arc<dyn TaskRouter>,
    projector: Arc<dyn ContextProjector>,
    budgets: BudgetTracker,
    network: Arc<dyn NetworkPolicy>,
    active: RwLock<BTreeMap<String, Arc<ActiveTask>>>,
    live: broadcast::Sender<TaskProgress>,
    permits: Arc<Semaphore>,
    plugin_changes: Mutex<()>,
    _store_lock: std::fs::File,
}
impl AgentRuntime {
    pub fn builder(config: RuntimeConfig) -> RuntimeBuilder {
        RuntimeBuilder::new(config)
    }
    pub async fn new(config: RuntimeConfig) -> Result<Arc<Self>> {
        Self::builder(config).build().await
    }
    async fn build(builder: RuntimeBuilder) -> Result<Arc<Self>> {
        let mut config = builder.config;
        config.validate()?;
        config.workspace = std::fs::canonicalize(&config.workspace).context("resolve workspace")?;
        tokio::fs::create_dir_all(&config.data_dir).await?;
        config.data_dir = std::fs::canonicalize(&config.data_dir)?;
        for root in &mut config.skill_roots {
            if root.is_relative() {
                *root = config.workspace.join(&*root);
            }
        }
        let store_lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(config.data_dir.join("runtime.lock"))?;
        fs2::FileExt::try_lock_exclusive(&store_lock)
            .context("data directory is already in use; TUI and Web must connect to the same rpi serve instance")?;
        let redactor = Redactor::from_env();
        let event_options = EventLogOptions {
            rotation_bytes: config.logs.rotation_bytes,
            max_archives: config.logs.max_archives,
        };
        let events = if let Some(backend) = builder.event_backend {
            EventLog::open_with_backend(
                backend,
                EventLogOptions {
                    rotation_bytes: 0,
                    ..event_options
                },
                config.durable_logs,
                redactor.clone(),
            )
            .await?
        } else if config.storage.backend == "sqlite" {
            EventLog::open_with_backend(
                Arc::new(SqliteJournalBackend::open(&config.data_dir.join("events.sqlite")).await?),
                EventLogOptions {
                    rotation_bytes: 0,
                    ..event_options
                },
                config.durable_logs,
                redactor.clone(),
            )
            .await?
        } else {
            EventLog::open_with_options(
                config.data_dir.join("events.jsonl"),
                event_options,
                config.durable_logs,
                redactor.clone(),
            )
            .await?
        };
        let sessions_store = if let Some(backend) = builder.session_backend {
            SessionStore::open_with_backend(backend, redactor, config.durable_logs).await?
        } else if config.storage.backend == "sqlite" {
            SessionStore::open_with_backend(
                Arc::new(
                    SqliteJournalBackend::open(&config.data_dir.join("sessions.sqlite")).await?,
                ),
                redactor,
                config.durable_logs,
            )
            .await?
        } else {
            SessionStore::open(
                &config.data_dir.join("sessions.jsonl"),
                redactor,
                config.durable_logs,
            )
            .await?
        };
        let projects =
            ProjectStore::open(config.data_dir.join("projects.json"), config.durable_logs).await?;
        let network = builder
            .network
            .unwrap_or_else(|| Arc::new(config.network.clone()));
        let skills = SkillRegistry::discover(&config.skill_roots)?;
        let mcp = McpRegistry::connect_with_policy(
            if config.tools.mcp {
                &config.mcp_servers
            } else {
                &[]
            },
            builder.mcp_hooks,
            network.clone(),
        )
        .await?;
        let tools = ToolRegistry::new(&config, skills.clone(), mcp.clone())?;
        tools.set_network_policy(network.clone());
        if let Some(policy) = builder.tool_policy {
            tools.set_policy(policy);
        }
        let models = ModelRouter::new(config.models.clone(), config.routing.clone());
        let router = builder.router.unwrap_or_else(|| Arc::new(models.clone()));
        let projector = builder.context.unwrap_or_else(|| {
            Arc::new(DeterministicContextProjector::new(config.context.clone()))
        });
        let budgets = BudgetTracker::open(
            &config.data_dir.join("budget.jsonl"),
            config.budget.clone(),
            config.durable_logs,
        )
        .await?;
        let provider = match builder.provider {
            Some(backend) => Provider::with_backend(config.request_timeout_ms, backend)?,
            None => Provider::new(config.request_timeout_ms)?,
        };
        let (live, _) = broadcast::channel(256);
        let runtime = Arc::new(Self {
            models,
            router,
            projector,
            budgets,
            provider,
            network,
            live,
            active: RwLock::new(BTreeMap::new()),
            permits: Arc::new(Semaphore::new(config.max_concurrent_tasks)),
            plugin_changes: Mutex::new(()),
            config,
            objects: ObjectRegistry::default(),
            events,
            skills,
            mcp,
            tools,
            sessions_store,
            projects,
            _store_lock: store_lock,
        });
        runtime.tools.bind_objects(runtime.objects.clone());
        runtime
            .objects
            .register(Arc::new(RuntimeObject(Arc::downgrade(&runtime))))?;
        for model in runtime.models.models() {
            runtime.objects.register(Arc::new(ModelObject {
                descriptor: model.clone(),
                provider: runtime.provider.clone(),
            }))?;
        }
        for object in runtime
            .skills
            .objects()
            .into_iter()
            .chain(runtime.tools.objects())
            .chain(runtime.mcp.objects())
        {
            runtime.objects.register(object)?;
        }
        for session in runtime.sessions_store.list().await? {
            runtime.objects.register(Arc::new(SessionObject {
                id: session.id,
                store: runtime.sessions_store.clone(),
            }))?;
        }
        for config in &runtime.config.remote_facets {
            let facet = crate::facet::RemoteFacet::connect(config, runtime.network.clone()).await?;
            runtime.objects.register(Arc::new(facet))?;
        }
        runtime.objects.activate().await?;
        Ok(runtime)
    }
    /// Install typed Rust tool Objects before admitting tasks.
    pub async fn register_tool(&self, object: Arc<dyn Object>) -> Result<()> {
        let _change = self.plugin_changes.lock().await;
        ToolRegistry::validate_object(&*object)?;
        if self.objects.get(&object.envelope().id).is_ok() {
            bail!("duplicate Object");
        }
        let id = object.envelope().id;
        self.objects.register(object.clone())?;
        if let Err(error) = self.objects.activate().await {
            self.objects.remove_inactive(&id).await?;
            return Err(error);
        }
        self.tools.register(object)?;
        Ok(())
    }
    pub async fn run(&self, request: TaskRequest, cancel: CancellationToken) -> Result<TaskResult> {
        self.run_with_task_id(Uuid::new_v4().to_string(), request, cancel)
            .await
    }
    pub fn subscribe_progress(&self) -> broadcast::Receiver<TaskProgress> {
        self.live.subscribe()
    }
    pub async fn progress(&self, id: &str) -> Option<TaskProgress> {
        let active = self.active.read().await.get(id).cloned()?;
        Some(active.progress.read().await.clone())
    }
    pub async fn control(
        &self,
        id: &str,
        prompt: String,
        follow_up: bool,
    ) -> Result<QueuedControl> {
        if prompt.trim().is_empty() || prompt.len() > 1024 * 1024 {
            bail!("control prompt must contain 1..1048576 bytes");
        }
        let active = self
            .active
            .read()
            .await
            .get(id)
            .cloned()
            .context("task is not active")?;
        let mut controls = active.controls.lock().await;
        if controls.closed || active.cancel.is_cancelled() {
            bail!("task no longer accepts controls");
        }
        if controls.steering.len() + controls.followups.len() >= 128 {
            bail!("control queue is full");
        }
        let sequence = controls.sequence + 1;
        let kind = if follow_up { "follow_up" } else { "steering" };
        let p = active.progress.read().await.clone();
        self.emit(
            &p.trace_id,
            &p.session_id,
            id,
            &format!("control:{sequence}"),
            "task.control.queued",
            json!({"sequence":sequence,"kind":kind,"prompt":self.content(json!(prompt))}),
            Some(format!("task:{id}")),
        )
        .await?;
        controls.sequence = sequence;
        let control = Control { sequence, prompt };
        if follow_up {
            controls.followups.push_back(control);
        } else {
            controls.steering.push_back(control);
        }
        Ok(QueuedControl {
            sequence,
            kind: kind.into(),
            status: "queued".into(),
        })
    }
    pub async fn fork(
        &self,
        parent: &str,
        cutoff: usize,
        title: Option<String>,
    ) -> Result<Session> {
        let session = self.sessions_store.fork(parent, cutoff, title).await?;
        let _change = self.plugin_changes.lock().await;
        self.objects.register(Arc::new(SessionObject {
            id: session.id.clone(),
            store: self.sessions_store.clone(),
        }))?;
        self.objects.activate().await?;
        Ok(session)
    }
    async fn publish(&self, active: &ActiveTask, mut progress: TaskProgress) {
        let mut previous = active.progress.write().await;
        progress.revision = previous.revision + 1;
        *previous = progress.clone();
        let _ = self.live.send(progress);
    }
    pub async fn run_with_task_id(
        &self,
        task_id: String,
        request: TaskRequest,
        cancel: CancellationToken,
    ) -> Result<TaskResult> {
        if request.prompt.trim().is_empty() {
            bail!("prompt cannot be empty");
        }
        let _permit = tokio::select! {
            result = self.permits.clone().acquire_owned() => result?,
            _ = cancel.cancelled() => bail!("cancelled while queued"),
        };
        let trace_id = Uuid::new_v4().to_string();
        let session = self
            .sessions_store
            .create_or_get(request.session_id.clone(), self.system_prompt())
            .await?;
        let session_id;
        let mut tasks = self.active.write().await;
        if tasks.contains_key(&task_id) {
            bail!("duplicate active task id");
        }
        {
            let mut guard = session.lock().await;
            if guard.status == "running" {
                bail!("session {} already has an active task", guard.id);
            }
            session_id = guard.id.clone();
            guard.status = "running".into();
            guard.updated_at = now_ms();
        }
        let active = Arc::new(ActiveTask {
            cancel: cancel.clone(),
            controls: Mutex::new(ControlState::default()),
            progress: RwLock::new(TaskProgress {
                task_id: task_id.clone(),
                session_id: session_id.clone(),
                trace_id: trace_id.clone(),
                status: "running".into(),
                ..Default::default()
            }),
        });
        tasks.insert(task_id.clone(), active.clone());
        drop(tasks);
        let mut result = self
            .execute_registered(
                &request,
                &session,
                &session_id,
                &task_id,
                &trace_id,
                cancel.clone(),
            )
            .await;
        let mut guard = session.lock().await;
        let mut controls = active.controls.lock().await;
        controls.closed = true;
        let unapplied = controls.steering.len() + controls.followups.len();
        drop(controls);
        guard.status = match &result {
            Ok(_) => "done",
            Err(_) if cancel.is_cancelled() => "cancelled",
            Err(_) => "failed",
        }
        .into();
        guard.updated_at = now_ms();
        if let Err(error) = self.sessions_store.commit(&guard).await {
            guard.status = "failed".into();
            result =
                Err(error.context("terminal session checkpoint failed; durability is uncertain"));
        }
        let status = guard.status.clone();
        drop(guard);
        let payload = match &result {
            Ok(result) => {
                json!({"status":status,"turns":result.turns,"usage":result.usage,"unapplied_controls":unapplied})
            }
            Err(error) => {
                json!({"status":status,"error":format!("{error:#}"),"provider_error":error.downcast_ref::<ProviderError>(),"unapplied_controls":unapplied})
            }
        };
        let emitted = self
            .emit(
                &trace_id,
                &session_id,
                &task_id,
                "terminal",
                &format!("task.{status}"),
                payload,
                None,
            )
            .await;
        let mut progress = active.progress.read().await.clone();
        progress.status = status;
        self.publish(&active, progress).await;
        self.active.write().await.remove(&task_id);
        self.budgets.finish(&task_id).await;
        let _ = self
            .objects
            .dispose_scope(&format!("task:{task_id}"), Duration::from_secs(5))
            .await;
        emitted?;
        result.map_err(|error| {
            if let Some(provider) = error.downcast_ref::<ProviderError>() {
                let mut provider = provider.clone();
                provider.message = self.events.redactor().text(&provider.message);
                anyhow::Error::new(provider)
            } else {
                anyhow::anyhow!(self.events.redactor().text(&format!("{error:#}")))
            }
        })
    }
    #[allow(clippy::too_many_arguments)]
    async fn execute_registered(
        &self,
        request: &TaskRequest,
        session: &Arc<Mutex<Session>>,
        session_id: &str,
        task_id: &str,
        trace_id: &str,
        cancel: CancellationToken,
    ) -> Result<TaskResult> {
        {
            let _change = self.plugin_changes.lock().await;
            if self.objects.get(&format!("session:{session_id}")).is_err() {
                self.objects.register(Arc::new(SessionObject {
                    id: session_id.into(),
                    store: self.sessions_store.clone(),
                }))?;
            }
            self.objects
                .register_scope(&format!("task:{task_id}"), "root")?;
            let active = self
                .active
                .read()
                .await
                .get(task_id)
                .cloned()
                .context("task control missing")?;
            self.objects.register_in_scope(
                Arc::new(TaskObject {
                    id: task_id.into(),
                    active,
                }),
                &format!("task:{task_id}"),
            )?;
            self.objects.activate().await?;
        }
        let _task_lease = self.objects.acquire(&format!("task:{task_id}"))?;
        let _session_lease = self.objects.acquire(&format!("session:{session_id}"))?;
        self.execute(request, session, session_id, task_id, trace_id, cancel)
            .await
    }
    fn system_prompt(&self) -> String {
        let metadata = self
            .skills
            .list()
            .into_iter()
            .filter(|skill| !skill.disable_model_invocation)
            .map(|skill| format!("{}: {}", skill.name, skill.description))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "{}\n\nAvailable skills (load full instructions with load_skill):\n{metadata}",
            self.config.system_prompt
        )
    }
    async fn checkpoint(
        &self,
        session: &Arc<Mutex<Session>>,
        messages: &[Value],
        model: Option<&str>,
    ) -> Result<()> {
        let mut guard = session.lock().await;
        guard.messages = messages.to_vec();
        guard.updated_at = now_ms();
        if let Some(model) = model {
            guard.last_model = Some(model.into());
        }
        self.sessions_store.commit(&guard).await
    }
    #[allow(clippy::too_many_arguments)]
    async fn execute(
        &self,
        request: &TaskRequest,
        session: &Arc<Mutex<Session>>,
        session_id: &str,
        task_id: &str,
        trace_id: &str,
        cancel: CancellationToken,
    ) -> Result<TaskResult> {
        self.emit(
            trace_id,
            session_id,
            task_id,
            "root",
            "task.start",
            json!({"task_id":task_id,"prompt":self.content(json!(request.prompt))}),
            Some(format!("task:{task_id}")),
        )
        .await?;
        let mut messages = session.lock().await.messages.clone();
        let active = self
            .active
            .read()
            .await
            .get(task_id)
            .cloned()
            .context("missing active task")?;
        let mut active_skills = Vec::new();
        for message in &messages {
            if message["role"] == "system"
                && let Some(content) = message["content"].as_str()
                && let Some(name) = content
                    .strip_prefix("Explicit skill ")
                    .and_then(|s| s.split_once(":\n").map(|(name, _)| name))
                && !active_skills.contains(&name.to_string())
            {
                active_skills.push(name.to_string());
            }
        }
        // A crash may leave an assistant's tool batch unfinished. Close each unresolved call with
        // an error observation. Never replay a possibly committed external effect automatically.
        let mut pending = Vec::new();
        for message in &messages {
            if let Some(calls) = message["tool_calls"].as_array() {
                for call in calls {
                    if let Some(id) = call["id"].as_str() {
                        pending.push(id.to_string());
                    }
                }
            }
            if message["role"] == "tool"
                && let Some(id) = message["tool_call_id"].as_str()
            {
                pending.retain(|pending| pending != id);
            }
        }
        for id in pending {
            messages.push(json!({"role":"tool","tool_call_id":id,"content":"Interrupted previous tool call. Its effect is unknown; reconcile state before repeating it."}));
        }
        messages.push(json!({"role":"user","content":request.prompt}));
        session.lock().await.title = request.prompt.chars().take(80).collect();
        for name in &request.skills {
            let instructions = self.skills.load(name, true).await?;
            if !active_skills.contains(name) {
                active_skills.push(name.clone());
            }
            messages.push(json!({"role":"system","content":format!("Explicit skill {name}:\n{instructions}")}));
            self.emit(
                trace_id,
                session_id,
                task_id,
                &format!("skill:{name}"),
                "skill.load",
                json!({"name":name}),
                Some(format!("skill:{name}")),
            )
            .await?;
        }
        self.checkpoint(session, &messages, None).await?;
        let mut schemas = self.tools.schemas().await;
        let mut profile = self.router.classify(&request.prompt, &request.profile);
        let initial_limit = self
            .config
            .models
            .iter()
            .map(|m| {
                m.context_window.saturating_sub(
                    m.max_output_tokens
                        .max(self.config.routing.output_reserve_tokens),
                )
            })
            .max()
            .unwrap_or(0);
        let initial_projection = self.projector.project(&messages, &schemas, initial_limit)?;
        profile.context_tokens = profile.context_tokens.max(initial_projection.input_tokens);
        let mut decision = self.router.decide(&profile, &[])?;
        self.emit(trace_id, session_id, task_id, "route", "model.route", json!({
            "model_id":decision.model.id,"provider":decision.model.provider,"tier":decision.model.tier,"profile":profile,
            "score":decision.score,"estimated_cost_usd":decision.estimated_cost_usd,"rationale":decision.rationale,"rejected":decision.rejected,
            "classifier":"configurable deterministic rules; tier labels are operator supplied, not intelligence measurements"
        }), Some(format!("model:{}",decision.model.id))).await?;
        let mut usage = Usage::default();
        let mut cost_usd = 0.0;
        for turn in 1..=self.config.max_turns {
            if cancel.is_cancelled() {
                bail!("cancelled");
            }
            self.apply_controls(&active, session, &mut messages, false)
                .await?;
            // Tool advertisement is refreshed at safe turn boundaries; executing a batch never changes it mid-batch.
            schemas = self.tools.schemas().await;
            let start = std::time::Instant::now();
            let mut excluded = Vec::new();
            let mut attempt = 0;
            let response = loop {
                attempt += 1;
                let model = &decision.model;
                let projection = self.projector.project(
                    &messages,
                    &schemas,
                    model.context_window.saturating_sub(model.max_output_tokens),
                )?;
                if let Some(lineage) = &projection.lineage {
                    self.emit(
                        trace_id,
                        session_id,
                        task_id,
                        &format!("context:{turn}:{attempt}"),
                        "context.project",
                        json!({"turn":turn,"attempt":attempt,"lineage":lineage}),
                        None,
                    )
                    .await?;
                }
                if model.provider == crate::model::ProviderKind::ChatCompletions {
                    let url = reqwest::Url::parse(
                        model.base_url.as_deref().context("provider URL missing")?,
                    )?;
                    tokio::select! {_=cancel.cancelled()=>bail!("cancelled before network request"),r=self.network.authorize_url(&url)=>r?}
                }
                let output_reserve = model.budget_output_reserve();
                let reservation_usd = (projection.input_tokens as f64
                    * model.input_usd_per_million
                    + output_reserve as f64 * model.output_usd_per_million)
                    / 1_000_000.0;
                let reservation = self
                    .budgets
                    .reserve(
                        task_id,
                        reservation_usd,
                        u64::from(projection.input_tokens) + u64::from(output_reserve),
                        profile.budget_usd,
                    )
                    .await?;
                self.emit(trace_id,session_id,task_id,&format!("model:{turn}:{attempt}"),"model.start",json!({"turn":turn,"attempt":attempt,"model_id":model.id,"reserved_usd":reservation_usd,"request":self.content(json!({"messages":projection.messages,"tools":schemas}))}),Some(format!("model:{}",model.id))).await?;
                let mut progress = active.progress.read().await.clone();
                progress.model_id = model.id.clone();
                progress.turn = turn;
                progress.attempt = attempt;
                progress.text.clear();
                progress.reasoning.clear();
                progress.tool_calls.clear();
                progress.status = "streaming".into();
                self.publish(&active, progress).await;
                let response = self
                    .model_call(
                        &active,
                        model,
                        &projection.messages,
                        &schemas,
                        cancel.clone(),
                    )
                    .await;
                match response {
                    Ok(response) => {
                        let actual = Consumption {
                            usd: (response.usage.input_tokens as f64 * model.input_usd_per_million
                                + response.usage.output_tokens as f64
                                    * model.output_usd_per_million)
                                / 1_000_000.0,
                            tokens: response
                                .usage
                                .input_tokens
                                .saturating_add(response.usage.output_tokens),
                        };
                        // Missing usage is not treated as a free request.
                        let actual = if actual.tokens == 0 {
                            Consumption {
                                usd: reservation_usd,
                                tokens: u64::from(projection.input_tokens)
                                    + u64::from(model.budget_output_reserve()),
                            }
                        } else {
                            actual
                        };
                        let overage = self.budgets.settle(reservation, actual.clone()).await?;
                        if overage {
                            // 最坏情况估计被打破：真实账目已持久化，发出告警并继续；
                            // 后续派发的 ceiling 检查仍会按累计真实用量拦截。
                            self.emit(
                                trace_id, session_id, task_id,
                                &format!("model:{turn}:{attempt}"),
                                "budget.overage",
                                json!({
                                    "turn":turn,"attempt":attempt,"model_id":model.id,
                                    "reserved_usd":reservation_usd,
                                    "actual":actual,
                                    "note":"usage exceeded worst-case reservation; recorded and continuing; dispatch ceilings still apply"
                                }),
                                Some(format!("model:{}", model.id)),
                            ).await?;
                        }
                        break response;
                    }
                    Err(error) => {
                        self.emit(trace_id,session_id,task_id,&format!("model:{turn}:{attempt}"),"model.error",json!({"turn":turn,"attempt":attempt,"model_id":model.id,"latency_ms":start.elapsed().as_millis() as u64,"error":error,"retryable":error.retryable(),"unknown_consumption_reserved":true}),Some(format!("model:{}",model.id))).await?;
                        if !error.retryable() || attempt >= self.config.retry.max_attempts {
                            return Err(error.into());
                        }
                        let backoff =
                            error
                                .retry_after_ms
                                .unwrap_or(self.config.retry.initial_backoff_ms.saturating_mul(
                                    1u64.checked_shl(attempt - 1).unwrap_or(u64::MAX),
                                ))
                                .min(self.config.retry.max_backoff_ms);
                        if self.config.retry.reroute && profile.model.is_none() {
                            excluded.push(model.id.clone());
                            let mut retry_profile = profile.clone();
                            retry_profile.context_tokens = projection.input_tokens;
                            if let Ok(next) = self.router.decide(&retry_profile, &excluded) {
                                self.emit(trace_id,session_id,task_id,&format!("route:{turn}:{attempt}"),"model.reroute",json!({"from":model.id,"to":next.model.id,"rationale":next.rationale,"rejected":next.rejected,"reason":error.kind}),Some(format!("model:{}",next.model.id))).await?;
                                decision = next;
                            }
                        }
                        self.emit(trace_id,session_id,task_id,&format!("retry:{turn}:{attempt}"),"model.retry",json!({"attempt":attempt,"next_model":decision.model.id,"backoff_ms":backoff}),None).await?;
                        tokio::select! {_=cancel.cancelled()=>return Err(ProviderError::new(ProviderErrorKind::Cancelled,"cancelled during retry backoff").into()),_=tokio::time::sleep(Duration::from_millis(backoff))=>{}}
                    }
                }
            };
            usage.input_tokens += response.usage.input_tokens;
            usage.output_tokens += response.usage.output_tokens;
            cost_usd += (response.usage.input_tokens as f64 * decision.model.input_usd_per_million
                + response.usage.output_tokens as f64 * decision.model.output_usd_per_million)
                / 1_000_000.0;
            if response.message["role"] != "assistant" {
                bail!("provider returned invalid assistant role");
            }
            messages.push(response.message.clone());
            self.checkpoint(session, &messages, Some(&decision.model.id))
                .await?;
            self.emit(trace_id, session_id, task_id, &format!("model:{turn}"), "model.response",
                json!({"turn":turn,"model_id":decision.model.id,"finish_reason":response.finish_reason,"usage":response.usage,
                    "cost_usd":cost_usd,"latency_ms":start.elapsed().as_millis() as u64,"message":self.content(response.message.clone())}),
                Some(format!("model:{}",decision.model.id))).await?;
            let calls = response
                .message
                .get("tool_calls")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if calls.is_empty() {
                if response.finish_reason == "length" {
                    bail!("response truncated at output limit");
                }
                let output = response.message["content"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("assistant response has no text content"))?
                    .to_string();
                if self
                    .apply_controls(&active, session, &mut messages, true)
                    .await?
                {
                    continue;
                }
                return Ok(TaskResult {
                    task_id: task_id.into(),
                    session_id: session_id.into(),
                    trace_id: trace_id.into(),
                    model_id: decision.model.id,
                    output,
                    turns: turn,
                    usage,
                    status: "done".into(),
                    cost_usd,
                    reserved_consumption: self.budgets.finish(task_id).await,
                });
            }
            if response.finish_reason == "length" {
                bail!("tool batch truncated; no tool executed");
            }
            let mut ids = std::collections::BTreeSet::new();
            for call in &calls {
                let id = call["id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("tool call id required"))?;
                if !ids.insert(id) {
                    bail!("duplicate tool call id");
                }
                let name = call["function"]["name"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("tool call name required"))?;
                if !schemas
                    .iter()
                    .any(|schema| schema["function"]["name"] == name)
                {
                    bail!("model requested unadvertised tool {name}");
                }
                serde_json::from_str::<Value>(
                    call["function"]["arguments"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("tool arguments required"))?,
                )
                .context("invalid tool JSON")?;
            }
            // Tool batches stay ordered to preserve read-after-write semantics.
            for call in calls {
                if cancel.is_cancelled() {
                    bail!("cancelled");
                }
                let name = call["function"]["name"].as_str().expect("validated name");
                let id = call["id"].as_str().expect("validated id");
                let args: Value = serde_json::from_str(
                    call["function"]["arguments"]
                        .as_str()
                        .expect("validated args"),
                )?;
                let span_id = format!("{task_id}:tool:{id}");
                self.emit(
                    trace_id,
                    session_id,
                    task_id,
                    &format!("tool:{id}"),
                    "tool.start",
                    json!({"name":name,"call_id":id,"arguments":self.content(args.clone()),"side_effect":if ["read_file","load_skill"].contains(&name){"read_only"}else{"possible_external"},"recovery":"unknown or interrupted effects must be reconciled; no automatic tool replay"}),
                    Some(format!("tool:{name}")),
                )
                .await?;
                let start = std::time::Instant::now();
                let context = InvocationContext {
                    session_id: session_id.into(),
                    trace_id: trace_id.into(),
                    span_id,
                    cancel: cancel.clone(),
                };
                let result = tokio::select! {_=cancel.cancelled()=>bail!("cancelled during tool call; external effect may be unknown"),result=self.tools.call_authorized(name,args,context,&active_skills)=>result};
                if cancel.is_cancelled() {
                    bail!("cancelled during tool call; inspect trace and external state");
                }
                let (content, is_error) = match result {
                    Ok(value) => {
                        let is_error = value["isError"].as_bool().unwrap_or(false)
                            || value["is_error"].as_bool().unwrap_or(false);
                        (value, is_error)
                    }
                    Err(error) => (json!({"error":format!("{error:#}")}), true),
                };
                let text = bounded_text(&content.to_string(), self.config.max_tool_output_bytes);
                messages.push(json!({"role":"tool","tool_call_id":id,"content":text}));
                if name == "load_skill"
                    && !is_error
                    && let Some(skill) = content["name"].as_str()
                {
                    if !active_skills.contains(&skill.to_string()) {
                        active_skills.push(skill.to_string());
                    }
                    let instructions = self.skills.load(skill, true).await?;
                    if !messages.iter().any(|m| {
                        m["role"] == "system"
                            && m["content"].as_str().is_some_and(|s| {
                                s.starts_with(&format!("Explicit skill {skill}:\n"))
                            })
                    }) {
                        messages.insert(1,json!({"role":"system","content":format!("Explicit skill {skill}:\n{instructions}")}));
                    }
                    self.emit(
                        trace_id,
                        session_id,
                        task_id,
                        &format!("skill:{skill}"),
                        "skill.load",
                        json!({"name":skill,"source":"model tool"}),
                        Some(format!("skill:{skill}")),
                    )
                    .await?;
                }
                self.checkpoint(session, &messages, None).await?;
                self.emit(trace_id, session_id, task_id, &format!("tool:{id}"), "tool.result",
                    json!({"name":name,"call_id":id,"is_error":is_error,"latency_ms":start.elapsed().as_millis() as u64,
                        "result":self.content(content)}), Some(format!("tool:{name}"))).await?;
            }
        }
        bail!("maximum agent turns reached; task is incomplete")
    }
    fn content(&self, value: Value) -> Value {
        if self.config.log_content {
            value
        } else {
            json!("[content omitted; enable log_content for full transcript trace]")
        }
    }
    async fn model_call(
        &self,
        active: &ActiveTask,
        model: &crate::model::ModelDescriptor,
        messages: &[Value],
        schemas: &[Value],
        cancel: CancellationToken,
    ) -> crate::provider::ProviderResult<ModelResponse> {
        if !self.config.streaming
            || (!model.streaming && model.provider != crate::model::ProviderKind::Demo)
        {
            let response = self
                .provider
                .complete_typed(model, messages, schemas, cancel)
                .await?;
            let mut p = active.progress.read().await.clone();
            p.text = self
                .events
                .redactor()
                .text(response.message["content"].as_str().unwrap_or(""));
            p.tool_calls = self
                .events
                .redactor()
                .value(response.message["tool_calls"].clone())
                .as_array()
                .cloned()
                .unwrap_or_default();
            p.usage = response.usage.clone();
            p.status = "model_done".into();
            self.publish(active, p).await;
            return Ok(response);
        }
        let (tx, mut rx) = mpsc::channel(self.config.stream_buffer.max(1));
        let provider = self.provider.clone();
        let model = model.clone();
        let messages = messages.to_vec();
        let schemas = schemas.to_vec();
        let child = cancel.clone();
        let call = tokio::spawn(async move {
            provider
                .complete_stream(&model, &messages, &schemas, child, tx)
                .await
        });
        let _abort_on_drop = AbortOnDrop(call.abort_handle());
        let mut full_text = String::new();
        let mut reasoning = String::new();
        let mut calls: BTreeMap<usize, Value> = BTreeMap::new();
        while let Some(event) = tokio::select! {_=cancel.cancelled()=>{call.abort();return Err(ProviderError::new(ProviderErrorKind::Cancelled,"cancelled"));},e=rx.recv()=>e}
        {
            match event {
                ProviderStreamEvent::TextDelta { delta } => full_text.push_str(&delta),
                ProviderStreamEvent::ReasoningDelta { delta } => reasoning.push_str(&delta),
                ProviderStreamEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments,
                } => {
                    let entry=calls.entry(index).or_insert_with(||json!({"id":null,"type":"function","function":{"name":"","arguments":""}}));
                    if let Some(id) = id {
                        entry["id"] = json!(id);
                    }
                    if let Some(name) = name {
                        let existing = entry["function"]["name"].as_str().unwrap_or("").to_string();
                        entry["function"]["name"] = json!(existing + &name);
                    }
                    if let Some(arguments) = arguments {
                        let existing = entry["function"]["arguments"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        entry["function"]["arguments"] = json!(existing + &arguments);
                    }
                }
                ProviderStreamEvent::Usage { usage } => {
                    let mut p = active.progress.read().await.clone();
                    p.usage = usage;
                    self.publish(active, p).await;
                }
                ProviderStreamEvent::Start { .. }
                | ProviderStreamEvent::Finish { .. }
                | ProviderStreamEvent::Done => {}
            }
            let mut p = active.progress.read().await.clone();
            p.text = self.events.redactor().streaming_text(&full_text, false);
            p.reasoning = self.events.redactor().streaming_text(&reasoning, false);
            p.tool_calls = calls
                .values()
                .map(|call| {
                    let mut v = call.clone();
                    v["function"]["arguments"] = json!(self.events.redactor().streaming_text(
                        call["function"]["arguments"].as_str().unwrap_or(""),
                        false
                    ));
                    self.events.redactor().value(v)
                })
                .collect();
            self.publish(active, p).await;
        }
        let response = call.await.map_err(|error| {
            ProviderError::new(
                ProviderErrorKind::Protocol,
                format!("provider worker failed: {error}"),
            )
        })??;
        let mut p = active.progress.read().await.clone();
        p.text = self
            .events
            .redactor()
            .text(response.message["content"].as_str().unwrap_or(""));
        p.reasoning = self.events.redactor().text(&reasoning);
        p.tool_calls = self
            .events
            .redactor()
            .value(response.message["tool_calls"].clone())
            .as_array()
            .cloned()
            .unwrap_or_default();
        p.usage = response.usage.clone();
        p.status = "model_done".into();
        self.publish(active, p).await;
        Ok(response)
    }
    async fn apply_controls(
        &self,
        active: &ActiveTask,
        session: &Arc<Mutex<Session>>,
        messages: &mut Vec<Value>,
        terminal: bool,
    ) -> Result<bool> {
        let mut controls = active.controls.lock().await;
        let mut applied = Vec::new();
        while let Some(control) = controls.steering.pop_front() {
            messages.push(json!({"role":"user","content":control.prompt}));
            applied.push((control.sequence, "steering"));
        }
        if terminal
            && applied.is_empty()
            && let Some(control) = controls.followups.pop_front()
        {
            messages.push(json!({"role":"user","content":control.prompt}));
            applied.push((control.sequence, "follow_up"));
        }
        if terminal && applied.is_empty() {
            controls.closed = true;
        }
        drop(controls);
        if applied.is_empty() {
            return Ok(false);
        }
        self.checkpoint(session, messages, None).await?;
        let p = active.progress.read().await.clone();
        for (sequence, kind) in applied {
            self.emit(
                &p.trace_id,
                &p.session_id,
                &p.task_id,
                &format!("control:{sequence}"),
                "task.control.applied",
                json!({"sequence":sequence,"kind":kind}),
                Some(format!("task:{}", p.task_id)),
            )
            .await?;
        }
        Ok(true)
    }
    // The envelope keeps correlation fields explicit at the persistence boundary.
    #[allow(clippy::too_many_arguments)]
    async fn emit(
        &self,
        trace_id: &str,
        session_id: &str,
        task_id: &str,
        suffix: &str,
        kind: &str,
        payload: Value,
        object: Option<String>,
    ) -> Result<()> {
        let span = if suffix == "root" {
            task_id.into()
        } else {
            format!("{task_id}:{suffix}")
        };
        let mut event = TraceEvent::new(trace_id, session_id, &span, kind, payload);
        if suffix != "root" {
            event.parent_span_id = Some(task_id.into());
        }
        event.object_id = object;
        self.events.append(event).await.map(|_| ())
    }
    pub async fn state(&self) -> Value {
        json!({"service":"rpi","version":env!("CARGO_PKG_VERSION"),
            "simulation":self.config.models.iter().all(|model| model.provider == crate::model::ProviderKind::Demo),
            "workspace":self.config.workspace,"data_dir":self.config.data_dir,"models":self.models.models(),"objects":self.objects.list(),
            "skills":self.skills.list(),"skill_diagnostics":self.skills.diagnostics(),"mcp":self.mcp.status().await,
            "events":self.events.len().await,
            "tiers":self.config.routing.tiers,"ui":self.config.ui,"storage":self.sessions_store.journal_metadata().await,"trace_schema":self.events.schema_metadata().await,"budget":self.budgets.total().await,
            "limits":{"max_turns":self.config.max_turns,"max_concurrent_tasks":self.config.max_concurrent_tasks},
            "log_content":self.config.log_content})
    }
    pub async fn sessions(&self) -> Result<Vec<Session>> {
        self.sessions_store.list().await
    }
    pub async fn session_summaries(&self) -> Vec<SessionSummary> {
        self.sessions_store.list_summaries().await
    }
    pub async fn session(&self, id: &str) -> Result<Option<Session>> {
        self.sessions_store.get(id).await
    }
    pub async fn shutdown(&self) -> Result<()> {
        for task in self.active.read().await.values() {
            task.cancel.cancel();
        }
        self.objects.dispose().await?;
        self.mcp.close().await
    }
}
struct AbortOnDrop(tokio::task::AbortHandle);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct SessionObject {
    id: String,
    store: SessionStore,
}
#[async_trait]
impl Object for SessionObject {
    fn envelope(&self) -> ObjectEnvelope {
        let mut e = ObjectEnvelope::new(
            format!("session:{}", self.id),
            ObjectKind::Session,
            "可持久化会话与分支",
        );
        e.capabilities.insert("inspect".into(), json!({}));
        e
    }
    async fn invoke(
        &self,
        method: &str,
        _input: Value,
        _context: InvocationContext,
    ) -> Result<Value> {
        if method != "inspect" {
            bail!("unknown session capability");
        }
        Ok(json!(self.store.get(&self.id).await?))
    }
}
struct TaskObject {
    id: String,
    active: Arc<ActiveTask>,
}
#[async_trait]
impl Object for TaskObject {
    fn envelope(&self) -> ObjectEnvelope {
        let mut e = ObjectEnvelope::new(
            format!("task:{}", self.id),
            ObjectKind::Task,
            "可取消、可观察的运行任务",
        );
        e.capabilities.insert("cancel".into(), json!({}));
        e.capabilities.insert("inspect".into(), json!({}));
        e
    }
    async fn invoke(
        &self,
        method: &str,
        _input: Value,
        _context: InvocationContext,
    ) -> Result<Value> {
        match method {
            "cancel" => {
                self.active.cancel.cancel();
                Ok(json!({"cancel_requested":true}))
            }
            "inspect" => Ok(json!(*self.active.progress.read().await)),
            _ => bail!("unknown task capability"),
        }
    }
}
fn bounded_text(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.into();
    }
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[tool output truncated]", &text[..end])
}
struct RuntimeObject(Weak<AgentRuntime>);
#[async_trait]
impl Object for RuntimeObject {
    fn envelope(&self) -> ObjectEnvelope {
        let mut envelope = ObjectEnvelope::new(
            "runtime",
            ObjectKind::Agent,
            "Shared Rust agent orchestration runtime",
        );
        envelope.capabilities.insert(
            "run".into(),
            json!({"prompt":"string","session_id":"optional string","profile":"TaskProfile"}),
        );
        envelope
    }
    async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        let runtime = self
            .0
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("runtime disposed"))?;
        match method {
            "run" => Ok(serde_json::to_value(
                runtime
                    .run(serde_json::from_value(input)?, context.cancel)
                    .await?,
            )?),
            "inspect" => Ok(runtime.state().await),
            _ => bail!("unknown runtime capability {method}"),
        }
    }
}
