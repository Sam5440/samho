use crate::config::RuntimeConfig;
use crate::mcp::McpRegistry;
use crate::object::{InvocationContext, Object, ObjectEnvelope, ObjectKind, ObjectRegistry};
use crate::skills::SkillRegistry;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tokio::io::AsyncReadExt;

#[derive(Clone)]
pub struct ToolRegistry {
    config: RuntimeConfig,
    mcp: McpRegistry,
    objects: Arc<RwLock<Vec<Arc<dyn Object>>>>,
    skills: SkillRegistry,
    policy: Arc<RwLock<Arc<dyn ToolPolicy>>>,
    lifecycle: Arc<RwLock<Option<ObjectRegistry>>>,
}
#[derive(Clone, Debug)]
pub struct ToolAuthorization {
    pub name: String,
    pub input: Value,
    pub workspace: PathBuf,
    pub session_id: String,
    pub active_skills: Vec<String>,
}
/// Implementations may deny calls but never bypass configured tools or Skill restrictions.
#[async_trait]
pub trait ToolPolicy: Send + Sync {
    async fn authorize(&self, request: &ToolAuthorization) -> Result<()>;
}
struct AllowConfiguredTools;
#[async_trait]
impl ToolPolicy for AllowConfiguredTools {
    async fn authorize(&self, _request: &ToolAuthorization) -> Result<()> {
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkPolicyConfig {
    pub allowed_hosts: Vec<String>,
    pub denied_hosts: Vec<String>,
    pub allow_loopback: bool,
    pub allow_private: bool,
    pub allow_http_loopback: bool,
}
impl Default for NetworkPolicyConfig {
    fn default() -> Self {
        Self {
            allowed_hosts: vec![],
            denied_hosts: vec![],
            allow_loopback: true,
            allow_private: false,
            allow_http_loopback: true,
        }
    }
}
#[async_trait]
pub trait NetworkPolicy: Send + Sync {
    async fn authorize_url(&self, url: &reqwest::Url) -> Result<()>;
}
#[async_trait]
impl NetworkPolicy for NetworkPolicyConfig {
    async fn authorize_url(&self, url: &reqwest::Url) -> Result<()> {
        let host = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("network URL has no host"))?;
        if !url.username().is_empty() || url.password().is_some() {
            bail!("credentials are forbidden in network URLs");
        }
        if self
            .denied_hosts
            .iter()
            .any(|entry| entry.eq_ignore_ascii_case(host))
            || (!self.allowed_hosts.is_empty()
                && !self
                    .allowed_hosts
                    .iter()
                    .any(|entry| entry.eq_ignore_ascii_case(host)))
        {
            bail!("network host denied: {host}");
        }
        let dns_host = host.trim_start_matches('[').trim_end_matches(']');
        let addresses = tokio::net::lookup_host((
            dns_host,
            url.port_or_known_default()
                .ok_or_else(|| anyhow::anyhow!("network URL has unknown port"))?,
        ))
        .await?
        .map(|address| address.ip())
        .collect::<Vec<_>>();
        if addresses.is_empty() {
            bail!("network host resolved to no addresses");
        }
        for ip in addresses {
            let ip = match ip {
                std::net::IpAddr::V6(ip) => ip
                    .to_ipv4_mapped()
                    .map(std::net::IpAddr::V4)
                    .unwrap_or(std::net::IpAddr::V6(ip)),
                other => other,
            };
            let loopback = ip.is_loopback();
            let private = match ip {
                std::net::IpAddr::V4(ip) => {
                    ip.is_private()
                        || ip.is_link_local()
                        || ip.is_unspecified()
                        || ip.is_multicast()
                        || ip.is_broadcast()
                }
                std::net::IpAddr::V6(ip) => {
                    ip.is_unique_local()
                        || ip.is_unicast_link_local()
                        || ip.is_unspecified()
                        || ip.is_multicast()
                }
            };
            if loopback && !self.allow_loopback || !loopback && private && !self.allow_private {
                bail!("network address denied: {ip}");
            }
            if url.scheme() != "https"
                && !(url.scheme() == "http" && loopback && self.allow_http_loopback)
            {
                bail!(
                    "network URL requires HTTPS (HTTP permitted only for enabled loopback fixtures)"
                );
            }
        }
        Ok(())
    }
}
pub(crate) fn validate_schema(schema: &Value, value: &Value, label: &str) -> Result<()> {
    fn reject_external_refs(value: &Value) -> Result<()> {
        match value {
            Value::Object(map) => {
                if let Some(reference) = map.get("$ref").and_then(Value::as_str)
                    && !reference.starts_with('#')
                {
                    bail!("external JSON Schema references are disabled");
                }
                for value in map.values() {
                    reject_external_refs(value)?;
                }
            }
            Value::Array(values) => {
                for value in values {
                    reject_external_refs(value)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    reject_external_refs(schema)?;
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| anyhow::anyhow!("invalid {label} schema: {error}"))?;
    if let Some(error) = validator.iter_errors(value).next() {
        bail!(
            "{label} schema validation failed at {}: {error}",
            error.instance_path
        );
    }
    Ok(())
}
impl ToolRegistry {
    pub fn new(config: &RuntimeConfig, skills: SkillRegistry, mcp: McpRegistry) -> Result<Self> {
        let mut objects: Vec<Arc<dyn Object>> = Vec::new();
        if config.tools.read {
            objects.push(Arc::new(ReadFileTool {
                workspace: config.workspace.clone(),
                max_bytes: config.max_tool_output_bytes,
            }));
        }
        if config.tools.write {
            objects.push(Arc::new(WriteFileTool {
                workspace: config.workspace.clone(),
            }));
        }
        objects.push(Arc::new(LoadSkillTool {
            skills: skills.clone(),
        }));
        if config.tools.shell {
            objects.push(Arc::new(ShellTool {
                workspace: config.workspace.clone(),
                max_bytes: config.max_tool_output_bytes,
                timeout_ms: config.tools.shell_timeout_ms,
            }));
        }
        Ok(Self {
            config: config.clone(),
            mcp,
            objects: Arc::new(RwLock::new(objects)),
            skills,
            policy: Arc::new(RwLock::new(Arc::new(AllowConfiguredTools))),
            lifecycle: Arc::new(RwLock::new(None)),
        })
    }
    pub fn objects(&self) -> Vec<Arc<dyn Object>> {
        self.objects.read().expect("tool registry poisoned").clone()
    }
    pub async fn schemas(&self) -> Vec<Value> {
        let mut schemas = self.objects().iter().map(|object| {
            let descriptor = object.envelope();
            json!({"type":"function","function":{"name":descriptor.id.strip_prefix("tool:").unwrap_or(&descriptor.id),"description":descriptor.description,"parameters":descriptor.spec}})
        }).collect::<Vec<_>>();
        if self.config.tools.mcp {
            schemas.extend(self.mcp.tools().await);
        }
        schemas
    }
    pub async fn call(
        &self,
        name: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        self.call_authorized(name, input, context, &[]).await
    }
    pub fn bind_objects(&self, registry: ObjectRegistry) {
        *self.lifecycle.write().expect("tool registry poisoned") = Some(registry);
    }
    pub fn set_policy(&self, policy: Arc<dyn ToolPolicy>) {
        *self.policy.write().expect("tool registry poisoned") = policy;
    }
    pub async fn call_authorized(
        &self,
        name: &str,
        input: Value,
        context: InvocationContext,
        active_skills: &[String],
    ) -> Result<Value> {
        if context.cancel.is_cancelled() {
            bail!("cancelled");
        }
        self.skills.enforce_allowed_tools(active_skills, name)?;
        let policy = self
            .policy
            .read()
            .map_err(|_| anyhow::anyhow!("tool policy poisoned"))?
            .clone();
        policy
            .authorize(&ToolAuthorization {
                name: name.into(),
                input: input.clone(),
                workspace: self.config.workspace.clone(),
                session_id: context.session_id.clone(),
                active_skills: active_skills.to_vec(),
            })
            .await?;
        if let Some(object) = self.objects().into_iter().find(|object| {
            object.envelope().id == name || object.envelope().id == format!("tool:{name}")
        }) {
            let lifecycle = self
                .lifecycle
                .read()
                .map_err(|_| anyhow::anyhow!("tool lifecycle poisoned"))?
                .clone();
            let descriptor = match &lifecycle {
                Some(registry) => registry.get(&object.envelope().id)?.envelope(),
                None => object.envelope(),
            };
            validate_schema(&descriptor.spec, &input, "tool input")?;
            let result = match lifecycle {
                Some(registry) => {
                    registry
                        .invoke_registered(&descriptor.id, "call", input, context)
                        .await?
                }
                None => object.invoke("call", input, context).await?,
            };
            if let Some(schema) = descriptor.capabilities.get("output_schema") {
                validate_schema(schema, &result, "tool output")?;
            }
            return Ok(result);
        }
        if name.starts_with("mcp__") && self.config.tools.mcp {
            return self.mcp.call(name, input, context.cancel).await;
        }
        bail!("unknown tool {name}")
    }
    pub fn validate_object(object: &dyn Object) -> Result<()> {
        let descriptor = object.envelope();
        let name = descriptor
            .id
            .strip_prefix("tool:")
            .ok_or_else(|| anyhow::anyhow!("custom tool id must be tool:<name>"))?;
        if descriptor.kind != ObjectKind::Tool
            || name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            || name.starts_with("mcp__")
        {
            bail!("invalid custom tool identity");
        }
        if !descriptor.spec.is_object() {
            bail!("custom tool requires a JSON Schema object in spec");
        }
        let _ = jsonschema::validator_for(&descriptor.spec)
            .map_err(|error| anyhow::anyhow!("invalid tool input schema: {error}"))?;
        Ok(())
    }
    pub fn register(&self, object: Arc<dyn Object>) -> Result<()> {
        Self::validate_object(&*object)?;
        let id = object.envelope().id;
        let mut objects = self
            .objects
            .write()
            .map_err(|_| anyhow::anyhow!("tool registry poisoned"))?;
        if objects.iter().any(|existing| existing.envelope().id == id) {
            bail!("duplicate tool {id}");
        }
        objects.push(object);
        Ok(())
    }
}
fn within_workspace(workspace: &Path, requested: &str) -> Result<PathBuf> {
    let root = std::fs::canonicalize(workspace)
        .with_context(|| format!("workspace {}", workspace.display()))?;
    let relative = Path::new(requested);
    if relative.is_absolute() {
        bail!("absolute paths are not allowed");
    }
    if relative
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        bail!("parent traversal is not allowed");
    }
    let candidate = root.join(relative);
    if candidate.exists() {
        let canonical = std::fs::canonicalize(&candidate)?;
        if !canonical.starts_with(&root) {
            bail!("path escapes workspace");
        }
        if std::fs::symlink_metadata(&candidate)?
            .file_type()
            .is_symlink()
        {
            bail!("symlink paths are not allowed");
        }
        Ok(canonical)
    } else {
        let parent = candidate
            .parent()
            .ok_or_else(|| anyhow::anyhow!("invalid path"))?;
        let canonical_parent = std::fs::canonicalize(parent)?;
        if !canonical_parent.starts_with(&root) {
            bail!("parent escapes workspace");
        }
        Ok(candidate)
    }
}
struct ReadFileTool {
    workspace: PathBuf,
    max_bytes: usize,
}
#[async_trait]
impl Object for ReadFileTool {
    fn envelope(&self) -> ObjectEnvelope {
        let mut envelope = ObjectEnvelope::new(
            "tool:read_file",
            ObjectKind::Tool,
            "Read a UTF-8 file under the configured workspace",
        );
        envelope.spec = json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false});
        envelope
    }
    async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        if method != "call" {
            bail!("unknown read_file method {method}");
        }
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("path is required"))?;
        let path = within_workspace(&self.workspace, path)?;
        let mut bytes = Vec::new();
        tokio::fs::File::open(&path)
            .await?
            .take((self.max_bytes + 1) as u64)
            .read_to_end(&mut bytes)
            .await?;
        if context.cancel.is_cancelled() {
            bail!("cancelled");
        }
        if bytes.len() > self.max_bytes {
            bail!("file exceeds configured output limit");
        }
        Ok(
            json!({"path":path.strip_prefix(&self.workspace).unwrap_or(&path).display().to_string(),"content":String::from_utf8(bytes).context("file is not UTF-8")?}),
        )
    }
}
struct WriteFileTool {
    workspace: PathBuf,
}
#[async_trait]
impl Object for WriteFileTool {
    fn envelope(&self) -> ObjectEnvelope {
        let mut envelope = ObjectEnvelope::new(
            "tool:write_file",
            ObjectKind::Tool,
            "Write a UTF-8 file under the configured workspace",
        );
        envelope.spec = json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false});
        envelope
    }
    async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        if method != "call" {
            bail!("unknown write_file method {method}");
        }
        if context.cancel.is_cancelled() {
            bail!("cancelled");
        }
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("path is required"))?;
        let content = input
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("content is required"))?;
        let path = within_workspace(&self.workspace, path)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, content).await?;
        Ok(
            json!({"path":path.strip_prefix(&self.workspace).unwrap_or(&path).display().to_string(),"bytes":content.len()}),
        )
    }
}
struct LoadSkillTool {
    skills: SkillRegistry,
}
#[async_trait]
impl Object for LoadSkillTool {
    fn envelope(&self) -> ObjectEnvelope {
        let mut envelope = ObjectEnvelope::new(
            "tool:load_skill",
            ObjectKind::Tool,
            "Load full instructions for a discovered skill",
        );
        envelope.spec = json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false});
        envelope
    }
    async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        if method != "call" {
            bail!("unknown load_skill method {method}");
        }
        let name = input
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("name is required"))?;
        if context.cancel.is_cancelled() {
            bail!("cancelled");
        }
        let instructions = self.skills.load(name, false).await?;
        let skill = self
            .skills
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("skill disappeared during load"))?;
        Ok(
            json!({"name":name,"instructions":instructions,"allowed_tools":skill.allowed_tools,"content_sha256":skill.content_sha256,"metadata":skill.metadata}),
        )
    }
}
struct ShellTool {
    workspace: PathBuf,
    max_bytes: usize,
    timeout_ms: u64,
}
async fn capture(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    max_bytes: usize,
) -> Result<String> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        let size = reader.read(&mut buffer).await?;
        if size == 0 {
            break;
        }
        let retained = size.min(max_bytes.saturating_sub(output.len()));
        output.extend_from_slice(&buffer[..retained]);
        truncated |= retained < size;
    }
    let mut text = String::from_utf8_lossy(&output).into_owned();
    if truncated {
        text.push_str("\n[output truncated]");
    }
    Ok(text)
}
#[async_trait]
impl Object for ShellTool {
    fn envelope(&self) -> ObjectEnvelope {
        let mut envelope = ObjectEnvelope::new(
            "tool:shell",
            ObjectKind::Tool,
            "Run an explicitly enabled shell command in the workspace",
        );
        envelope.spec = json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"],"additionalProperties":false});
        envelope
    }
    async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        if method != "call" {
            bail!("unknown shell method {method}");
        }
        if context.cancel.is_cancelled() {
            bail!("cancelled");
        }
        let script = input["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("command is required"))?;
        let mut command = if cfg!(windows) {
            let mut command = tokio::process::Command::new("powershell.exe");
            command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
            command
        } else {
            let mut command = tokio::process::Command::new("sh");
            command.args(["-c", script]);
            command
        };
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        command
            .current_dir(&self.workspace)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("no shell stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("no shell stderr"))?;
        let limit = self.max_bytes;
        let stdout = tokio::spawn(async move { capture(stdout, limit).await });
        let stderr = tokio::spawn(async move { capture(stderr, limit).await });
        let status = tokio::select! {
            status = child.wait() => status?,
            _ = context.cancel.cancelled() => { let _ = child.kill().await; stdout.abort(); stderr.abort(); bail!("cancelled"); },
            _ = tokio::time::sleep(std::time::Duration::from_millis(self.timeout_ms)) => {
                let _ = child.kill().await; stdout.abort(); stderr.abort(); bail!("shell timeout");
            }
        };
        let out = stdout.await??;
        let err = stderr.await??;
        Ok(
            json!({"exit_code":status.code(),"stdout":out,"stderr":err,"is_error":!status.success()}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;
    #[tokio::test]
    async fn rejects_traversal() {
        let config = RuntimeConfig {
            workspace: std::env::temp_dir(),
            ..RuntimeConfig::default()
        };
        let registry =
            ToolRegistry::new(&config, SkillRegistry::default(), McpRegistry::default()).unwrap();
        let error = registry
            .call(
                "tool:read_file",
                json!({"path":"../secret"}),
                InvocationContext {
                    session_id: "s".into(),
                    trace_id: "t".into(),
                    span_id: "p".into(),
                    cancel: CancellationToken::new(),
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("traversal"));
    }
}
