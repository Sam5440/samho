use crate::config::RuntimeConfig;
use crate::mcp::McpRegistry;
use crate::object::{InvocationContext, Object, ObjectEnvelope, ObjectKind, ObjectRegistry};
use crate::skills::SkillRegistry;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct ToolRegistry {
    config: RuntimeConfig,
    mcp: McpRegistry,
    objects: Arc<RwLock<Vec<Arc<dyn Object>>>>,
    skills: SkillRegistry,
    policy: Arc<RwLock<Arc<dyn ToolPolicy>>>,
    lifecycle: Arc<RwLock<Option<ObjectRegistry>>>,
    network: Arc<RwLock<Arc<dyn NetworkPolicy>>>,
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
    /// 允许以明文 HTTP 访问的主机白名单（如仅提供 HTTP 的模型网关）。
    pub allow_http_hosts: Vec<String>,
}
impl Default for NetworkPolicyConfig {
    fn default() -> Self {
        Self {
            allowed_hosts: vec![],
            denied_hosts: vec![],
            allow_loopback: true,
            allow_private: false,
            allow_http_loopback: true,
            allow_http_hosts: vec![],
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
                && !(url.scheme() == "http"
                    && self
                        .allow_http_hosts
                        .iter()
                        .any(|entry| entry.eq_ignore_ascii_case(host)))
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
        let network: Arc<RwLock<Arc<dyn NetworkPolicy>>> =
            Arc::new(RwLock::new(Arc::new(config.network.clone())));
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
        if config.tools.edit {
            objects.push(Arc::new(EditFileTool {
                workspace: config.workspace.clone(),
                max_file_bytes: EDIT_MAX_FILE_BYTES,
            }));
        }
        if config.tools.list_files {
            objects.push(Arc::new(ListFilesTool {
                workspace: config.workspace.clone(),
            }));
        }
        if config.tools.glob {
            objects.push(Arc::new(GlobTool {
                workspace: config.workspace.clone(),
            }));
        }
        if config.tools.grep {
            objects.push(Arc::new(GrepTool {
                workspace: config.workspace.clone(),
            }));
        }
        if config.tools.webfetch {
            objects.push(Arc::new(WebFetchTool {
                network: network.clone(),
                client: reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(std::time::Duration::from_secs(10))
                    .build()?,
                max_bytes_default: WEBFETCH_MAX_BYTES_DEFAULT,
            }));
        }
        if config.tools.todo {
            objects.push(Arc::new(TodoWriteTool {
                data_dir: config.data_dir.clone(),
            }));
            objects.push(Arc::new(TodoReadTool {
                data_dir: config.data_dir.clone(),
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
            network: Arc::new(RwLock::new(Arc::new(config.network.clone()))),
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
    /// 运行时装配时注入与 Agent 循环同款的网络策略实例，保证
    /// webfetch 与模型请求共用同一套 URL 准入规则。
    pub fn set_network_policy(&self, policy: Arc<dyn NetworkPolicy>) {
        *self.network.write().expect("tool registry poisoned") = policy;
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
        // 目标尚不存在（含多级新目录，如 write_file 新建 `pkg/sub/file.py`）：
        // 沿父链找到最近的已存在祖先做 canonicalize 复核；中间缺失的目录由调用方创建。
        let mut ancestor = candidate
            .parent()
            .ok_or_else(|| anyhow::anyhow!("invalid path"))?;
        while !ancestor.exists() {
            ancestor = ancestor
                .parent()
                .ok_or_else(|| anyhow::anyhow!("invalid path"))?;
        }
        let canonical_parent = std::fs::canonicalize(ancestor)?;
        if !canonical_parent.starts_with(&root) {
            bail!("parent escapes workspace");
        }
        // 悬空符号链接 exists()==false 会漏到本分支，但写入仍会跟随链接；
        // 目标本身是符号链接时一律拒绝。
        if let Ok(meta) = std::fs::symlink_metadata(&candidate)
            && meta.file_type().is_symlink()
        {
            bail!("symlink paths are not allowed");
        }
        Ok(candidate)
    }
}
/// 文本解码：UTF-8（去 BOM）；带 BOM 的 UTF-16LE/BE（Windows PowerShell 5.1 的 `>`
/// 重定向与 Out-File 默认产物）也按文本读取，其余非 UTF-8 内容报错。
fn decode_text(bytes: Vec<u8>) -> Result<String> {
    let utf16 = |big_endian: bool| {
        let units = bytes[2..].chunks_exact(2).map(|pair| {
            if big_endian {
                u16::from_be_bytes([pair[0], pair[1]])
            } else {
                u16::from_le_bytes([pair[0], pair[1]])
            }
        });
        char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .context("file is not valid UTF-16")
    };
    match bytes.as_slice() {
        [0xEF, 0xBB, 0xBF, ..] => {
            String::from_utf8(bytes[3..].to_vec()).context("file is not UTF-8")
        }
        [0xFF, 0xFE, ..] => utf16(false),
        [0xFE, 0xFF, ..] => utf16(true),
        _ => String::from_utf8(bytes).context("file is not UTF-8"),
    }
}
/// 工具结果中的 workspace 相对路径（`/` 分隔）。目标路径来自 `within_workspace`，
/// 可能已 canonicalize（Windows 上带 `\\?\` 前缀），因此先以规范化根剥离。
fn workspace_relative(workspace: &Path, path: &Path) -> String {
    let canonical_root = std::fs::canonicalize(workspace).ok();
    let relative = canonical_root
        .as_deref()
        .and_then(|root| path.strip_prefix(root).ok())
        .or_else(|| path.strip_prefix(workspace).ok())
        .unwrap_or(path);
    relative.to_string_lossy().replace('\\', "/")
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
            "Read a UTF-8 file under the configured workspace, optionally a 1-based line window",
        );
        envelope.spec = json!({"type":"object","properties":{
            "path":{"type":"string"},
            "offset":{"type":"integer","minimum":1,"description":"First line to return (1-based, default from the start)"},
            "limit":{"type":"integer","minimum":1,"maximum":2000,"description":"Maximum lines to return (default all)"}
        },"required":["path"],"additionalProperties":false});
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
        let content = decode_text(bytes)?;
        let rel = workspace_relative(&self.workspace, &path);
        let total_lines = content.lines().count();
        let offset = input.get("offset").and_then(Value::as_u64).unwrap_or(1);
        let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(0);
        if (offset == 1 && limit == 0) || total_lines == 0 {
            return Ok(
                json!({"path":rel,"content":content,"total_lines":total_lines,
                "start_line":1,"end_line":total_lines,"truncated_lines":false}),
            );
        }
        let start = offset as usize;
        let selected: Vec<&str> = content
            .lines()
            .skip(start.saturating_sub(1))
            .take(if limit == 0 {
                usize::MAX
            } else {
                limit as usize
            })
            .collect();
        let selected_count = selected.len();
        let end_line = if selected_count == 0 {
            start.saturating_sub(1)
        } else {
            start + selected_count - 1
        };
        Ok(
            json!({"path":rel,"content":selected.join("\n"),"total_lines":total_lines,
            "start_line":start,"end_line":end_line,"truncated_lines":end_line < total_lines}),
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
        Ok(json!({"path":workspace_relative(&self.workspace, &path),"bytes":content.len()}))
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
/// Windows shell 包装：UTF-8 输出 + 文件类 cmdlet 默认 UTF-8（5.1 默认 ANSI/UTF-16，会把
/// UTF-8 源码读写成乱码）+ 在当前作用域执行用户脚本 + 保留最后语句的成功状态。
/// 用户脚本中的 `exit N` 直接结束进程；原生命令失败沿用其 $LASTEXITCODE。
const WINDOWS_SHELL_WRAPPER: &str = "[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false); \
$OutputEncoding = [Console]::OutputEncoding; \
foreach ($c in 'Get-Content','Set-Content','Add-Content','Out-File','Select-String','Import-Csv','Export-Csv') \
{ $PSDefaultParameterValues[\"${c}:Encoding\"] = 'utf8' }; $global:LASTEXITCODE = 0; \
. ([ScriptBlock]::Create($env:SAMHO_SHELL_COMMAND + \"`n\" + '$global:SamhoShellOk = $?')); \
if ($LASTEXITCODE) { exit $LASTEXITCODE } elseif (-not $global:SamhoShellOk) { exit 1 }";
/// 告知模型实际 shell 方言，避免在 Windows PowerShell 5.1 中使用 `&&`/`||` 等 bash 语法。
fn shell_description() -> &'static str {
    if cfg!(windows) {
        "Run a command in the workspace root using Windows PowerShell 5.1 (each call is a fresh process; \
cwd resets to the workspace root). PowerShell syntax only: chain with `;` (no `&&`/`||`), \
use `if ($LASTEXITCODE -ne 0) { ... }` for conditionals, `New-Item -ItemType Directory -Force` instead of `mkdir -p`, \
`Get-ChildItem -Force` instead of `ls -a`, `Select-Object -Last N` instead of `tail`. \
Prefer the file tools for creating/editing files; parent directories are created automatically."
    } else {
        "Run a POSIX `sh -c` command in the workspace root (each call is a fresh process; cwd resets to the workspace root)"
    }
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
        let mut envelope = ObjectEnvelope::new("tool:shell", ObjectKind::Tool, shell_description());
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
            // 脚本经环境变量传入 ScriptBlock：先把控制台输出切到 UTF-8（否则中文系统按
            // GBK 输出，解码后模型只能看到乱码，包括解析错误），再在当前作用域执行，
            // 末尾捕获最后一条语句的 $? 以保持 `-Command` 的退出码语义。
            let mut command = tokio::process::Command::new("powershell.exe");
            command
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    WINDOWS_SHELL_WRAPPER,
                ])
                .env("SAMHO_SHELL_COMMAND", script);
            if std::env::var_os("PYTHONUTF8").is_none() {
                command.env("PYTHONUTF8", "1");
            }
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
/// 内置工具的限额常量：编辑文件大小、网页抓取字节数/重定向/超时与 todo 列表容量。
const EDIT_MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const WEBFETCH_MAX_BYTES_DEFAULT: usize = 128 * 1024;
const WEBFETCH_MAX_BYTES_CAP: usize = 1024 * 1024;
const WEBFETCH_MAX_REDIRECTS: usize = 5;
const WEBFETCH_TIMEOUT_SECS: u64 = 30;
const TODO_MAX_ITEMS: usize = 200;
const TODO_TASK_CHARS: usize = 500;
const TODO_NOTE_CHARS: usize = 1000;

struct EditFileTool {
    workspace: PathBuf,
    max_file_bytes: usize,
}
#[async_trait]
impl Object for EditFileTool {
    fn envelope(&self) -> ObjectEnvelope {
        let mut envelope = ObjectEnvelope::new(
            "tool:edit_file",
            ObjectKind::Tool,
            "Replace an exact string in an existing UTF-8 workspace file (fails when the match is ambiguous unless replace_all is set)",
        );
        envelope.spec = json!({"type":"object","properties":{
            "path":{"type":"string"},
            "old_string":{"type":"string","minLength":1,"description":"Exact text to find; must match unless replace_all"},
            "new_string":{"type":"string","description":"Replacement text; must differ from old_string"},
            "replace_all":{"type":"boolean","description":"Replace every occurrence (default false, single match required)"}
        },"required":["path","old_string","new_string"],"additionalProperties":false});
        envelope
    }
    async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        if method != "call" {
            bail!("unknown edit_file method {method}");
        }
        if context.cancel.is_cancelled() {
            bail!("cancelled");
        }
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("path is required"))?;
        let old_string = input
            .get("old_string")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("old_string is required"))?;
        let new_string = input
            .get("new_string")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("new_string is required"))?;
        if old_string == new_string {
            bail!("new_string must differ from old_string");
        }
        let replace_all = input
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // 展示层始终使用调用方提供的相对路径，避免 Windows 规范化路径泄漏前缀。
        let rel = input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .strip_prefix("./")
            .unwrap_or_default()
            .to_string();
        let path = within_workspace(&self.workspace, path)?;
        if !path.is_file() {
            bail!("file not found: {rel} (edit_file does not create files; use write_file)");
        }
        let metadata = tokio::fs::metadata(&path).await?;
        if metadata.len() as usize > self.max_file_bytes {
            bail!("file exceeds edit_file size limit");
        }
        let bytes = tokio::fs::read(&path).await?;
        let text = String::from_utf8(bytes).context("file is not UTF-8")?;
        let occurrences = text.matches(old_string).count();
        if occurrences == 0 {
            bail!("old_string not found in {rel}");
        }
        if occurrences > 1 && !replace_all {
            bail!(
                "old_string matches {occurrences} locations in {rel}; add surrounding context or pass replace_all=true"
            );
        }
        let replacements = if replace_all { occurrences } else { 1 };
        let updated = if replace_all {
            text.replace(old_string, new_string)
        } else {
            text.replacen(old_string, new_string, 1)
        };
        tokio::fs::write(&path, &updated).await?;
        Ok(json!({"path":rel,"replacements":replacements,
            "bytes_before":text.len(),"bytes_after":updated.len()}))
    }
}
fn compile_glob_segments(pattern: &str) -> Result<Vec<String>> {
    let mut segments = Vec::new();
    for segment in pattern.split('/') {
        let segment = segment.trim();
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            bail!("parent traversal is not allowed in glob patterns");
        }
        segments.push(segment.to_string());
    }
    if segments.is_empty() {
        bail!("glob pattern must match at least one path segment");
    }
    Ok(segments)
}
struct GlobTool {
    workspace: PathBuf,
}
struct GlobWalk {
    matches: Vec<String>,
    remaining: usize,
    truncated: bool,
    skipped_symlinks: usize,
    skipped_unreadable: usize,
    cancelled: bool,
}
impl GlobWalk {
    fn push_match(&mut self, rel: &[String]) {
        if self.remaining == 0 {
            self.truncated = true;
            return;
        }
        self.remaining -= 1;
        self.matches.push(rel.join("/"));
    }
    fn collect_all_files(&mut self, dir: &Path, rel: &mut Vec<String>, cancel: &CancellationToken) {
        if self.cancelled || cancel.is_cancelled() {
            if cancel.is_cancelled() {
                self.cancelled = true;
            }
            return;
        }
        if rel.len() > WALK_MAX_DEPTH as usize {
            return;
        }
        let children = match sorted_children(dir) {
            Ok(children) => children,
            Err(_) => {
                self.skipped_unreadable += 1;
                return;
            }
        };
        for child in children {
            if self.cancelled || cancel.is_cancelled() || self.remaining == 0 && self.truncated {
                if cancel.is_cancelled() {
                    self.cancelled = true;
                }
                break;
            }
            self.visit_child(child, rel, cancel);
        }
    }
    fn visit_child(
        &mut self,
        child: std::fs::DirEntry,
        rel: &mut Vec<String>,
        cancel: &CancellationToken,
    ) {
        let Ok(file_type) = child.file_type() else {
            return;
        };
        if file_type.is_symlink() {
            self.skipped_symlinks += 1;
            return;
        }
        let name = child.file_name().to_string_lossy().into_owned();
        let is_dir = file_type.is_dir();
        if is_dir {
            if SKIP_DIRS.contains(&name.as_str()) {
                return;
            }
            rel.push(name);
            self.collect_all_files(&child.path(), rel, cancel);
            rel.pop();
        } else {
            rel.push(name);
            self.push_match(rel);
            rel.pop();
        }
    }
    fn run(
        &mut self,
        dir: &Path,
        rel: &mut Vec<String>,
        segments: &[String],
        cancel: &CancellationToken,
    ) {
        if self.cancelled || cancel.is_cancelled() {
            if cancel.is_cancelled() {
                self.cancelled = true;
            }
            return;
        }
        if segments.is_empty() {
            return;
        }
        if segments.len() == 1 && segments[0] == "**" {
            self.collect_all_files(dir, rel, cancel);
            return;
        }
        if rel.len() > WALK_MAX_DEPTH as usize {
            return;
        }
        let children = match sorted_children(dir) {
            Ok(children) => children,
            Err(_) => {
                self.skipped_unreadable += 1;
                return;
            }
        };
        let head = segments[0].as_str();
        if head == "**" {
            // `**` 匹配零层或多层目录：先在本层尝试剩余段，再下钻子目录保持同段。
            self.run(dir, rel, &segments[1..], cancel);
            for child in children {
                if self.cancelled || cancel.is_cancelled() {
                    if cancel.is_cancelled() {
                        self.cancelled = true;
                    }
                    break;
                }
                let Ok(file_type) = child.file_type() else {
                    continue;
                };
                if !file_type.is_dir() || file_type.is_symlink() {
                    continue;
                }
                let name = child.file_name().to_string_lossy().into_owned();
                if SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                rel.push(name);
                self.run(&child.path(), rel, segments, cancel);
                rel.pop();
            }
            return;
        }
        for child in children {
            if self.cancelled || cancel.is_cancelled() {
                if cancel.is_cancelled() {
                    self.cancelled = true;
                }
                break;
            }
            let Ok(file_type) = child.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                self.skipped_symlinks += 1;
                continue;
            }
            let name = child.file_name().to_string_lossy().into_owned();
            let is_dir = file_type.is_dir();
            if is_dir && SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            if !glob_match(head, &name) {
                continue;
            }
            if segments.len() == 1 {
                if !is_dir {
                    rel.push(name);
                    self.push_match(rel);
                    rel.pop();
                }
            } else if is_dir {
                rel.push(name);
                self.run(&child.path(), rel, &segments[1..], cancel);
                rel.pop();
            }
        }
    }
}
#[async_trait]
impl Object for GlobTool {
    fn envelope(&self) -> ObjectEnvelope {
        let mut envelope = ObjectEnvelope::new(
            "tool:glob",
            ObjectKind::Tool,
            "Find files under the workspace by glob pattern (* and ? per segment, ** for any directory depth; case-sensitive)",
        );
        envelope.spec = json!({"type":"object","properties":{
            "pattern":{"type":"string","minLength":1,"description":"Glob pattern relative to path, e.g. src/*.rs or **/*.txt; patterns without / match file names at any depth"},
            "path":{"type":"string","description":"Workspace-relative directory (default workspace root)"},
            "max_entries":{"type":"integer","minimum":1,"maximum":2000,"description":"Maximum matches returned (default 200)"}
        },"required":["pattern"],"additionalProperties":false});
        envelope
    }
    async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        if method != "call" {
            bail!("unknown glob method {method}");
        }
        if context.cancel.is_cancelled() {
            bail!("cancelled");
        }
        let pattern = input
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("pattern is required"))?;
        let segments = compile_glob_segments(pattern)?;
        // 不含 `/` 的模式按文件名在任意深度匹配（等价 **/pattern）。
        let segments = if segments.len() == 1 && segments[0] != "**" && !pattern.contains('/') {
            vec!["**".to_string(), segments[0].clone()]
        } else {
            segments
        };
        let requested = input.get("path").and_then(Value::as_str).unwrap_or(".");
        let root = within_workspace(&self.workspace, requested)?;
        if !root.is_dir() {
            bail!("path is not a directory: {requested}");
        }
        let max_entries = input
            .get("max_entries")
            .and_then(Value::as_u64)
            .unwrap_or(200) as usize;
        let cancel = context.cancel.clone();
        let walk = tokio::task::spawn_blocking(move || {
            let mut walker = GlobWalk {
                matches: Vec::new(),
                remaining: max_entries,
                truncated: false,
                skipped_symlinks: 0,
                skipped_unreadable: 0,
                cancelled: false,
            };
            let mut rel: Vec<String> = Vec::new();
            walker.run(&root, &mut rel, &segments, &cancel);
            walker
        });
        let walker = tokio::select! {
            walker = walk => walker.map_err(|error| anyhow::anyhow!("glob worker failed: {error}"))?,
            _ = context.cancel.cancelled() => bail!("cancelled"),
        };
        if walker.cancelled {
            bail!("cancelled");
        }
        Ok(json!({"pattern":pattern,
            "path":requested.strip_prefix("./").unwrap_or(requested),
            "matches":walker.matches,"truncated":walker.truncated,
            "skipped_symlinks":walker.skipped_symlinks,
            "skipped_unreadable":walker.skipped_unreadable}))
    }
}
fn strip_html_block(html: &str, tag: &str) -> String {
    // 大小写不敏感地剔除 <tag ...>...</tag> 整块内容；未闭合则丢弃其余部分。
    let lower = html.to_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut result = String::with_capacity(html.len());
    let mut cursor = 0usize;
    while let Some(found) = lower[cursor..].find(&open) {
        let start = cursor + found;
        let after = lower[start + open.len()..].chars().next();
        let boundary = matches!(
            after,
            Some('>') | Some('/') | Some(' ') | Some('\t') | Some('\n') | Some('\r')
        );
        if !boundary {
            // <scriptx 之类的巧合前缀按原文保留。
            result.push_str(&html[cursor..start + open.len()]);
            cursor = start + open.len();
            continue;
        }
        result.push_str(&html[cursor..start]);
        match lower[start..].find(&close) {
            Some(offset) => cursor = start + offset + close.len(),
            None => return result,
        }
    }
    result.push_str(&html[cursor..]);
    result
}
fn html_to_text(html: &str) -> String {
    // 简化的纯文本提取：剔除 script/style/注释与标签并解码常见实体；
    // 不是完整 HTML 解析，也不宣称转换为 Markdown。
    let mut text = strip_html_block(html, "script");
    text = strip_html_block(&text, "style");
    let without_comments = {
        let mut result = String::with_capacity(text.len());
        let mut rest = text.as_str();
        while let Some(start) = rest.find("<!--") {
            result.push_str(&rest[..start]);
            match rest[start..].find("-->") {
                Some(offset) => rest = &rest[start + offset + 3..],
                None => {
                    rest = "";
                    break;
                }
            }
        }
        result.push_str(rest);
        result
    };
    let mut tagged = String::with_capacity(without_comments.len());
    let mut in_tag = false;
    for character in without_comments.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            character if !in_tag => tagged.push(character),
            _ => {}
        }
    }
    tagged
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
struct WebFetchTool {
    network: Arc<RwLock<Arc<dyn NetworkPolicy>>>,
    client: reqwest::Client,
    max_bytes_default: usize,
}
#[async_trait]
impl Object for WebFetchTool {
    fn envelope(&self) -> ObjectEnvelope {
        let mut envelope = ObjectEnvelope::new(
            "tool:webfetch",
            ObjectKind::Tool,
            "Fetch a URL through the configured network policy; HTML is reduced to plain text",
        );
        envelope.spec = json!({"type":"object","properties":{
            "url":{"type":"string","description":"HTTP or HTTPS URL"},
            "max_bytes":{"type":"integer","minimum":1024,"maximum":1048576,"description":"Maximum body bytes returned (default 131072)"}
        },"required":["url"],"additionalProperties":false});
        envelope
    }
    async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        if method != "call" {
            bail!("unknown webfetch method {method}");
        }
        if context.cancel.is_cancelled() {
            bail!("cancelled");
        }
        let raw_url = input
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("url is required"))?;
        let max_bytes = input
            .get("max_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(self.max_bytes_default as u64) as usize;
        let max_bytes = max_bytes.min(WEBFETCH_MAX_BYTES_CAP);
        let policy = self
            .network
            .read()
            .map_err(|_| anyhow::anyhow!("network policy poisoned"))?
            .clone();
        let mut current = reqwest::Url::parse(raw_url)
            .map_err(|error| anyhow::anyhow!("invalid url: {error}"))?;
        if current.scheme() != "http" && current.scheme() != "https" {
            bail!("unsupported URL scheme: {}", current.scheme());
        }
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs(WEBFETCH_TIMEOUT_SECS);
        let mut final_url = current.clone();
        let mut response = None;
        for _hop in 0..=WEBFETCH_MAX_REDIRECTS {
            policy.authorize_url(&current).await?;
            let attempt = self.client.get(current.clone()).send();
            let outcome = tokio::select! {
                outcome = attempt => outcome.map_err(|error| anyhow::anyhow!("webfetch request failed for {current}: {error}"))?,
                _ = context.cancel.cancelled() => bail!("cancelled"),
                _ = tokio::time::sleep_until(deadline) => bail!("webfetch timeout"),
            };
            let status = outcome.status();
            if status.is_redirection() {
                let location = outcome
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| {
                        anyhow::anyhow!("redirect {} without Location header", status)
                    })?;
                current = current.join(location).map_err(|error| {
                    anyhow::anyhow!("invalid redirect location {location}: {error}")
                })?;
                final_url = current.clone();
                continue;
            }
            if !status.is_success() {
                bail!("HTTP {} for {current}", status.as_u16());
            }
            final_url = current.clone();
            response = Some(outcome);
            break;
        }
        let response = response.ok_or_else(|| anyhow::anyhow!("too many redirects"))?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let is_html = content_type.contains("text/html");
        let is_text = content_type.starts_with("text/")
            || [
                "application/json",
                "application/xml",
                "application/javascript",
                "application/x-yaml",
                "application/yaml",
            ]
            .contains(&content_type.as_str());
        if !is_html && !is_text {
            bail!("unsupported content type {content_type} for {final_url}");
        }
        let mut body = Vec::new();
        let mut truncated = false;
        let mut stream = response.bytes_stream();
        loop {
            let chunk = tokio::select! {
                chunk = stream.next() => match chunk {
                    Some(Ok(chunk)) => chunk,
                    Some(Err(error)) => bail!("webfetch body read failed for {final_url}: {error}"),
                    None => break,
                },
                _ = context.cancel.cancelled() => bail!("cancelled"),
                _ = tokio::time::sleep_until(deadline) => bail!("webfetch timeout"),
            };
            let retained = chunk.len().min(max_bytes.saturating_sub(body.len()));
            body.extend_from_slice(&chunk[..retained]);
            if retained < chunk.len() || body.len() >= max_bytes {
                truncated = true;
                break;
            }
        }
        let raw = String::from_utf8_lossy(&body).into_owned();
        let content = if is_html { html_to_text(&raw) } else { raw };
        Ok(
            json!({"url":final_url.as_str(),"status":status,"content_type":content_type,
            "content":content,"bytes":body.len(),"truncated":truncated}),
        )
    }
}
fn todo_path(data_dir: &Path, session_id: &str) -> Result<PathBuf> {
    if session_id.is_empty()
        || session_id.len() > 128
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        bail!("todo storage requires a session id of [A-Za-z0-9_-]");
    }
    Ok(data_dir.join("todos").join(format!("{session_id}.json")))
}
async fn read_todos(path: &Path) -> Result<Option<Vec<Value>>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            Ok(Some(serde_json::from_slice(&bytes).with_context(|| {
                format!("todo store {}", path.display())
            })?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
async fn write_todos(path: &Path, items: &[Value]) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let payload = serde_json::to_vec(items)?;
    let temp = path.with_extension("json.tmp");
    tokio::fs::write(&temp, &payload).await?;
    tokio::fs::rename(&temp, path).await?;
    Ok(())
}
fn todo_schema() -> Value {
    json!({"type":"array","maxItems":TODO_MAX_ITEMS,"items":{"type":"object","properties":{
        "task":{"type":"string","minLength":1,"maxLength":TODO_TASK_CHARS},
        "status":{"type":"string","enum":["pending","in_progress","completed"]},
        "note":{"type":"string","maxLength":TODO_NOTE_CHARS}
    },"required":["task","status"],"additionalProperties":false}})
}
struct TodoWriteTool {
    data_dir: PathBuf,
}
#[async_trait]
impl Object for TodoWriteTool {
    fn envelope(&self) -> ObjectEnvelope {
        let mut envelope = ObjectEnvelope::new(
            "tool:todo_write",
            ObjectKind::Tool,
            "Replace this session's task checklist and return the stored list",
        );
        envelope.spec = json!({"type":"object","properties":{"items":todo_schema()},"required":["items"],"additionalProperties":false});
        envelope
    }
    async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        if method != "call" {
            bail!("unknown todo_write method {method}");
        }
        if context.cancel.is_cancelled() {
            bail!("cancelled");
        }
        let items = input
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("items is required"))?;
        let path = todo_path(&self.data_dir, &context.session_id)?;
        write_todos(&path, items).await?;
        Ok(json!({"items":items,"count":items.len(),"replaced":true}))
    }
}
struct TodoReadTool {
    data_dir: PathBuf,
}
#[async_trait]
impl Object for TodoReadTool {
    fn envelope(&self) -> ObjectEnvelope {
        let mut envelope = ObjectEnvelope::new(
            "tool:todo_read",
            ObjectKind::Tool,
            "Read this session's stored task checklist",
        );
        envelope.spec = json!({"type":"object","properties":{},"additionalProperties":false});
        envelope
    }
    async fn invoke(
        &self,
        method: &str,
        _input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        if method != "call" {
            bail!("unknown todo_read method {method}");
        }
        if context.cancel.is_cancelled() {
            bail!("cancelled");
        }
        let path = todo_path(&self.data_dir, &context.session_id)?;
        match read_todos(&path).await? {
            Some(items) => Ok(json!({"items":items,"count":items.len(),"exists":true})),
            None => Ok(json!({"items":[],"count":0,"exists":false})),
        }
    }
}
/// 递归下降时跳过的构建缓存/依赖目录（与技能发现的忽略清单一致）。
const SKIP_DIRS: [&str; 3] = [".git", "node_modules", "target"];
const GREP_MAX_FILE_BYTES: u64 = 1024 * 1024;
const GREP_LINE_CHARS: usize = 240;
/// 目录树最大深度上限，防止符号链接环外的失控深树。
const WALK_MAX_DEPTH: u32 = 32;

fn sorted_children(dir: &Path) -> std::io::Result<Vec<std::fs::DirEntry>> {
    let mut children: Vec<std::fs::DirEntry> =
        std::fs::read_dir(dir)?.filter_map(Result::ok).collect();
    children.sort_by_key(|entry| entry.file_name());
    Ok(children)
}
fn relative_name(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}
/// 仅支持 `*` 与 `?` 的文件名通配（字面匹配其余字符），避免引入 glob 依赖。
fn glob_match(pattern: &str, name: &str) -> bool {
    fn inner(pattern: &[u8], name: &[u8]) -> bool {
        match (pattern.first(), name.first()) {
            (None, None) => true,
            (Some(b'*'), _) => {
                inner(&pattern[1..], name) || (!name.is_empty() && inner(pattern, &name[1..]))
            }
            (Some(b'?'), _) => !name.is_empty() && inner(&pattern[1..], &name[1..]),
            (Some(a), Some(b)) => a == b && inner(&pattern[1..], &name[1..]),
            _ => false,
        }
    }
    inner(pattern.as_bytes(), name.as_bytes())
}
struct ListFilesTool {
    workspace: PathBuf,
}
struct ListWalk {
    base: PathBuf,
    entries: Vec<Value>,
    remaining: usize,
    truncated: bool,
    skipped_symlinks: usize,
    skipped_unreadable: usize,
    cancelled: bool,
}
impl ListWalk {
    fn run(&mut self, dir: &Path, depth: u32, cancel: &CancellationToken) {
        if self.cancelled || self.remaining == 0 {
            return;
        }
        if cancel.is_cancelled() {
            self.cancelled = true;
            return;
        }
        if depth == 0 {
            return;
        }
        let children = match sorted_children(dir) {
            Ok(children) => children,
            Err(_) => {
                self.skipped_unreadable += 1;
                return;
            }
        };
        for child in children {
            if self.cancelled || self.remaining == 0 || cancel.is_cancelled() {
                if cancel.is_cancelled() {
                    self.cancelled = true;
                }
                if self.remaining == 0 {
                    self.truncated = true;
                }
                break;
            }
            let Ok(file_type) = child.file_type() else {
                continue;
            };
            let path = child.path();
            let name = relative_name(&self.base, &path);
            if file_type.is_symlink() {
                self.skipped_symlinks += 1;
                continue;
            }
            let file_name = child.file_name().to_string_lossy().into_owned();
            if file_type.is_dir() {
                if SKIP_DIRS.contains(&file_name.as_str()) {
                    continue;
                }
                self.remaining -= 1;
                self.entries.push(json!({"name":name,"type":"dir"}));
                self.run(&path, depth - 1, cancel);
            } else {
                self.remaining -= 1;
                let bytes = child.metadata().map(|meta| meta.len()).unwrap_or(0);
                self.entries
                    .push(json!({"name":name,"type":"file","bytes":bytes}));
            }
        }
    }
}
#[async_trait]
impl Object for ListFilesTool {
    fn envelope(&self) -> ObjectEnvelope {
        let mut envelope = ObjectEnvelope::new(
            "tool:list_files",
            ObjectKind::Tool,
            "List files and directories under a workspace path (skips .git, node_modules, target)",
        );
        envelope.spec = json!({"type":"object","properties":{
            "path":{"type":"string","description":"Workspace-relative directory to list (default workspace root)"},
            "depth":{"type":"integer","minimum":1,"maximum":16,"description":"Directory levels to descend (default 4)"},
            "max_entries":{"type":"integer","minimum":1,"maximum":2000,"description":"Maximum entries returned (default 500)"}
        },"additionalProperties":false});
        envelope
    }
    async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        if method != "call" {
            bail!("unknown list_files method {method}");
        }
        if context.cancel.is_cancelled() {
            bail!("cancelled");
        }
        let requested = input.get("path").and_then(Value::as_str).unwrap_or(".");
        let root = within_workspace(&self.workspace, requested)?;
        if !root.is_dir() {
            bail!("path is not a directory: {requested}");
        }
        let depth = input
            .get("depth")
            .and_then(Value::as_u64)
            .unwrap_or(4)
            .min(WALK_MAX_DEPTH as u64) as u32;
        let max_entries = input
            .get("max_entries")
            .and_then(Value::as_u64)
            .unwrap_or(500) as usize;
        let cancel = context.cancel.clone();
        let walk = tokio::task::spawn_blocking(move || {
            let mut walker = ListWalk {
                base: root.clone(),
                entries: Vec::new(),
                remaining: max_entries,
                truncated: false,
                skipped_symlinks: 0,
                skipped_unreadable: 0,
                cancelled: false,
            };
            walker.run(&root, depth, &cancel);
            walker
        });
        let walker = tokio::select! {
            walker = walk => walker.map_err(|error| anyhow::anyhow!("list_files worker failed: {error}"))?,
            _ = context.cancel.cancelled() => bail!("cancelled"),
        };
        if walker.cancelled {
            bail!("cancelled");
        }
        Ok(json!({
            "path": requested.strip_prefix("./").unwrap_or(requested),
            "entries": walker.entries,
            "truncated": walker.truncated,
            "skipped_symlinks": walker.skipped_symlinks,
            "skipped_unreadable": walker.skipped_unreadable,
        }))
    }
}
struct GrepTool {
    workspace: PathBuf,
}
struct GrepWalk {
    base_dir: PathBuf,
    matches: Vec<Value>,
    remaining: usize,
    truncated: bool,
    files_searched: usize,
    skipped_binary: usize,
    skipped_large: usize,
    skipped_symlinks: usize,
    skipped_unreadable: usize,
    cancelled: bool,
}
impl GrepWalk {
    fn run(
        &mut self,
        dir: &Path,
        depth: u32,
        regex: &regex::Regex,
        glob: Option<&str>,
        cancel: &CancellationToken,
    ) {
        if self.cancelled || self.remaining == 0 || cancel.is_cancelled() {
            if cancel.is_cancelled() {
                self.cancelled = true;
            }
            return;
        }
        if depth == 0 {
            return;
        }
        let children = match sorted_children(dir) {
            Ok(children) => children,
            Err(_) => {
                self.skipped_unreadable += 1;
                return;
            }
        };
        for child in children {
            if self.cancelled || self.remaining == 0 || cancel.is_cancelled() {
                if cancel.is_cancelled() {
                    self.cancelled = true;
                }
                if self.remaining == 0 {
                    self.truncated = true;
                }
                break;
            }
            let Ok(file_type) = child.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                self.skipped_symlinks += 1;
                continue;
            }
            let path = child.path();
            let file_name = child.file_name().to_string_lossy().into_owned();
            if file_type.is_dir() {
                if SKIP_DIRS.contains(&file_name.as_str()) {
                    continue;
                }
                self.run(&path, depth - 1, regex, glob, cancel);
            } else {
                if let Some(pattern) = glob
                    && !glob_match(pattern, &file_name)
                {
                    continue;
                }
                self.search_file(&path, regex, cancel);
            }
        }
    }
    fn search_file(&mut self, path: &Path, regex: &regex::Regex, cancel: &CancellationToken) {
        let Ok(metadata) = std::fs::metadata(path) else {
            self.skipped_unreadable += 1;
            return;
        };
        if metadata.len() > GREP_MAX_FILE_BYTES {
            self.skipped_large += 1;
            return;
        }
        let Ok(bytes) = std::fs::read(path) else {
            self.skipped_unreadable += 1;
            return;
        };
        if bytes[..bytes.len().min(8192)].contains(&0u8) {
            self.skipped_binary += 1;
            return;
        }
        if cancel.is_cancelled() {
            self.cancelled = true;
            return;
        }
        let text = String::from_utf8_lossy(&bytes);
        let file = relative_name(&self.base_dir, path);
        for (index, line) in text.lines().enumerate() {
            if self.remaining == 0 {
                self.truncated = true;
                break;
            }
            if cancel.is_cancelled() {
                self.cancelled = true;
                return;
            }
            if regex.is_match(line) {
                self.remaining -= 1;
                let mut text = line.trim_end_matches('\r').to_string();
                if text.chars().count() > GREP_LINE_CHARS {
                    text = text.chars().take(GREP_LINE_CHARS).collect::<String>() + "[…]";
                }
                self.matches
                    .push(json!({"file":file,"line":index + 1,"text":text}));
            }
        }
        self.files_searched += 1;
    }
}
#[async_trait]
impl Object for GrepTool {
    fn envelope(&self) -> ObjectEnvelope {
        let mut envelope = ObjectEnvelope::new(
            "tool:grep",
            ObjectKind::Tool,
            "Search file contents under the workspace with a regular expression",
        );
        envelope.spec = json!({"type":"object","properties":{
            "pattern":{"type":"string","description":"Regular expression to search for"},
            "path":{"type":"string","description":"Workspace-relative file or directory (default workspace root)"},
            "glob":{"type":"string","description":"File name filter with * and ? wildcards, e.g. *.rs"},
            "ignore_case":{"type":"boolean","description":"Case-insensitive matching (default false)"},
            "max_results":{"type":"integer","minimum":1,"maximum":1000,"description":"Maximum matches returned (default 100)"}
        },"required":["pattern"],"additionalProperties":false});
        envelope
    }
    async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        if method != "call" {
            bail!("unknown grep method {method}");
        }
        if context.cancel.is_cancelled() {
            bail!("cancelled");
        }
        let pattern = input
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("pattern is required"))?;
        let compiled = if input
            .get("ignore_case")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            regex::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
        } else {
            regex::Regex::new(pattern)
        };
        let compiled = compiled.map_err(|error| anyhow::anyhow!("invalid pattern: {error}"))?;
        let glob = input.get("glob").and_then(Value::as_str).map(str::to_owned);
        if let Some(pattern) = glob.as_deref()
            && pattern.is_empty()
        {
            bail!("glob filter must not be empty");
        }
        let requested = input.get("path").and_then(Value::as_str).unwrap_or(".");
        let target = within_workspace(&self.workspace, requested)?;
        let max_results = input
            .get("max_results")
            .and_then(Value::as_u64)
            .unwrap_or(100) as usize;
        let cancel = context.cancel.clone();
        let walk = tokio::task::spawn_blocking(move || {
            let mut walker = GrepWalk {
                matches: Vec::new(),
                remaining: max_results,
                truncated: false,
                files_searched: 0,
                skipped_binary: 0,
                skipped_large: 0,
                skipped_symlinks: 0,
                skipped_unreadable: 0,
                cancelled: false,
                base_dir: target.clone(),
            };
            if target.is_dir() {
                walker.run(&target, WALK_MAX_DEPTH, &compiled, glob.as_deref(), &cancel);
            } else {
                walker.search_file(&target, &compiled, &cancel);
            }
            walker
        });
        let walker = tokio::select! {
            walker = walk => walker.map_err(|error| anyhow::anyhow!("grep worker failed: {error}"))?,
            _ = context.cancel.cancelled() => bail!("cancelled"),
        };
        if walker.cancelled {
            bail!("cancelled");
        }
        Ok(json!({
            "pattern": pattern,
            "matches": walker.matches,
            "truncated": walker.truncated,
            "files_searched": walker.files_searched,
            "skipped_binary": walker.skipped_binary,
            "skipped_large": walker.skipped_large,
            "skipped_symlinks": walker.skipped_symlinks,
            "skipped_unreadable": walker.skipped_unreadable,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    /// 回归：悬空符号链接的 exists()==false 会漏进"新建文件"分支，
    /// 写入会跟随链接逃逸 workspace；必须显式拒绝（unix 才能稳定建链）。
    #[cfg(unix)]
    #[test]
    fn within_workspace_rejects_dangling_symlink_targets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        std::fs::create_dir_all(&root).unwrap();
        // 悬空链接：目标不存在。
        std::os::unix::fs::symlink(dir.path().join("outside-missing"), root.join("dang.txt"))
            .unwrap();
        assert!(within_workspace(&root, "dang.txt").is_err());
        // 既有外部文件链接：exists() 为 true 走 canonicalize 分支同样拒绝。
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, "sentinel").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link.txt")).unwrap();
        assert!(within_workspace(&root, "link.txt").is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "sentinel");
    }

    #[tokio::test]
    async fn http_allowlist_permits_plain_http_host() {
        let policy = NetworkPolicyConfig {
            allow_http_hosts: vec!["192.0.2.10".into()],
            ..NetworkPolicyConfig::default()
        };
        policy
            .authorize_url(&"http://192.0.2.10:3000/v1".parse().unwrap())
            .await
            .expect("allowlisted host may use plain http");
        let error = policy
            .authorize_url(&"http://192.0.2.99:3000/v1".parse().unwrap())
            .await
            .expect_err("non-allowlisted http host stays denied");
        assert!(error.to_string().contains("HTTPS"));
    }

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

    fn test_context() -> InvocationContext {
        InvocationContext {
            session_id: "s".into(),
            trace_id: "t".into(),
            span_id: "p".into(),
            cancel: CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn default_policy_registers_full_toolset() {
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().into(),
            ..RuntimeConfig::default()
        };
        let registry =
            ToolRegistry::new(&config, SkillRegistry::default(), McpRegistry::default()).unwrap();
        let names: Vec<String> = registry
            .objects()
            .iter()
            .map(|object| object.envelope().id)
            .collect();
        for expected in [
            "tool:read_file",
            "tool:write_file",
            "tool:edit_file",
            "tool:list_files",
            "tool:glob",
            "tool:grep",
            "tool:webfetch",
            "tool:todo_write",
            "tool:todo_read",
            "tool:shell",
            "tool:load_skill",
        ] {
            assert!(names.contains(&expected.into()), "missing {expected}");
        }
    }

    async fn shell_call(workspace: &Path, command: &str) -> Value {
        let tool = ShellTool {
            workspace: workspace.to_path_buf(),
            max_bytes: 64 * 1024,
            timeout_ms: 60_000,
        };
        tool.invoke("call", json!({ "command": command }), test_context())
            .await
            .unwrap()
    }

    /// 回归：中文 Windows 上 PowerShell 默认按 GBK 输出，模型看到的报错/输出全是乱码；
    /// 包装后输出为 UTF-8，且退出码语义（成功 0、原生命令非零、exit N、cmdlet 失败）保持不变。
    #[tokio::test]
    async fn shell_outputs_utf8_and_preserves_exit_codes() {
        let dir = tempfile::tempdir().unwrap();
        let ok = shell_call(dir.path(), "echo 中文输出").await;
        assert_eq!(ok["exit_code"], json!(0), "{ok}");
        assert!(ok["stdout"].as_str().unwrap().contains("中文输出"), "{ok}");
        let exit = shell_call(dir.path(), "echo before; exit 7").await;
        assert_eq!(exit["exit_code"], json!(7), "{exit}");
        assert!(exit["stdout"].as_str().unwrap().contains("before"));
        assert_eq!(exit["is_error"], json!(true));
        if cfg!(windows) {
            let native = shell_call(dir.path(), "cmd /c exit 3").await;
            assert_eq!(native["exit_code"], json!(3), "{native}");
            let failed = shell_call(dir.path(), "Get-Item missing-zz").await;
            assert_eq!(failed["exit_code"], json!(1), "{failed}");
            let stderr = failed["stderr"].as_str().unwrap();
            assert!(
                stderr.contains("missing-zz") && !stderr.contains('\u{fffd}'),
                "{stderr}"
            );
            let recovered = shell_call(dir.path(), "Get-Item missing-zz; echo after").await;
            assert_eq!(recovered["exit_code"], json!(0), "{recovered}");
            let variables = shell_call(dir.path(), "$x = 5; Write-Output \"x=$x\"").await;
            assert!(
                variables["stdout"].as_str().unwrap().contains("x=5"),
                "{variables}"
            );
            // 文件 cmdlet 默认 UTF-8：就地改写 UTF-8 源码不得损坏中文。
            std::fs::write(dir.path().join("src.py"), "# 中文注释\nprint('ok')\n").unwrap();
            let rewrite = shell_call(
                dir.path(),
                "(Get-Content src.py -Raw) -replace 'ok','fine' | Set-Content src.py -NoNewline; Get-Content src.py",
            )
            .await;
            assert_eq!(rewrite["exit_code"], json!(0), "{rewrite}");
            let rewritten = decode_text(std::fs::read(dir.path().join("src.py")).unwrap()).unwrap();
            assert_eq!(rewritten, "# 中文注释\nprint('fine')\n");
            assert!(
                rewrite["stdout"].as_str().unwrap().contains("中文注释"),
                "{rewrite}"
            );
            assert!(shell_description().contains("PowerShell"));
        }
    }

    #[tokio::test]
    async fn list_files_lists_sorted_skips_and_respects_limits() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        std::fs::create_dir_all(dir.path().join("sub/deep")).unwrap();
        std::fs::write(dir.path().join("sub/b.txt"), "b").unwrap();
        std::fs::write(dir.path().join("sub/deep/c.txt"), "c").unwrap();
        for skipped in [".git", "node_modules", "target"] {
            std::fs::create_dir_all(dir.path().join(skipped)).unwrap();
            std::fs::write(dir.path().join(skipped).join("x"), "x").unwrap();
        }
        let config = RuntimeConfig {
            workspace: dir.path().into(),
            ..RuntimeConfig::default()
        };
        let registry =
            ToolRegistry::new(&config, SkillRegistry::default(), McpRegistry::default()).unwrap();
        let names = |result: &Value| -> Vec<String> {
            result["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["name"].as_str().unwrap().to_string())
                .collect()
        };
        let result = registry
            .call("tool:list_files", json!({}), test_context())
            .await
            .unwrap();
        let all = names(&result);
        for expected in ["a.txt", "sub", "sub/b.txt", "sub/deep/c.txt"] {
            assert!(
                all.iter().any(|name| name == expected),
                "missing {expected}"
            );
        }
        for skipped in [".git/x", "node_modules/y", "target/z"] {
            assert!(!all.iter().any(|name| name == skipped), "leaked {skipped}");
        }
        assert_eq!(result["truncated"], json!(false));

        let shallow = registry
            .call("tool:list_files", json!({"depth": 1}), test_context())
            .await
            .unwrap();
        let shallow_names = names(&shallow);
        assert!(shallow_names.iter().any(|name| name == "sub"));
        assert!(!shallow_names.iter().any(|name| name == "sub/b.txt"));

        let limited = registry
            .call("tool:list_files", json!({"max_entries": 2}), test_context())
            .await
            .unwrap();
        assert_eq!(names(&limited).len(), 2);
        assert_eq!(limited["truncated"], json!(true));

        let error = registry
            .call(
                "tool:list_files",
                json!({"path": "../outside"}),
                test_context(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("traversal"));
    }

    #[tokio::test]
    async fn grep_finds_matches_respects_glob_case_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("notes.txt"),
            "hello World\nplain line\nHELLO again",
        )
        .unwrap();
        std::fs::write(dir.path().join("code.rs"), "fn hello() {}\n").unwrap();
        std::fs::write(dir.path().join("data.bin"), b"\0hello\0").unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().into(),
            ..RuntimeConfig::default()
        };
        let registry =
            ToolRegistry::new(&config, SkillRegistry::default(), McpRegistry::default()).unwrap();

        let insensitive = registry
            .call(
                "tool:grep",
                json!({"pattern": "hello", "ignore_case": true}),
                test_context(),
            )
            .await
            .unwrap();
        assert_eq!(insensitive["matches"].as_array().unwrap().len(), 3);
        assert_eq!(insensitive["skipped_binary"], json!(1));
        assert_eq!(insensitive["truncated"], json!(false));

        let case_sensitive = registry
            .call(
                "tool:grep",
                json!({"pattern": "hello", "glob": "*.txt"}),
                test_context(),
            )
            .await
            .unwrap();
        let matches = case_sensitive["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["file"], json!("notes.txt"));
        assert_eq!(matches[0]["line"], json!(1));

        let limited = registry
            .call(
                "tool:grep",
                json!({"pattern": "hello", "ignore_case": true, "max_results": 1}),
                test_context(),
            )
            .await
            .unwrap();
        assert_eq!(limited["matches"].as_array().unwrap().len(), 1);
        assert_eq!(limited["truncated"], json!(true));

        let single_file = registry
            .call(
                "tool:grep",
                json!({"pattern": "hello", "path": "code.rs"}),
                test_context(),
            )
            .await
            .unwrap();
        assert_eq!(single_file["matches"].as_array().unwrap().len(), 1);

        let invalid = registry
            .call("tool:grep", json!({"pattern": "(["}), test_context())
            .await
            .unwrap_err();
        assert!(invalid.to_string().contains("invalid pattern"));
        let traversal = registry
            .call(
                "tool:grep",
                json!({"pattern": "x", "path": "../.."}),
                test_context(),
            )
            .await
            .unwrap_err();
        assert!(traversal.to_string().contains("traversal"));
    }

    #[tokio::test]
    async fn edit_file_replaces_exact_strings_and_enforces_uniqueness() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("doc.txt"),
            "alpha beta alpha 中文\r\nalpha tail",
        )
        .unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().into(),
            ..RuntimeConfig::default()
        };
        let registry =
            ToolRegistry::new(&config, SkillRegistry::default(), McpRegistry::default()).unwrap();

        let ambiguous = registry
            .call(
                "tool:edit_file",
                json!({"path": "doc.txt", "old_string": "alpha", "new_string": "GAMMA"}),
                test_context(),
            )
            .await
            .unwrap_err();
        assert!(ambiguous.to_string().contains("3 locations"));
        assert!(ambiguous.to_string().contains("replace_all"));

        let replaced = registry
            .call(
                "tool:edit_file",
                json!({"path": "doc.txt", "old_string": "alpha", "new_string": "GAMMA", "replace_all": true}),
                test_context(),
            )
            .await
            .unwrap();
        assert_eq!(replaced["replacements"], json!(3));
        let content = std::fs::read_to_string(dir.path().join("doc.txt")).unwrap();
        assert_eq!(content, "GAMMA beta GAMMA 中文\r\nGAMMA tail");

        let single = registry
            .call(
                "tool:edit_file",
                json!({"path": "doc.txt", "old_string": "beta", "new_string": "BETA"}),
                test_context(),
            )
            .await
            .unwrap();
        assert_eq!(single["replacements"], json!(1));
        assert!(
            std::fs::read_to_string(dir.path().join("doc.txt"))
                .unwrap()
                .contains("GAMMA BETA GAMMA")
        );

        let missing = registry
            .call(
                "tool:edit_file",
                json!({"path": "doc.txt", "old_string": "absent", "new_string": "x"}),
                test_context(),
            )
            .await
            .unwrap_err();
        assert!(missing.to_string().contains("not found"));

        let identical = registry
            .call(
                "tool:edit_file",
                json!({"path": "doc.txt", "old_string": "x", "new_string": "x"}),
                test_context(),
            )
            .await
            .unwrap_err();
        assert!(identical.to_string().contains("differ"));

        let no_create = registry
            .call(
                "tool:edit_file",
                json!({"path": "ghost.txt", "old_string": "x", "new_string": "y"}),
                test_context(),
            )
            .await
            .unwrap_err();
        assert!(no_create.to_string().contains("file not found"));

        let traversal = registry
            .call(
                "tool:edit_file",
                json!({"path": "../out.txt", "old_string": "x", "new_string": "y"}),
                test_context(),
            )
            .await
            .unwrap_err();
        assert!(traversal.to_string().contains("traversal"));
    }

    #[tokio::test]
    async fn glob_matches_patterns_recursively_and_respects_limits() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "a").unwrap();
        std::fs::write(dir.path().join("notes.md"), "n").unwrap();
        std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        std::fs::write(dir.path().join("src/b.rs"), "b").unwrap();
        std::fs::write(dir.path().join("src/deep/c.rs"), "c").unwrap();
        for skipped in [".git", "node_modules", "target"] {
            std::fs::create_dir_all(dir.path().join(skipped)).unwrap();
            std::fs::write(dir.path().join(skipped).join("x.rs"), "x").unwrap();
        }
        let config = RuntimeConfig {
            workspace: dir.path().into(),
            ..RuntimeConfig::default()
        };
        let registry =
            ToolRegistry::new(&config, SkillRegistry::default(), McpRegistry::default()).unwrap();
        async fn matches_for(registry: &ToolRegistry, pattern: &str) -> Vec<String> {
            registry
                .call("tool:glob", json!({"pattern": pattern}), test_context())
                .await
                .unwrap()["matches"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }

        assert_eq!(matches_for(&registry, "./a.rs").await, vec!["a.rs"]);
        assert_eq!(matches_for(&registry, "src/*.rs").await, vec!["src/b.rs"]);
        assert_eq!(
            matches_for(&registry, "**/*.rs").await,
            vec!["a.rs", "src/b.rs", "src/deep/c.rs"]
        );
        assert_eq!(
            matches_for(&registry, "*.rs").await,
            vec!["a.rs", "src/b.rs", "src/deep/c.rs"]
        );
        assert_eq!(
            matches_for(&registry, "**").await,
            vec!["a.rs", "notes.md", "src/b.rs", "src/deep/c.rs"]
        );
        assert_eq!(matches_for(&registry, "./?.rs").await, vec!["a.rs"]);
        assert!(matches_for(&registry, "src").await.is_empty());

        let limited = registry
            .call(
                "tool:glob",
                json!({"pattern": "**", "max_entries": 2}),
                test_context(),
            )
            .await
            .unwrap();
        assert_eq!(limited["matches"].as_array().unwrap().len(), 2);
        assert_eq!(limited["truncated"], json!(true));

        let traversal = registry
            .call(
                "tool:glob",
                json!({"pattern": "../out/*.rs"}),
                test_context(),
            )
            .await
            .unwrap_err();
        assert!(traversal.to_string().contains("traversal"));
    }

    async fn webfetch_fixture() -> (String, tokio::task::JoinHandle<()>) {
        use axum::{Router, http::StatusCode, response::Html, response::Redirect, routing::get};
        let listener = crate::test_support::bind_loopback().await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/text", get(|| async { "plain body" }))
            .route(
                "/html",
                get(|| async {
                    Html("<html><head><style>p{color:red}</style></head><body><script>alert(1)</script><h1>标题</h1><p>Hello &amp; world</p></body></html>")
                }),
            )
            .route("/redirect", get(Redirect::temporary("/text")))
            .route(
                "/redirect-private",
                get(Redirect::temporary("http://10.255.255.1:1/x")),
            )
            .route("/missing", get(|| async { StatusCode::NOT_FOUND }))
            .route(
                "/binary",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
                        "0000",
                    )
                }),
            )
            .route("/big", get(|| async { "x".repeat(3000) }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{}", address), server)
    }

    #[tokio::test]
    async fn webfetch_extracts_text_and_follows_allowed_redirects() {
        let (base, _server) = webfetch_fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().into(),
            ..RuntimeConfig::default()
        };
        let registry =
            ToolRegistry::new(&config, SkillRegistry::default(), McpRegistry::default()).unwrap();

        let text = registry
            .call(
                "tool:webfetch",
                json!({"url": format!("{base}/text")}),
                test_context(),
            )
            .await
            .unwrap();
        assert_eq!(text["status"], json!(200));
        assert_eq!(text["content"], json!("plain body"));
        assert_eq!(text["truncated"], json!(false));

        let html = registry
            .call(
                "tool:webfetch",
                json!({"url": format!("{base}/html")}),
                test_context(),
            )
            .await
            .unwrap();
        let content = html["content"].as_str().unwrap();
        assert!(content.contains("标题"));
        assert!(content.contains("Hello & world"));
        assert!(!content.contains('<'));
        assert!(!content.contains("alert"));

        let redirected = registry
            .call(
                "tool:webfetch",
                json!({"url": format!("{base}/redirect")}),
                test_context(),
            )
            .await
            .unwrap();
        assert_eq!(redirected["content"], json!("plain body"));
        assert!(redirected["url"].as_str().unwrap().ends_with("/text"));
    }

    #[tokio::test]
    async fn webfetch_enforces_policy_status_and_size_limits() {
        let (base, _server) = webfetch_fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().into(),
            ..RuntimeConfig::default()
        };
        let registry =
            ToolRegistry::new(&config, SkillRegistry::default(), McpRegistry::default()).unwrap();

        let denied = registry
            .call(
                "tool:webfetch",
                json!({"url": format!("{base}/redirect-private")}),
                test_context(),
            )
            .await
            .unwrap_err();
        assert!(denied.to_string().contains("denied"));

        let missing = registry
            .call(
                "tool:webfetch",
                json!({"url": format!("{base}/missing")}),
                test_context(),
            )
            .await
            .unwrap_err();
        assert!(missing.to_string().contains("HTTP 404"));

        let binary = registry
            .call(
                "tool:webfetch",
                json!({"url": format!("{base}/binary")}),
                test_context(),
            )
            .await
            .unwrap_err();
        assert!(binary.to_string().contains("unsupported content type"));

        let oversized = registry
            .call(
                "tool:webfetch",
                json!({"url": format!("{base}/big"), "max_bytes": 1024}),
                test_context(),
            )
            .await
            .unwrap();
        assert_eq!(oversized["truncated"], json!(true));
        assert_eq!(oversized["bytes"], json!(1024));

        let scheme = registry
            .call(
                "tool:webfetch",
                json!({"url": "ftp://example.invalid/x"}),
                test_context(),
            )
            .await
            .unwrap_err();
        assert!(scheme.to_string().contains("unsupported URL scheme"));

        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let error = registry
            .call(
                "tool:webfetch",
                json!({"url": format!("{base}/text")}),
                InvocationContext {
                    session_id: "s".into(),
                    trace_id: "t".into(),
                    span_id: "p".into(),
                    cancel: cancelled,
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    }

    #[tokio::test]
    async fn todo_tools_roundtrip_isolate_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().into(),
            data_dir: dir.path().join("data"),
            ..RuntimeConfig::default()
        };
        let registry =
            ToolRegistry::new(&config, SkillRegistry::default(), McpRegistry::default()).unwrap();
        let context_for = |session: &str| InvocationContext {
            session_id: session.into(),
            trace_id: "t".into(),
            span_id: "p".into(),
            cancel: CancellationToken::new(),
        };
        let items = json!([
            {"task": "实现功能", "status": "pending"},
            {"task": "写测试", "status": "in_progress", "note": "覆盖边界"}
        ]);
        let written = registry
            .call(
                "tool:todo_write",
                json!({"items": items}),
                context_for("session-A"),
            )
            .await
            .unwrap();
        assert_eq!(written["count"], json!(2));

        let read = registry
            .call("tool:todo_read", json!({}), context_for("session-A"))
            .await
            .unwrap();
        assert_eq!(read["exists"], json!(true));
        assert_eq!(read["items"], items);

        let other = registry
            .call("tool:todo_read", json!({}), context_for("session-B"))
            .await
            .unwrap();
        assert_eq!(other["exists"], json!(false));
        assert_eq!(other["items"], json!([]));

        let replaced = registry
            .call(
                "tool:todo_write",
                json!({"items": [{"task": "收尾", "status": "completed"}]}),
                context_for("session-A"),
            )
            .await
            .unwrap();
        assert_eq!(replaced["count"], json!(1));
        let read = registry
            .call("tool:todo_read", json!({}), context_for("session-A"))
            .await
            .unwrap();
        assert_eq!(read["count"], json!(1));

        // 存储按会话隔离且跨 registry 重建持久（temp+rename 原子发布）。
        let registry_two =
            ToolRegistry::new(&config, SkillRegistry::default(), McpRegistry::default()).unwrap();
        let read = registry_two
            .call("tool:todo_read", json!({}), context_for("session-A"))
            .await
            .unwrap();
        assert_eq!(read["count"], json!(1));

        let invalid_status = registry
            .call(
                "tool:todo_write",
                json!({"items": [{"task": "x", "status": "archived"}]}),
                context_for("session-A"),
            )
            .await
            .unwrap_err();
        assert!(invalid_status.to_string().contains("schema validation"));

        let bad_session = registry
            .call("tool:todo_read", json!({}), context_for("bad/session"))
            .await
            .unwrap_err();
        assert!(bad_session.to_string().contains("session id"));
    }

    #[test]
    fn decode_text_handles_boms_and_utf16() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFhi".to_vec()).unwrap(), "hi");
        let le: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("中a".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(decode_text(le).unwrap(), "中a");
        let be: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain("ok".encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        assert_eq!(decode_text(be).unwrap(), "ok");
        assert!(decode_text(vec![0xC3, 0x28]).is_err());
    }

    #[test]
    fn within_workspace_allows_nested_new_directories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        std::fs::create_dir_all(&root).unwrap();
        // 多级尚不存在的目录：允许（调用方负责创建），且仍限制在 workspace 内。
        let nested = within_workspace(&root, "pkg/sub/deep/file.py").unwrap();
        assert!(nested.ends_with("pkg/sub/deep/file.py"));
        assert!(within_workspace(&root, "pkg/../../escape.txt").is_err());
        assert_eq!(
            workspace_relative(
                &root,
                &std::fs::canonicalize(&root)
                    .unwrap()
                    .join("a")
                    .join("b.txt")
            ),
            "a/b.txt"
        );
    }

    #[tokio::test]
    async fn read_file_returns_line_windows() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lines.txt"), "l1\nl2\nl3\nl4\n").unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().into(),
            ..RuntimeConfig::default()
        };
        let registry =
            ToolRegistry::new(&config, SkillRegistry::default(), McpRegistry::default()).unwrap();

        let whole = registry
            .call(
                "tool:read_file",
                json!({"path": "lines.txt"}),
                test_context(),
            )
            .await
            .unwrap();
        assert_eq!(whole["content"], json!("l1\nl2\nl3\nl4\n"));
        assert_eq!(whole["total_lines"], json!(4));
        assert_eq!(whole["truncated_lines"], json!(false));

        let window = registry
            .call(
                "tool:read_file",
                json!({"path": "lines.txt", "offset": 2, "limit": 2}),
                test_context(),
            )
            .await
            .unwrap();
        assert_eq!(window["content"], json!("l2\nl3"));
        assert_eq!(window["start_line"], json!(2));
        assert_eq!(window["end_line"], json!(3));
        assert_eq!(window["truncated_lines"], json!(true));

        let beyond = registry
            .call(
                "tool:read_file",
                json!({"path": "lines.txt", "offset": 99}),
                test_context(),
            )
            .await
            .unwrap();
        assert_eq!(beyond["content"], json!(""));
        assert_eq!(beyond["truncated_lines"], json!(false));
    }

    #[tokio::test]
    async fn new_tools_honour_pre_cancelled_context() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("doc.txt"), "x").unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().into(),
            data_dir: dir.path().join("data"),
            ..RuntimeConfig::default()
        };
        let registry =
            ToolRegistry::new(&config, SkillRegistry::default(), McpRegistry::default()).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let context = InvocationContext {
            session_id: "s".into(),
            trace_id: "t".into(),
            span_id: "p".into(),
            cancel,
        };
        let cases = [
            (
                "tool:edit_file",
                json!({"path": "doc.txt", "old_string": "x", "new_string": "y"}),
            ),
            ("tool:glob", json!({"pattern": "*"})),
            ("tool:webfetch", json!({"url": "http://127.0.0.1:1/"})),
            ("tool:todo_write", json!({"items": []})),
            ("tool:todo_read", json!({})),
        ];
        for (name, input) in cases {
            let error = registry
                .call(name, input, context.clone())
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains("cancelled"),
                "{name} ignored cancel"
            );
        }
    }
}
