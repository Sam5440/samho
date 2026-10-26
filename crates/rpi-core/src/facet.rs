//! Authenticated remote Facet client.
//!
//! This module is intentionally independent of Runtime/config. The host wires a
//! `RemoteFacetConfig` into its configuration and registers the connected
//! `RemoteFacet` as an Object. Every call has an explicit trace context and a
//! bounded response; no unauthenticated "convenience" request is exposed.
use crate::object::{InvocationContext, Object, ObjectEnvelope, ObjectKind};
use crate::tools::NetworkPolicy;
use anyhow::Result;
use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fmt, sync::Arc};
use tokio::time::{Duration, timeout};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const FACET_PROTOCOL_VERSION: &str = "rpi-facet/1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteFacetConfig {
    pub name: String,
    pub endpoint: String,
    pub auth_token_env: String,
    pub timeout_ms: u64,
    pub startup_timeout_ms: u64,
    pub max_response_bytes: usize,
    pub allowed_capabilities: Vec<String>,
    pub object_id: String,
    pub allow_insecure_loopback: bool,
}
impl Default for RemoteFacetConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            endpoint: String::new(),
            auth_token_env: String::new(),
            timeout_ms: 30_000,
            startup_timeout_ms: 10_000,
            max_response_bytes: 4 * 1024 * 1024,
            allowed_capabilities: Vec::new(),
            object_id: String::new(),
            allow_insecure_loopback: false,
        }
    }
}
pub type FacetConfig = RemoteFacetConfig;
impl RemoteFacetConfig {
    pub fn validate(&self) -> FacetResult<()> {
        validate_config(self)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FacetErrorKind {
    Configuration,
    Authentication,
    Protocol,
    Network,
    NetworkDenied,
    Timeout,
    Cancelled,
    Disconnected,
    ResponseLimit,
    CapabilityDenied,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FacetError {
    pub kind: FacetErrorKind,
    pub message: String,
    pub status_code: Option<u16>,
    pub call_id: Option<String>,
}
impl FacetError {
    fn new(kind: FacetErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            status_code: None,
            call_id: None,
        }
    }
}
impl fmt::Display for FacetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "facet {:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for FacetError {}
pub type FacetResult<T> = std::result::Result<T, FacetError>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FacetHandshake {
    pub protocol: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub metadata: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct FacetCallRequest {
    protocol: String,
    call_id: String,
    method: String,
    input: Value,
    trace_id: String,
    session_id: String,
    span_id: String,
    parent_span_id: Option<String>,
}

#[derive(Clone)]
pub struct RemoteFacet {
    config: RemoteFacetConfig,
    client: Client,
    base: Url,
    token: Arc<String>,
    handshake: Arc<FacetHandshake>,
    network: Arc<dyn NetworkPolicy>,
    lifetime: CancellationToken,
}
impl fmt::Debug for RemoteFacet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteFacet")
            .field("name", &self.config.name)
            .field("protocol", &self.handshake.protocol)
            .finish_non_exhaustive()
    }
}
impl RemoteFacet {
    /// Connects and performs a versioned handshake before returning an active Facet.
    pub async fn connect(
        config: &FacetConfig,
        network: Arc<dyn NetworkPolicy>,
    ) -> FacetResult<Self> {
        validate_config(config)?;
        let token = std::env::var(&config.auth_token_env).map_err(|_| {
            FacetError::new(
                FacetErrorKind::Authentication,
                format!(
                    "facet auth environment variable {} is not set",
                    config.auth_token_env
                ),
            )
        })?;
        Self::connect_with_token(config.clone(), token, CancellationToken::new(), network).await
    }
    /// Explicit credential injection for embedding and deterministic fixtures; the token is never serialized.
    pub async fn connect_with_token(
        config: RemoteFacetConfig,
        token: String,
        cancel: CancellationToken,
        network: Arc<dyn NetworkPolicy>,
    ) -> FacetResult<Self> {
        validate_config(&config)?;
        let base = Url::parse(&config.endpoint).map_err(|_| {
            FacetError::new(FacetErrorKind::Configuration, "invalid facet endpoint")
        })?;
        if token.trim().is_empty() {
            return Err(FacetError::new(
                FacetErrorKind::Authentication,
                "facet auth token is empty",
            ));
        }
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| FacetError::new(FacetErrorKind::Configuration, error.to_string()))?;
        let operation = async {
            let url = join(&base, "handshake");
            network.authorize_url(&url).await.map_err(|error| {
                FacetError::new(FacetErrorKind::NetworkDenied, error.to_string())
            })?;
            let request = client
                .post(url)
                .bearer_auth(token.clone())
                .json(&json!({"protocol":FACET_PROTOCOL_VERSION,"name":config.name}));
            let response = request
                .send()
                .await
                .map_err(|error| FacetError::new(FacetErrorKind::Network, error.to_string()))?;
            decode_handshake(response, config.max_response_bytes).await
        };
        let handshake = tokio::select! { biased;
            _ = cancel.cancelled() => return Err(FacetError::new(FacetErrorKind::Cancelled, "facet handshake cancelled")),
            result = timeout(Duration::from_millis(config.startup_timeout_ms), operation) => result.map_err(|_| FacetError::new(FacetErrorKind::Timeout, "facet handshake timeout"))??,
        };
        if handshake.protocol != FACET_PROTOCOL_VERSION {
            return Err(FacetError::new(
                FacetErrorKind::Protocol,
                format!(
                    "unsupported facet protocol {}; expected {FACET_PROTOCOL_VERSION}",
                    handshake.protocol
                ),
            ));
        }
        if handshake.name != config.name {
            return Err(FacetError::new(
                FacetErrorKind::Protocol,
                "facet handshake name does not match configuration",
            ));
        }
        for capability in &handshake.capabilities {
            if !config.allowed_capabilities.is_empty()
                && !config
                    .allowed_capabilities
                    .iter()
                    .any(|allowed| allowed == capability)
            {
                return Err(FacetError::new(
                    FacetErrorKind::CapabilityDenied,
                    format!("facet advertised capability {capability} outside allowlist"),
                ));
            }
        }
        if handshake.capabilities.is_empty() {
            return Err(FacetError::new(
                FacetErrorKind::Protocol,
                "facet did not advertise any capabilities",
            ));
        }
        Ok(Self {
            config,
            client,
            base,
            token: Arc::new(token),
            handshake: Arc::new(handshake),
            network,
            lifetime: CancellationToken::new(),
        })
    }
    pub fn config(&self) -> &RemoteFacetConfig {
        &self.config
    }
    pub async fn handshake(&self) -> FacetHandshake {
        self.handshake.as_ref().clone()
    }
    pub fn capabilities(&self) -> Vec<String> {
        self.handshake.capabilities.clone()
    }

    pub async fn call(
        &self,
        method: &str,
        input: Value,
        context: &InvocationContext,
    ) -> FacetResult<Value> {
        if context.cancel.is_cancelled() {
            return Err(FacetError::new(
                FacetErrorKind::Cancelled,
                "facet call cancelled before dispatch",
            ));
        }
        if self.lifetime.is_cancelled() {
            return Err(FacetError::new(
                FacetErrorKind::Disconnected,
                "facet has been disposed",
            ));
        }
        if method.trim().is_empty() {
            return Err(FacetError::new(
                FacetErrorKind::Configuration,
                "facet method must not be empty",
            ));
        }
        let handshake = self.handshake().await;
        if !self
            .config
            .allowed_capabilities
            .iter()
            .any(|capability| capability == method)
            || !handshake
                .capabilities
                .iter()
                .any(|capability| capability == method)
        {
            return Err(FacetError::new(
                FacetErrorKind::CapabilityDenied,
                format!("facet capability {method} is not allowed and advertised"),
            ));
        }
        if context.trace_id.is_empty()
            || context.session_id.is_empty()
            || context.span_id.is_empty()
        {
            return Err(FacetError::new(
                FacetErrorKind::Configuration,
                "remote facet calls require trace, session and parent span identity",
            ));
        }
        let call_id = Uuid::new_v4().to_string();
        let span_id = Uuid::new_v4().simple().to_string()[..16].to_owned();
        let request = FacetCallRequest {
            protocol: FACET_PROTOCOL_VERSION.into(),
            call_id: call_id.clone(),
            method: method.into(),
            input,
            trace_id: context.trace_id.clone(),
            session_id: context.session_id.clone(),
            span_id: span_id.clone(),
            parent_span_id: Some(context.span_id.clone()),
        };
        let operation = async {
            let url = join(&self.base, "invoke");
            self.network.authorize_url(&url).await.map_err(|error| {
                FacetError::new(FacetErrorKind::NetworkDenied, error.to_string())
            })?;
            let mut request = self
                .client
                .post(url)
                .bearer_auth(self.token.as_str())
                .json(&request);
            let trace_hex = context.trace_id.replace('-', "");
            if trace_hex.len() == 32 && trace_hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                request = request.header("traceparent", format!("00-{trace_hex}-{span_id}-01"));
            }
            let response = request
                .send()
                .await
                .map_err(|error| FacetError::new(FacetErrorKind::Network, error.to_string()))?;
            if response.status() == StatusCode::UNAUTHORIZED
                || response.status() == StatusCode::FORBIDDEN
            {
                return Err(FacetError {
                    kind: FacetErrorKind::Authentication,
                    message: "facet rejected bearer token".into(),
                    status_code: Some(response.status().as_u16()),
                    call_id: Some(call_id.clone()),
                });
            }
            if !response.status().is_success() {
                return Err(FacetError {
                    kind: FacetErrorKind::Protocol,
                    message: format!("facet HTTP {}", response.status()),
                    status_code: Some(response.status().as_u16()),
                    call_id: Some(call_id.clone()),
                });
            }
            read_json(response, self.config.max_response_bytes).await
        };
        let value = tokio::select! {
            biased;
            _ = self.lifetime.cancelled() => { self.send_cancel(&call_id).await; return Err(FacetError::new(FacetErrorKind::Disconnected, "facet disposed during active call")); }
            _ = context.cancel.cancelled() => { self.send_cancel(&call_id).await; return Err(FacetError { kind:FacetErrorKind::Cancelled, message:"facet call cancelled".into(), status_code:None, call_id:Some(call_id) }); }
            result = timeout(Duration::from_millis(self.config.timeout_ms), operation) => match result { Ok(result) => result?, Err(_) => { self.send_cancel(&call_id).await; return Err(FacetError { kind:FacetErrorKind::Timeout, message:"facet call timeout".into(), status_code:None, call_id:Some(call_id.clone()) }); } },
        };
        if value.get("protocol").and_then(Value::as_str) != Some(FACET_PROTOCOL_VERSION) {
            return Err(FacetError::new(
                FacetErrorKind::Protocol,
                "facet invoke protocol version mismatch",
            ));
        }
        if value.get("call_id").and_then(Value::as_str) != Some(call_id.as_str()) {
            return Err(FacetError {
                kind: FacetErrorKind::Protocol,
                message: "facet response call_id mismatch".into(),
                status_code: None,
                call_id: Some(call_id),
            });
        }
        if let Some(error) = value.get("error") {
            return Err(FacetError {
                kind: FacetErrorKind::Protocol,
                message: error.to_string(),
                status_code: None,
                call_id: Some(call_id),
            });
        }
        value.get("result").cloned().ok_or_else(|| FacetError {
            kind: FacetErrorKind::Protocol,
            message: "facet response has no result".into(),
            status_code: None,
            call_id: Some(call_id),
        })
    }
    async fn send_cancel(&self, call_id: &str) {
        let operation = async {
            let url = join(&self.base, "cancel");
            self.network.authorize_url(&url).await.ok()?;
            self.client
                .post(url)
                .bearer_auth(self.token.as_str())
                .json(&json!({"protocol":FACET_PROTOCOL_VERSION,"call_id":call_id}))
                .send()
                .await
                .ok()
        };
        let _ = timeout(
            Duration::from_millis(self.config.timeout_ms.min(250)),
            operation,
        )
        .await;
    }
}
#[async_trait::async_trait]
impl Object for RemoteFacet {
    fn envelope(&self) -> ObjectEnvelope {
        let mut envelope = ObjectEnvelope::new(
            if self.config.object_id.is_empty() {
                format!("facet:{}", self.config.name)
            } else {
                self.config.object_id.clone()
            },
            ObjectKind::Plugin,
            format!("Remote Facet {}", self.config.name),
        );
        envelope.version = 1;
        envelope
            .labels
            .insert("protocol".into(), FACET_PROTOCOL_VERSION.into());
        envelope.spec = serde_json::to_value(&self.config).unwrap_or(Value::Null);
        for capability in self.capabilities() {
            envelope
                .capabilities
                .insert(capability, json!({"remote":true}));
        }
        envelope
    }
    async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        self.call(method, input, &context).await.map_err(Into::into)
    }
    async fn dispose(&self) -> Result<()> {
        self.lifetime.cancel();
        Ok(())
    }
}

fn validate_config(config: &RemoteFacetConfig) -> FacetResult<()> {
    if config.name.trim().is_empty() || config.name.len() > 128 {
        return Err(FacetError::new(
            FacetErrorKind::Configuration,
            "facet name must be 1..=128 characters",
        ));
    }
    if config.auth_token_env.trim().is_empty() {
        return Err(FacetError::new(
            FacetErrorKind::Configuration,
            "facet auth_token_env is required",
        ));
    }
    if config.allowed_capabilities.is_empty()
        || config
            .allowed_capabilities
            .iter()
            .any(|name| name.trim().is_empty())
    {
        return Err(FacetError::new(
            FacetErrorKind::Configuration,
            "facet requires an explicit nonempty capability allowlist",
        ));
    }
    if config.timeout_ms == 0 || config.startup_timeout_ms == 0 || config.max_response_bytes == 0 {
        return Err(FacetError::new(
            FacetErrorKind::Configuration,
            "facet timeouts and response limit must be positive",
        ));
    }
    let url = Url::parse(&config.endpoint)
        .map_err(|_| FacetError::new(FacetErrorKind::Configuration, "invalid facet endpoint"))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        return Err(FacetError::new(
            FacetErrorKind::Configuration,
            "facet endpoint cannot embed credentials, query or fragment",
        ));
    }
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if url.scheme() != "https"
        && !(config.allow_insecure_loopback && loopback && url.scheme() == "http")
    {
        return Err(FacetError::new(
            FacetErrorKind::Configuration,
            "facet endpoint must use HTTPS; HTTP is allowed only for explicit loopback fixtures",
        ));
    }
    Ok(())
}
fn join(base: &Url, path: &str) -> Url {
    let mut url = base.clone();
    let prefix = url.path().trim_end_matches('/');
    url.set_path(&format!("{prefix}/{path}"));
    url
}
fn network_error(error: reqwest::Error) -> FacetError {
    FacetError::new(
        if error.is_timeout() {
            FacetErrorKind::Timeout
        } else {
            FacetErrorKind::Network
        },
        error.without_url().to_string(),
    )
}
async fn read_json(response: reqwest::Response, max: usize) -> FacetResult<Value> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(network_error)?;
        if bytes.len().saturating_add(chunk.len()) > max {
            return Err(FacetError::new(
                FacetErrorKind::ResponseLimit,
                "facet response exceeds configured limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        FacetError::new(
            FacetErrorKind::Protocol,
            format!("invalid facet JSON: {error}"),
        )
    })
}
async fn decode_handshake(response: reqwest::Response, max: usize) -> FacetResult<FacetHandshake> {
    if response.status() == StatusCode::UNAUTHORIZED || response.status() == StatusCode::FORBIDDEN {
        return Err(FacetError::new(
            FacetErrorKind::Authentication,
            "facet handshake rejected bearer token",
        ));
    }
    if !response.status().is_success() {
        return Err(FacetError::new(
            FacetErrorKind::Protocol,
            format!("facet handshake HTTP {}", response.status()),
        ));
    }
    let value = read_json(response, max).await?;
    serde_json::from_value(value).map_err(|error| {
        FacetError::new(
            FacetErrorKind::Protocol,
            format!("invalid facet handshake: {error}"),
        )
    })
}
