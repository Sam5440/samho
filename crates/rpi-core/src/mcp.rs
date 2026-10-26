//! MCP client integration backed by rmcp.
//!
//! This module intentionally delegates JSON-RPC framing, protocol negotiation,
//! messages and cancellation notifications to rmcp. It maps configured
//! servers into the rpi Object model and exposes a stable JSON API to callers.
#![allow(deprecated)] // rmcp 3.5.1 retains legacy sampling/roots/logging for negotiated peers.

pub mod oauth;

use crate::object::{InvocationContext, Object, ObjectEnvelope, ObjectKind};
use crate::tools::{NetworkPolicy, NetworkPolicyConfig};
use anyhow::Result;
use async_trait::async_trait;
use rmcp::model::*;
use rmcp::service::{PeerRequestOptions, RunningService, serve_client_with_ct};
use rmcp::transport::TokioChildProcess;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::{ClientHandler, Peer, RoleClient, ServiceError};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpClientOptions {
    pub roots: Vec<std::path::PathBuf>,
    pub enable_sampling: bool,
    pub enable_elicitation: bool,
    pub enable_tasks: bool,
    pub max_sampling_tokens: u32,
}
#[derive(Clone)]
pub struct McpCallbackContext {
    pub server: String,
    pub cancel: CancellationToken,
}
/// Host-provided model/user interaction. No default hook silently accepts or simulates input.
#[async_trait]
pub trait McpClientHooks: Send + Sync {
    async fn bearer_token(&self, _server: &str) -> Result<Option<String>> {
        Ok(None)
    }
    async fn sample(
        &self,
        _params: CreateMessageRequestParams,
        _context: McpCallbackContext,
    ) -> std::result::Result<CreateMessageResult, rmcp::ErrorData> {
        Err(rmcp::ErrorData::method_not_found::<
            CreateMessageRequestMethod,
        >())
    }
    async fn elicit(
        &self,
        _params: ElicitRequestParams,
        _context: McpCallbackContext,
    ) -> std::result::Result<ElicitResult, rmcp::ErrorData> {
        Err(rmcp::ErrorData::method_not_found::<
            ElicitationCreateRequestMethod,
        >())
    }
    fn notification(&self, _server: &str, _method: &str, _payload: &Value) {}
    async fn task_input(
        &self,
        _requests: Value,
        _context: McpCallbackContext,
    ) -> std::result::Result<Value, rmcp::ErrorData> {
        Err(rmcp::ErrorData::new(
            ErrorCode::METHOD_NOT_FOUND,
            "task input callback not configured",
            None,
        ))
    }
}
#[derive(Default)]
struct ClientState {
    roots: std::sync::RwLock<Vec<Root>>,
    events: std::sync::Mutex<VecDeque<Value>>,
}
impl ClientState {
    fn record(
        &self,
        server: &str,
        method: &str,
        payload: Value,
        hooks: &Option<Arc<dyn McpClientHooks>>,
    ) {
        let mut events = self.events.lock().expect("MCP notification state poisoned");
        if events.len() == 256 {
            events.pop_front();
        }
        events.push_back(json!({"server":server,"method":method,"payload":payload}));
        drop(events);
        if let Some(hooks) = hooks {
            hooks.notification(server, method, &payload);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpServerConfig {
    pub name: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub env: BTreeMap<String, String>,
    pub bearer_token_env: Option<String>,
    pub enabled: bool,
    pub timeout_ms: u64,
    pub startup_timeout_ms: u64,
    pub cwd: Option<String>,
    pub scrub_env: bool,
    pub max_discovery_pages: usize,
    pub client: McpClientOptions,
}
impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            command: None,
            args: Vec::new(),
            url: None,
            env: BTreeMap::new(),
            bearer_token_env: None,
            enabled: true,
            timeout_ms: 60_000,
            startup_timeout_ms: 30_000,
            cwd: None,
            scrub_env: true,
            max_discovery_pages: 100,
            client: McpClientOptions::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpError {
    Configuration {
        server: String,
        message: String,
    },
    Disconnected {
        server: String,
        message: String,
    },
    InvalidParams {
        server: String,
        method: String,
        message: String,
    },
    Unsupported {
        server: String,
        method: String,
        message: String,
    },
    Remote {
        server: String,
        method: String,
        code: i32,
        message: String,
        data: Option<Value>,
    },
    Transport {
        server: String,
        method: String,
        message: String,
    },
    Timeout {
        server: String,
        method: String,
        timeout_ms: u64,
    },
    Cancelled {
        server: String,
        method: String,
    },
    ToolFailure {
        server: String,
        method: String,
        result: Value,
    },
    InvalidResponse {
        server: String,
        method: String,
        message: String,
    },
}
impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration { server, message } => {
                write!(f, "MCP {server} configuration: {message}")
            }
            Self::Disconnected { server, message } => {
                write!(f, "MCP {server} disconnected: {message}")
            }
            Self::InvalidParams {
                server,
                method,
                message,
            } => write!(f, "MCP {server} {method} invalid parameters: {message}"),
            Self::Unsupported {
                server,
                method,
                message,
            } => write!(f, "MCP {server} {method} unsupported: {message}"),
            Self::Remote {
                server,
                method,
                code,
                message,
                ..
            } => write!(f, "MCP {server} {method} remote error {code}: {message}"),
            Self::Transport {
                server,
                method,
                message,
            } => write!(f, "MCP {server} {method} transport error: {message}"),
            Self::Timeout {
                server,
                method,
                timeout_ms,
            } => write!(f, "MCP {server} {method} timed out after {timeout_ms} ms"),
            Self::Cancelled { server, method } => write!(f, "MCP {server} {method} cancelled"),
            Self::ToolFailure { server, method, .. } => {
                write!(f, "MCP {server} {method} returned isError")
            }
            Self::InvalidResponse {
                server,
                method,
                message,
            } => write!(f, "MCP {server} {method} invalid response: {message}"),
        }
    }
}
impl std::error::Error for McpError {}

fn service_error(server: &str, method: &str, error: ServiceError) -> McpError {
    match error {
        ServiceError::McpError(error) => McpError::Remote {
            server: server.into(),
            method: method.into(),
            code: error.code.0,
            message: error.message.into_owned(),
            data: error.data,
        },
        ServiceError::Timeout { timeout } => McpError::Timeout {
            server: server.into(),
            method: method.into(),
            timeout_ms: timeout.as_millis() as u64,
        },
        ServiceError::Cancelled { .. } => McpError::Cancelled {
            server: server.into(),
            method: method.into(),
        },
        other => McpError::Transport {
            server: server.into(),
            method: method.into(),
            message: other.to_string(),
        },
    }
}

#[derive(Clone)]
struct HarnessClient {
    tools_changed: Arc<AtomicBool>,
    state: Arc<ClientState>,
    server: String,
    options: McpClientOptions,
    hooks: Option<Arc<dyn McpClientHooks>>,
    timeout_ms: u64,
}
impl ClientHandler for HarnessClient {
    fn get_info(&self) -> ClientConfig {
        let mut capabilities = ClientCapabilities::default();
        if !self.options.roots.is_empty() {
            let mut roots = RootsCapabilities::default();
            roots.list_changed = Some(true);
            capabilities.roots = Some(roots);
        }
        if self.options.enable_sampling && self.hooks.is_some() {
            capabilities.sampling = Some(SamplingCapability::default());
        }
        if self.options.enable_elicitation && self.hooks.is_some() {
            capabilities.elicitation = Some(
                ElicitationCapability::new()
                    .with_form(FormElicitationCapability::new().with_schema_validation(true)),
            );
        }
        if self.options.enable_tasks {
            capabilities.extensions = Some(BTreeMap::from([(
                TASKS_EXTENSION_ID.into(),
                Default::default(),
            )]));
        }
        ClientConfig::new(
            capabilities,
            Implementation::new("rpi", env!("CARGO_PKG_VERSION")),
        )
    }
    async fn create_message(
        &self,
        params: CreateMessageRequestParams,
        context: rmcp::service::RequestContext<RoleClient>,
    ) -> std::result::Result<CreateMessageResult, rmcp::ErrorData> {
        if !self.options.enable_sampling {
            return Err(rmcp::ErrorData::method_not_found::<
                CreateMessageRequestMethod,
            >());
        }
        let limit = if self.options.max_sampling_tokens == 0 {
            8192
        } else {
            self.options.max_sampling_tokens
        };
        if params.max_tokens > limit || params.tools.is_some() {
            return Err(rmcp::ErrorData::invalid_params(
                "sampling token limit exceeded or unadvertised sampling tools requested",
                None,
            ));
        }
        let hooks = self
            .hooks
            .as_ref()
            .ok_or_else(rmcp::ErrorData::method_not_found::<CreateMessageRequestMethod>)?;
        let result = tokio::select! {
            _=context.ct.cancelled()=>Err(rmcp::ErrorData::new(ErrorCode(-32800),"sampling cancelled",None)),
            result=tokio::time::timeout(Duration::from_millis(self.timeout_ms),hooks.sample(params,McpCallbackContext{server:self.server.clone(),cancel:context.ct.clone()}))=>result.map_err(|_|rmcp::ErrorData::internal_error("sampling callback timed out",None))?,
        }?;
        result
            .validate()
            .map_err(|error| rmcp::ErrorData::invalid_params(error, None))?;
        Ok(result)
    }
    async fn list_roots(
        &self,
        _context: rmcp::service::RequestContext<RoleClient>,
    ) -> std::result::Result<ListRootsResult, rmcp::ErrorData> {
        if self.options.roots.is_empty() {
            return Err(rmcp::ErrorData::method_not_found::<ListRootsRequestMethod>());
        }
        Ok(ListRootsResult::new(
            self.state.roots.read().expect("MCP roots poisoned").clone(),
        ))
    }
    async fn create_elicitation(
        &self,
        params: ElicitRequestParams,
        context: rmcp::service::RequestContext<RoleClient>,
    ) -> std::result::Result<ElicitResult, rmcp::ErrorData> {
        if !self.options.enable_elicitation {
            return Err(rmcp::ErrorData::method_not_found::<
                ElicitationCreateRequestMethod,
            >());
        }
        let schema = match &params {
            ElicitRequestParams::FormElicitationParams {
                requested_schema, ..
            } => serde_json::to_value(requested_schema)
                .map_err(|error| rmcp::ErrorData::invalid_params(error.to_string(), None))?,
            _ => {
                return Err(rmcp::ErrorData::invalid_params(
                    "URL elicitation is not advertised",
                    None,
                ));
            }
        };
        let hooks = self
            .hooks
            .as_ref()
            .ok_or_else(rmcp::ErrorData::method_not_found::<ElicitationCreateRequestMethod>)?;
        let result = tokio::select! {
            _=context.ct.cancelled()=>Err(rmcp::ErrorData::new(ErrorCode(-32800),"elicitation cancelled",None)),
            result=tokio::time::timeout(Duration::from_millis(self.timeout_ms),hooks.elicit(params,McpCallbackContext{server:self.server.clone(),cancel:context.ct.clone()}))=>result.map_err(|_|rmcp::ErrorData::internal_error("elicitation callback timed out",None))?,
        }?;
        if result.action == ElicitationAction::Accept {
            crate::tools::validate_schema(
                &schema,
                result.content.as_ref().unwrap_or(&Value::Null),
                "elicitation content",
            )
            .map_err(|error| rmcp::ErrorData::invalid_params(error.to_string(), None))?;
        } else if result.content.is_some() {
            return Err(rmcp::ErrorData::invalid_params(
                "declined/cancelled elicitation must not carry content",
                None,
            ));
        }
        Ok(result)
    }
    fn on_tool_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        self.tools_changed.store(true, Ordering::Release);
        self.state.record(
            &self.server,
            "notifications/tools/list_changed",
            json!({}),
            &self.hooks,
        );
        std::future::ready(())
    }
    async fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        self.state.record(
            &self.server,
            "notifications/resources/updated",
            serde_json::to_value(params).unwrap_or(Value::Null),
            &self.hooks,
        );
    }
    async fn on_resource_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        self.state.record(
            &self.server,
            "notifications/resources/list_changed",
            json!({}),
            &self.hooks,
        );
    }
    async fn on_prompt_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        self.state.record(
            &self.server,
            "notifications/prompts/list_changed",
            json!({}),
            &self.hooks,
        );
    }
    async fn on_task_status(
        &self,
        params: TaskStatusNotificationParams,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        self.state.record(
            &self.server,
            "notifications/tasks",
            serde_json::to_value(params).unwrap_or(Value::Null),
            &self.hooks,
        );
    }
}
type ClientService = RunningService<RoleClient, HarnessClient>;

#[derive(Clone)]
struct ToolEntry {
    raw_name: String,
    schema: Value,
    input_schema: Value,
    output_schema: Option<Value>,
}
struct Connection {
    peer: Peer<RoleClient>,
    service: Mutex<Option<ClientService>>,
    shutdown: CancellationToken,
    info: Value,
    tools_changed: Arc<AtomicBool>,
    client_state: Arc<ClientState>,
    hooks: Option<Arc<dyn McpClientHooks>>,
}
pub struct McpServerObject {
    pub config: McpServerConfig,
    connection: Option<Connection>,
    state: RwLock<Value>,
    tools: RwLock<BTreeMap<String, ToolEntry>>,
    refresh_lock: Mutex<()>,
}

impl McpServerObject {
    async fn connect(
        config: McpServerConfig,
        hooks: Option<Arc<dyn McpClientHooks>>,
        policy: Arc<dyn NetworkPolicy>,
    ) -> Self {
        let name = config.name.clone();
        if !config.enabled {
            return Self::unavailable(config, json!({"name":name,"state":"disabled"}));
        }
        match connect_service(&config, hooks, policy).await {
            Err(error) => {
                Self::unavailable(config, json!({"name":name,"state":"failed","error":error}))
            }
            Ok(connection) => {
                let object = Self {
                    config,
                    connection: Some(connection),
                    state: RwLock::new(json!({"name":name,"state":"connected"})),
                    tools: RwLock::new(BTreeMap::new()),
                    refresh_lock: Mutex::new(()),
                };
                if object.has_capability("tools")
                    && let Err(error) = object.refresh_tools(CancellationToken::new()).await
                {
                    *object.state.write().await =
                        json!({"name":name,"state":"degraded","error":error_json(&error)});
                }
                object
            }
        }
    }
    fn unavailable(config: McpServerConfig, state: Value) -> Self {
        Self {
            config,
            connection: None,
            state: RwLock::new(state),
            tools: RwLock::new(BTreeMap::new()),
            refresh_lock: Mutex::new(()),
        }
    }
    fn has_capability(&self, capability: &str) -> bool {
        self.connection.as_ref().is_some_and(|c| {
            !c.info
                .pointer(&format!("/capabilities/{capability}"))
                .unwrap_or(&Value::Null)
                .is_null()
        })
    }
    async fn status(&self) -> Value {
        let mut state = self.state.read().await.clone();
        if let Some(connection) = &self.connection {
            if connection.shutdown.is_cancelled() || connection.peer.is_transport_closed() {
                state["state"] = json!("closed");
            }
            state["server_info"] = connection.info.clone();
        }
        state["transport"] = json!(if self.config.url.is_some() {
            "streamable_http"
        } else {
            "stdio"
        });
        state["tool_count"] = json!(self.tools.read().await.len());
        state
    }
    fn invalid_response(&self, method: &str, message: impl Into<String>) -> anyhow::Error {
        McpError::InvalidResponse {
            server: self.config.name.clone(),
            method: method.into(),
            message: message.into(),
        }
        .into()
    }
    async fn refresh_tools(&self, cancel: CancellationToken) -> Result<()> {
        let _guard = self.refresh_lock.lock().await;
        if let Some(connection) = &self.connection {
            // Clear before discovery; notifications arriving during discovery stay dirty.
            connection.tools_changed.store(false, Ordering::Release);
        }
        let mut cursor = None;
        let mut cursors = BTreeSet::new();
        let mut generation = BTreeMap::new();
        for _ in 0..self.config.max_discovery_pages {
            let mut params = serde_json::Map::new();
            if let Some(value) = &cursor {
                params.insert("cursor".into(), json!(value));
            }
            let result: ListToolsResult = serde_json::from_value(
                self.request_inner("tools/list", Value::Object(params), cancel.clone(), false)
                    .await?,
            )
            .map_err(|e| self.invalid_response("tools/list", e.to_string()))?;
            for tool in result.tools {
                let public_name = public_tool_name(&self.config.name, &tool.name)?;
                let input_schema = serde_json::to_value(&tool.input_schema)?;
                let output_schema = tool
                    .output_schema
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()?;
                if generation.insert(public_name.clone(),ToolEntry { raw_name:tool.name.to_string(),schema:json!({"type":"function","function":{"name":public_name,"description":tool.description.unwrap_or_default(),"parameters":input_schema}}),input_schema,output_schema}).is_some() { return Err(self.invalid_response("tools/list",format!("duplicate normalized tool name: {public_name}"))); }
            }
            cursor = result.next_cursor;
            if cursor.is_none() {
                *self.tools.write().await = generation;
                *self.state.write().await = json!({"name":self.config.name,"state":"connected"});
                return Ok(());
            }
            if !cursors.insert(cursor.clone().unwrap_or_default()) {
                return Err(self.invalid_response("tools/list", "repeated pagination cursor"));
            }
        }
        Err(self.invalid_response("tools/list", "pagination page limit exceeded"))
    }
    async fn request_inner(
        &self,
        method: &str,
        params: Value,
        cancel: CancellationToken,
        enforce_tool_discovery: bool,
    ) -> Result<Value> {
        let method = canonical_method(method);
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| McpError::Disconnected {
                server: self.config.name.clone(),
                message: "connection unavailable".into(),
            })?;
        if connection.shutdown.is_cancelled() || connection.peer.is_transport_closed() {
            return Err(McpError::Disconnected {
                server: self.config.name.clone(),
                message: "connection closed".into(),
            }
            .into());
        }
        let capability = match method {
            "tools/list" | "tools/call" => Some("tools"),
            "resources/list" | "resources/templates/list" | "resources/read" => Some("resources"),
            "resources/subscribe" | "resources/unsubscribe" => Some("resources"),
            "prompts/list" | "prompts/get" => Some("prompts"),
            "completion/complete" => Some("completions"),
            "tasks/get" | "tasks/update" | "tasks/cancel" => {
                Some("extensions/io.modelcontextprotocol~1tasks")
            }
            "ping" => None,
            _ => {
                return Err(McpError::Unsupported {
                    server: self.config.name.clone(),
                    method: method.into(),
                    message: "method is outside implemented capability matrix".into(),
                }
                .into());
            }
        };
        if let Some(capability) = capability
            && !self.has_capability(capability)
        {
            return Err(McpError::Unsupported {
                server: self.config.name.clone(),
                method: method.into(),
                message: format!("server did not advertise {capability}"),
            }
            .into());
        }
        if matches!(method, "resources/subscribe" | "resources/unsubscribe")
            && self
                .connection
                .as_ref()
                .and_then(|connection| connection.info.pointer("/capabilities/resources/subscribe"))
                .and_then(Value::as_bool)
                != Some(true)
        {
            return Err(McpError::Unsupported {
                server: self.config.name.clone(),
                method: method.into(),
                message: "server did not advertise resource subscriptions".into(),
            }
            .into());
        }
        if method.starts_with("tasks/") && !self.config.client.enable_tasks {
            return Err(McpError::Unsupported {
                server: self.config.name.clone(),
                method: method.into(),
                message: "tasks extension is not enabled for this client".into(),
            }
            .into());
        }
        if cancel.is_cancelled() {
            return Err(McpError::Cancelled {
                server: self.config.name.clone(),
                method: method.into(),
            }
            .into());
        }
        let tool_entry = if method == "tools/call" && enforce_tool_discovery {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let entry = self
                .tools
                .read()
                .await
                .values()
                .find(|entry| entry.raw_name == name)
                .cloned()
                .ok_or_else(|| McpError::InvalidParams {
                    server: self.config.name.clone(),
                    method: method.into(),
                    message: "tool was not discovered".into(),
                })?;
            crate::tools::validate_schema(
                &entry.input_schema,
                params.get("arguments").unwrap_or(&json!({})),
                "MCP tool input",
            )?;
            Some(entry)
        } else {
            None
        };
        let request = typed_request(&self.config.name, method, params)?;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(self.config.timeout_ms);
        let mut handle = tokio::select! {
            biased;
            _=cancel.cancelled()=>return Err(McpError::Cancelled{server:self.config.name.clone(),method:method.into()}.into()),
            _=connection.shutdown.cancelled()=>return Err(McpError::Disconnected{server:self.config.name.clone(),message:"shutting down".into()}.into()),
            _=tokio::time::sleep_until(deadline)=>return Err(McpError::Timeout{server:self.config.name.clone(),method:method.into(),timeout_ms:self.config.timeout_ms}.into()),
            response=connection.peer.send_cancellable_request(request,PeerRequestOptions::no_options())=>response.map_err(|e|service_error(&self.config.name,method,e))?,
        };
        let outcome = tokio::select! {
            biased;
            _=cancel.cancelled()=>Err(McpError::Cancelled{server:self.config.name.clone(),method:method.into()}),
            _=connection.shutdown.cancelled()=>Err(McpError::Disconnected{server:self.config.name.clone(),message:"shutting down".into()}),
            _=tokio::time::sleep_until(deadline)=>Err(McpError::Timeout{server:self.config.name.clone(),method:method.into(),timeout_ms:self.config.timeout_ms}),
            response=&mut handle.rx=>response.map_err(|_|McpError::Disconnected{server:self.config.name.clone(),message:"response channel closed".into()}).and_then(|response|response.map_err(|e|service_error(&self.config.name,method,e))),
        };
        let response = match outcome {
            Ok(response) => response,
            Err(error) => {
                let _ = tokio::time::timeout(
                    Duration::from_secs(1),
                    handle.cancel(Some(error.to_string())),
                )
                .await;
                return Err(error.into());
            }
        };
        if !matches_response(method, &response) {
            return Err(self.invalid_response(method, "unexpected SDK result variant"));
        }
        let value = serde_json::to_value(response)?;
        if method == "tools/call" && value.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(McpError::ToolFailure {
                server: self.config.name.clone(),
                method: method.into(),
                result: value,
            }
            .into());
        }
        if value["resultType"] != "task"
            && let Some(schema) = tool_entry.and_then(|entry| entry.output_schema)
        {
            crate::tools::validate_schema(
                &schema,
                value.get("structuredContent").unwrap_or(&Value::Null),
                "MCP tool output",
            )?;
        }
        Ok(value)
    }
    async fn resolve_task(&self, seed: Value, cancel: CancellationToken) -> Result<Value> {
        let task_id = seed["taskId"]
            .as_str()
            .ok_or_else(|| self.invalid_response("tools/call", "task result missing taskId"))?
            .to_owned();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(self.config.timeout_ms);
        let polling = async {
            let mut task = seed;
            loop {
                match task["status"].as_str().unwrap_or_default() {
                    "completed" => {
                        return task.get("result").cloned().ok_or_else(|| {
                            self.invalid_response("tasks/get", "completed task missing result")
                        });
                    }
                    "failed" => {
                        return Err(McpError::Remote {
                            server: self.config.name.clone(),
                            method: "tasks/get".into(),
                            code: task["error"]["code"].as_i64().unwrap_or(-32603) as i32,
                            message: task["error"]["message"]
                                .as_str()
                                .unwrap_or("task failed")
                                .into(),
                            data: task["error"].get("data").cloned(),
                        }
                        .into());
                    }
                    "cancelled" => {
                        return Err(McpError::Cancelled {
                            server: self.config.name.clone(),
                            method: "tasks/get".into(),
                        }
                        .into());
                    }
                    "input_required" => {
                        let hooks = self
                            .connection
                            .as_ref()
                            .and_then(|connection| connection.hooks.as_ref())
                            .ok_or_else(|| McpError::Unsupported {
                                server: self.config.name.clone(),
                                method: "tasks/update".into(),
                                message: "task requires host input hooks".into(),
                            })?;
                        let responses = hooks
                            .task_input(
                                task["inputRequests"].clone(),
                                McpCallbackContext {
                                    server: self.config.name.clone(),
                                    cancel: cancel.clone(),
                                },
                            )
                            .await
                            .map_err(|error| {
                                service_error(
                                    &self.config.name,
                                    "tasks/update",
                                    ServiceError::McpError(error),
                                )
                            })?;
                        self.request_inner(
                            "tasks/update",
                            json!({"taskId":task_id,"inputResponses":responses}),
                            cancel.clone(),
                            false,
                        )
                        .await?;
                    }
                    "working" => {}
                    _ => return Err(self.invalid_response("tasks/get", "unknown task state")),
                }
                let interval = task["pollIntervalMs"]
                    .as_u64()
                    .unwrap_or(100)
                    .clamp(10, 5000);
                tokio::time::sleep(Duration::from_millis(interval)).await;
                task = self
                    .request_inner(
                        "tasks/get",
                        json!({"taskId":task_id}),
                        cancel.clone(),
                        false,
                    )
                    .await?;
            }
        };
        let result = tokio::select! {
            _=cancel.cancelled()=>Err(McpError::Cancelled{server:self.config.name.clone(),method:"tasks/get".into()}.into()),
            result=tokio::time::timeout_at(deadline,polling)=>result.unwrap_or_else(|_|Err(McpError::Timeout{server:self.config.name.clone(),method:"tasks/get".into(),timeout_ms:self.config.timeout_ms}.into())),
        };
        if result.is_err() {
            let _ = tokio::time::timeout(
                Duration::from_secs(1),
                self.request_inner(
                    "tasks/cancel",
                    json!({"taskId":task_id}),
                    CancellationToken::new(),
                    false,
                ),
            )
            .await;
        }
        result
    }
    async fn close_connection(&self) -> Result<()> {
        if let Some(connection) = &self.connection {
            connection.shutdown.cancel();
            if let Some(mut running) = connection.service.lock().await.take() {
                let closed = running
                    .close_with_timeout(Duration::from_secs(5))
                    .await
                    .map_err(|e| McpError::Transport {
                        server: self.config.name.clone(),
                        method: "close".into(),
                        message: e.to_string(),
                    })?;
                if closed.is_none() {
                    return Err(McpError::Timeout {
                        server: self.config.name.clone(),
                        method: "close".into(),
                        timeout_ms: 5_000,
                    }
                    .into());
                }
            }
        }
        self.tools.write().await.clear();
        *self.state.write().await = json!({"name":self.config.name,"state":"closed"});
        Ok(())
    }
}

#[async_trait]
impl Object for McpServerObject {
    fn envelope(&self) -> ObjectEnvelope {
        let mut envelope = ObjectEnvelope::new(
            format!("mcp:{}", self.config.name),
            ObjectKind::McpServer,
            "rmcp MCP server connection",
        );
        envelope.capabilities.insert(
            "methods".into(),
            json!([
                "inspect",
                "tools/list",
                "tools/call",
                "resources/list",
                "resources/templates/list",
                "resources/read",
                "resources/subscribe",
                "resources/unsubscribe",
                "prompts/list",
                "prompts/get",
                "completion/complete",
                "tasks/get",
                "tasks/update",
                "tasks/cancel",
                "ping"
            ]),
        );
        envelope
    }
    async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        if method == "inspect" {
            return Ok(self.status().await);
        }
        self.request_inner(method, input, context.cancel, true)
            .await
    }
    async fn dispose(&self) -> Result<()> {
        self.close_connection().await
    }
}

#[derive(Clone, Default)]
pub struct McpRegistry {
    servers: BTreeMap<String, Arc<McpServerObject>>,
}
impl McpRegistry {
    pub async fn connect(configs: &[McpServerConfig]) -> Result<Self> {
        Self::connect_with_client(configs, None).await
    }
    pub async fn connect_with_client(
        configs: &[McpServerConfig],
        hooks: Option<Arc<dyn McpClientHooks>>,
    ) -> Result<Self> {
        Self::connect_with_policy(configs, hooks, Arc::new(NetworkPolicyConfig::default())).await
    }
    pub async fn connect_with_policy(
        configs: &[McpServerConfig],
        hooks: Option<Arc<dyn McpClientHooks>>,
        policy: Arc<dyn NetworkPolicy>,
    ) -> Result<Self> {
        let mut names = BTreeSet::new();
        for config in configs {
            if !names.insert(config.name.clone()) {
                return Err(McpError::Configuration {
                    server: config.name.clone(),
                    message: "duplicate server name".into(),
                }
                .into());
            }
        }
        let objects = futures_util::future::join_all(
            configs
                .iter()
                .cloned()
                .map(|config| McpServerObject::connect(config, hooks.clone(), policy.clone())),
        )
        .await;
        Ok(Self {
            servers: objects
                .into_iter()
                .map(|object| (object.config.name.clone(), Arc::new(object)))
                .collect(),
        })
    }
    pub async fn status(&self) -> Value {
        let mut servers = Vec::new();
        for server in self.servers.values() {
            servers.push(server.status().await);
        }
        json!({"servers":servers,"protocol_support":{"sdk":"rmcp 3.5.1","implemented":["initialize","stdio","streamable_http","tools/list","tools/call","tools/list_changed refresh","inputSchema and outputSchema validation","resources/list","resources/templates/list","resources/read","resources/subscribe","resources/unsubscribe","resources updated/list_changed notifications","prompts/list","prompts/get","prompts/list_changed notifications","completion/complete","client roots/list and list_changed","host callback sampling with token limit","host callback form elicitation with schema validation","SEP-2663 tasks/get/update/cancel and input_required polling","OAuth authorization-code PKCE loopback client with refresh/revoke and pluggable credential store","network host/address policy","ping","timeout","cancellation"],"not_implemented":["OAuth metadata discovery and dynamic client registration","automatic token refresh on already-connected HTTP transport","URL elicitation","modern subscriptions/listen","modern server/discover lifecycle","non-task MRTR input_required completion","automatic stdio reconnect","external JSON Schema refs","MCP conformance certification"],"conditional":"sampling/elicitation require explicit host hooks; roots/tasks require per-server configuration; unadvertised capabilities return explicit unsupported"}})
    }
    pub async fn tools(&self) -> Vec<Value> {
        let mut tools = Vec::new();
        for server in self.servers.values() {
            if let Some(connection) = &server.connection {
                if connection.shutdown.is_cancelled() || connection.peer.is_transport_closed() {
                    continue;
                }
                if connection.tools_changed.load(Ordering::Acquire)
                    && let Err(error) = server.refresh_tools(CancellationToken::new()).await
                {
                    connection.tools_changed.store(true, Ordering::Release);
                    *server.state.write().await = json!({"name":server.config.name,"state":"degraded","error":error_json(&error)});
                }
                tools.extend(
                    server
                        .tools
                        .read()
                        .await
                        .values()
                        .map(|entry| entry.schema.clone()),
                );
            }
        }
        tools
    }
    pub async fn call(
        &self,
        tool_name: &str,
        args: Value,
        cancel: CancellationToken,
    ) -> Result<Value> {
        for server in self.servers.values() {
            if let Some(entry) = server.tools.read().await.get(tool_name).cloned() {
                crate::tools::validate_schema(&entry.input_schema, &args, "MCP tool input")?;
                let result = server
                    .request_inner(
                        "tools/call",
                        json!({"name":entry.raw_name,"arguments":args}),
                        cancel.clone(),
                        true,
                    )
                    .await?;
                let result = if result["resultType"] == "task" {
                    server.resolve_task(result, cancel).await?
                } else {
                    result
                };
                if result["isError"] == true {
                    return Err(McpError::ToolFailure {
                        server: server.config.name.clone(),
                        method: "tools/call".into(),
                        result,
                    }
                    .into());
                }
                if let Some(schema) = entry.output_schema {
                    crate::tools::validate_schema(
                        &schema,
                        result.get("structuredContent").unwrap_or(&Value::Null),
                        "MCP tool output",
                    )?;
                }
                return Ok(result);
            }
        }
        Err(McpError::Unsupported {
            server: String::new(),
            method: "tools/call".into(),
            message: format!("unknown public tool {tool_name}"),
        }
        .into())
    }
    pub fn drain_notifications(&self, server: &str) -> Result<Vec<Value>> {
        let connection = self
            .servers
            .get(server)
            .and_then(|server| server.connection.as_ref())
            .ok_or_else(|| anyhow::anyhow!("MCP server not connected"))?;
        Ok(connection
            .client_state
            .events
            .lock()
            .map_err(|_| anyhow::anyhow!("MCP event queue poisoned"))?
            .drain(..)
            .collect())
    }
    pub async fn update_roots(&self, server: &str, roots: &[std::path::PathBuf]) -> Result<()> {
        let object = self
            .servers
            .get(server)
            .ok_or_else(|| anyhow::anyhow!("MCP server unknown"))?;
        if object.config.client.roots.is_empty() {
            anyhow::bail!("roots capability was not configured");
        }
        let connection = object
            .connection
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("MCP server disconnected"))?;
        let roots = roots
            .iter()
            .map(root_descriptor)
            .collect::<Result<Vec<_>>>()?;
        *connection
            .client_state
            .roots
            .write()
            .map_err(|_| anyhow::anyhow!("MCP roots poisoned"))? = roots;
        connection.peer.notify_roots_list_changed().await?;
        Ok(())
    }
    pub async fn request(
        &self,
        server: &str,
        method: &str,
        params: Value,
        cancel: CancellationToken,
    ) -> Result<Value> {
        self.servers
            .get(server)
            .ok_or_else(|| McpError::Disconnected {
                server: server.into(),
                message: "server not configured".into(),
            })?
            .request_inner(method, params, cancel, false)
            .await
    }
    pub async fn close(&self) -> Result<()> {
        let mut errors = Vec::new();
        for server in self.servers.values() {
            if let Err(error) = server.close_connection().await {
                errors.push(error.to_string());
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!("MCP shutdown failures: {}", errors.join("; "))
        }
    }
    pub fn objects(&self) -> Vec<Arc<dyn Object>> {
        self.servers
            .values()
            .cloned()
            .map(|object| object as Arc<dyn Object>)
            .collect()
    }
}

fn error_json(error: &anyhow::Error) -> Value {
    error
        .downcast_ref::<McpError>()
        .and_then(|error| serde_json::to_value(error).ok())
        .unwrap_or_else(|| json!({"message":error.to_string()}))
}
fn canonical_method(method: &str) -> &str {
    if method == "resource_templates/list" {
        "resources/templates/list"
    } else {
        method
    }
}
fn parse<T: DeserializeOwned>(
    server: &str,
    method: &str,
    params: Value,
) -> std::result::Result<T, McpError> {
    serde_json::from_value(params).map_err(|error| McpError::InvalidParams {
        server: server.into(),
        method: method.into(),
        message: error.to_string(),
    })
}
fn typed_request(
    server: &str,
    method: &str,
    params: Value,
) -> std::result::Result<ClientRequest, McpError> {
    let params = if params.is_null() { json!({}) } else { params };
    if !params.is_object() {
        return Err(McpError::InvalidParams {
            server: server.into(),
            method: method.into(),
            message: "params must be object".into(),
        });
    }
    Ok(match method {
        "tools/list" => ListToolsRequest::with_param(parse(server, method, params)?).into(),
        "tools/call" => CallToolRequest::new(parse(server, method, params)?).into(),
        "resources/list" => ListResourcesRequest::with_param(parse(server, method, params)?).into(),
        "resources/templates/list" => {
            ListResourceTemplatesRequest::with_param(parse(server, method, params)?).into()
        }
        "resources/read" => ReadResourceRequest::new(parse(server, method, params)?).into(),
        "resources/subscribe" => SubscribeRequest::new(parse(server, method, params)?).into(),
        "resources/unsubscribe" => UnsubscribeRequest::new(parse(server, method, params)?).into(),
        "completion/complete" => CompleteRequest::new(parse(server, method, params)?).into(),
        "tasks/get" => GetTaskRequest::new(parse(server, method, params)?).into(),
        "tasks/update" => UpdateTaskRequest::new(parse(server, method, params)?).into(),
        "tasks/cancel" => CancelTaskRequest::new(parse(server, method, params)?).into(),
        "prompts/list" => ListPromptsRequest::with_param(parse(server, method, params)?).into(),
        "prompts/get" => GetPromptRequest::new(parse(server, method, params)?).into(),
        "ping" => PingRequest::default().into(),
        _ => {
            return Err(McpError::Unsupported {
                server: server.into(),
                method: method.into(),
                message: "method not implemented".into(),
            });
        }
    })
}
fn matches_response(method: &str, result: &ServerResult) -> bool {
    matches!(
        (method, result),
        ("tools/list", ServerResult::ListToolsResult(_))
            | ("tools/call", ServerResult::CallToolResult(_))
            | ("resources/list", ServerResult::ListResourcesResult(_))
            | (
                "resources/templates/list",
                ServerResult::ListResourceTemplatesResult(_)
            )
            | ("resources/read", ServerResult::ReadResourceResult(_))
            | ("prompts/list", ServerResult::ListPromptsResult(_))
            | ("prompts/get", ServerResult::GetPromptResult(_))
            | ("ping", ServerResult::EmptyResult(_))
            | (
                "resources/subscribe" | "resources/unsubscribe",
                ServerResult::EmptyResult(_)
            )
            | ("completion/complete", ServerResult::CompleteResult(_))
            | ("tasks/get", ServerResult::GetTaskResult(_))
            | (
                "tasks/update" | "tasks/cancel",
                ServerResult::TaskAckResult(_)
            )
            | (
                "tasks/update" | "tasks/cancel",
                ServerResult::EmptyResult(_)
            )
            | ("tools/call", ServerResult::CreateTaskResult(_))
    )
}
fn public_tool_name(server: &str, raw: &str) -> std::result::Result<String, McpError> {
    let normalized: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = format!("mcp__{server}__{normalized}");
    if raw.is_empty() || name.len() > 64 {
        return Err(McpError::InvalidResponse {
            server: server.into(),
            method: "tools/list".into(),
            message: format!("tool name cannot fit model limit: {raw}"),
        });
    }
    Ok(name)
}
fn root_descriptor(path: &std::path::PathBuf) -> Result<Root> {
    let root = std::fs::canonicalize(path)?;
    if !root.is_dir() {
        anyhow::bail!("MCP root must be a directory");
    }
    let uri = reqwest::Url::from_directory_path(&root)
        .map_err(|_| anyhow::anyhow!("invalid root URI"))?;
    Ok(
        Root::new(uri.to_string())
            .with_name(root.file_name().unwrap_or_default().to_string_lossy()),
    )
}
async fn connect_service(
    config: &McpServerConfig,
    hooks: Option<Arc<dyn McpClientHooks>>,
    policy: Arc<dyn NetworkPolicy>,
) -> std::result::Result<Connection, McpError> {
    let fail = |message: String| McpError::Configuration {
        server: config.name.clone(),
        message,
    };
    if config.name.is_empty()
        || config.name.len() > 32
        || config.name.contains("__")
        || !config
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(fail(
            "name must contain 1..32 ASCII letters, digits, '_' or '-'; '__' is reserved for tool namespaces".into(),
        ));
    }
    if config.timeout_ms == 0 || config.startup_timeout_ms == 0 || config.max_discovery_pages == 0 {
        return Err(fail(
            "timeouts and discovery page limit must be positive".into(),
        ));
    }
    if config.command.is_some() == config.url.is_some() {
        return Err(fail("configure exactly one of command or url".into()));
    }
    let shutdown = CancellationToken::new();
    let tools_changed = Arc::new(AtomicBool::new(false));
    if (config.client.enable_sampling || config.client.enable_elicitation) && hooks.is_none() {
        return Err(fail(
            "sampling/elicitation require configured host hooks".into(),
        ));
    }
    let client_state = Arc::new(ClientState::default());
    *client_state.roots.write().expect("MCP roots poisoned") = config
        .client
        .roots
        .iter()
        .map(root_descriptor)
        .collect::<Result<Vec<_>>>()
        .map_err(|error| fail(error.to_string()))?;
    let token_hook = hooks.clone();
    let handler = HarnessClient {
        tools_changed: tools_changed.clone(),
        state: client_state.clone(),
        server: config.name.clone(),
        options: config.client.clone(),
        hooks,
        timeout_ms: config.timeout_ms,
    };
    let duration = Duration::from_millis(config.startup_timeout_ms);
    let service = if let Some(command) = &config.command {
        let mut child = tokio::process::Command::new(command);
        child.args(&config.args).kill_on_drop(true);
        #[cfg(windows)]
        child.creation_flags(0x0800_0000); // CREATE_NO_WINDOW for background MCP servers.
        if let Some(cwd) = &config.cwd {
            child.current_dir(cwd);
        }
        if config.scrub_env {
            for (name, _) in std::env::vars_os() {
                let upper = name.to_string_lossy().to_ascii_uppercase();
                if ["KEY", "PASSWORD", "SECRET", "TOKEN"]
                    .iter()
                    .any(|pattern| upper.contains(pattern))
                {
                    child.env_remove(name);
                }
            }
        }
        child.envs(&config.env);
        let transport = TokioChildProcess::new(child)
            .map_err(|error| fail(format!("spawn failed: {error}")))?;
        tokio::time::timeout(
            duration,
            serve_client_with_ct(handler, transport, shutdown.clone()),
        )
        .await
    } else {
        let uri = config.url.as_deref().unwrap_or_default();
        let url =
            reqwest::Url::parse(uri).map_err(|error| fail(format!("invalid URL: {error}")))?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(fail(
                "HTTP URL must use http/https and no credentials".into(),
            ));
        }
        policy
            .authorize_url(&url)
            .await
            .map_err(|error| fail(error.to_string()))?;
        let mut transport_config = StreamableHttpClientTransportConfig::with_uri(uri.to_owned());
        if let Some(variable) = &config.bearer_token_env {
            let token = std::env::var(variable).map_err(|_| {
                fail(format!(
                    "bearer token environment variable {variable} unavailable"
                ))
            })?;
            if token.trim().is_empty() {
                return Err(fail(format!(
                    "bearer token environment variable {variable} empty"
                )));
            }
            transport_config = transport_config.auth_header(token);
        } else if let Some(hooks) = &token_hook
            && let Some(token) = hooks
                .bearer_token(&config.name)
                .await
                .map_err(|error| fail(error.to_string()))?
        {
            if token.trim().is_empty() {
                return Err(fail("credential hook returned empty bearer token".into()));
            }
            transport_config = transport_config.auth_header(token);
        }
        let http_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| fail(format!("HTTP client initialization failed: {error}")))?;
        let transport = StreamableHttpClientTransport::with_client(http_client, transport_config);
        tokio::time::timeout(
            duration,
            serve_client_with_ct(handler, transport, shutdown.clone()),
        )
        .await
    };
    let service = match service {
        Err(_) => {
            shutdown.cancel();
            return Err(McpError::Timeout {
                server: config.name.clone(),
                method: "initialize".into(),
                timeout_ms: config.startup_timeout_ms,
            });
        }
        Ok(Err(error)) => {
            shutdown.cancel();
            return Err(McpError::Transport {
                server: config.name.clone(),
                method: "initialize".into(),
                message: error.to_string(),
            });
        }
        Ok(Ok(service)) => service,
    };
    let peer = service.peer().clone();
    let info = peer
        .peer_info()
        .and_then(|info| serde_json::to_value(&*info).ok())
        .unwrap_or(Value::Null);
    Ok(Connection {
        peer,
        service: Mutex::new(Some(service)),
        shutdown,
        info,
        tools_changed,
        client_state,
        hooks: token_hook,
    })
}
