//! OAuth authorization-code + PKCE for public local clients. No browser is launched automatically.
use crate::tools::{NetworkPolicy, NetworkPolicyConfig};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthConfig {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub revocation_endpoint: Option<String>,
    pub client_id: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub resource: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Optional loopback callback range. Test fixtures set this to `(1301, 1399)`
    /// to comply with the repository port policy; production clients may leave
    /// it unset to request an OS-assigned callback port.
    #[serde(default)]
    pub callback_port_range: Option<(u16, u16)>,
}
fn default_timeout() -> u64 {
    120_000
}

#[derive(Clone)]
pub struct OAuthTokens {
    access_token: String,
    refresh_token: Option<String>,
    expires_at_ms: Option<u64>,
    scope: Option<String>,
}
impl std::fmt::Debug for OAuthTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthTokens")
            .field("access_token", &"[REDACTED]")
            .field("has_refresh_token", &self.refresh_token.is_some())
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}
impl OAuthTokens {
    pub fn bearer(&self) -> &str {
        &self.access_token
    }
    pub fn summary(&self) -> OAuthTokenSummary {
        OAuthTokenSummary {
            has_access_token: !self.access_token.is_empty(),
            has_refresh_token: self.refresh_token.is_some(),
            expires_at_ms: self.expires_at_ms,
            scope: self.scope.clone(),
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct OAuthTokenSummary {
    pub has_access_token: bool,
    pub has_refresh_token: bool,
    pub expires_at_ms: Option<u64>,
    pub scope: Option<String>,
}

/// Production embedders can implement an OS keychain/encrypted vault. The default is memory-only.
#[async_trait]
pub trait OAuthCredentialStore: Send + Sync {
    async fn load(&self, key: &str) -> Result<Option<OAuthTokens>>;
    async fn save(&self, key: &str, tokens: OAuthTokens) -> Result<()>;
    async fn remove(&self, key: &str) -> Result<()>;
}
#[derive(Default)]
pub struct MemoryCredentialStore {
    tokens: Mutex<BTreeMap<String, OAuthTokens>>,
}
#[async_trait]
impl OAuthCredentialStore for MemoryCredentialStore {
    async fn load(&self, key: &str) -> Result<Option<OAuthTokens>> {
        Ok(self.tokens.lock().await.get(key).cloned())
    }
    async fn save(&self, key: &str, tokens: OAuthTokens) -> Result<()> {
        self.tokens.lock().await.insert(key.into(), tokens);
        Ok(())
    }
    async fn remove(&self, key: &str) -> Result<()> {
        self.tokens.lock().await.remove(key);
        Ok(())
    }
}

pub struct AuthorizationSession {
    pub authorization_url: String,
    pub redirect_uri: String,
    listener: TcpListener,
    verifier: String,
    state: String,
    deadline: tokio::time::Instant,
}
impl std::fmt::Debug for AuthorizationSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizationSession")
            .field("redirect_uri", &self.redirect_uri)
            .field("verifier", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
pub struct OAuthClient {
    config: OAuthConfig,
    store: Arc<dyn OAuthCredentialStore>,
    http: reqwest::Client,
    policy: Arc<dyn NetworkPolicy>,
    operation: Mutex<()>,
}
impl OAuthClient {
    pub fn new(config: OAuthConfig, store: Arc<dyn OAuthCredentialStore>) -> Result<Self> {
        Self::with_policy(config, store, Arc::new(NetworkPolicyConfig::default()))
    }
    pub fn with_policy(
        config: OAuthConfig,
        store: Arc<dyn OAuthCredentialStore>,
        policy: Arc<dyn NetworkPolicy>,
    ) -> Result<Self> {
        if config.client_id.trim().is_empty() || config.timeout_ms == 0 {
            bail!("OAuth client id and positive timeout required");
        }
        for endpoint in [&config.authorization_endpoint, &config.token_endpoint] {
            parse_endpoint(endpoint)?;
        }
        if let Some(endpoint) = &config.revocation_endpoint {
            parse_endpoint(endpoint)?;
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()?;
        Ok(Self {
            config,
            store,
            http,
            policy,
            operation: Mutex::new(()),
        })
    }
    pub async fn begin_authorization(&self) -> Result<AuthorizationSession> {
        let mut url = parse_endpoint(&self.config.authorization_endpoint)?;
        if url.query_pairs().any(|(name, _)| {
            matches!(
                name.as_ref(),
                "response_type"
                    | "client_id"
                    | "redirect_uri"
                    | "state"
                    | "code_challenge"
                    | "code_challenge_method"
                    | "scope"
                    | "resource"
            )
        }) {
            bail!("OAuth authorization endpoint contains reserved query parameters");
        }
        self.policy.authorize_url(&url).await?;
        let listener = if let Some((first, last)) = self.config.callback_port_range {
            bind_loopback_range(first, last).await?
        } else {
            TcpListener::bind("127.0.0.1:0").await?
        };
        let redirect_uri = format!("http://{}/oauth/callback", listener.local_addr()?);
        let verifier = random_token();
        let state = random_token();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("response_type", "code")
                .append_pair("client_id", &self.config.client_id)
                .append_pair("redirect_uri", &redirect_uri)
                .append_pair("state", &state)
                .append_pair("code_challenge", &challenge)
                .append_pair("code_challenge_method", "S256");
            if !self.config.scopes.is_empty() {
                query.append_pair("scope", &self.config.scopes.join(" "));
            }
            if let Some(resource) = &self.config.resource {
                query.append_pair("resource", resource);
            }
        }
        Ok(AuthorizationSession {
            authorization_url: url.to_string(),
            redirect_uri,
            listener,
            verifier,
            state,
            deadline: tokio::time::Instant::now() + Duration::from_millis(self.config.timeout_ms),
        })
    }
    pub async fn complete_authorization(
        &self,
        key: &str,
        session: AuthorizationSession,
        cancel: CancellationToken,
    ) -> Result<OAuthTokenSummary> {
        let _guard = self.operation.lock().await;
        let code = tokio::select! {
            _=cancel.cancelled()=>bail!("OAuth authorization cancelled"),
            code=tokio::time::timeout_at(session.deadline,receive_callback(&session))=>code.context("OAuth authorization timed out")??,
        };
        let mut form = vec![
            ("grant_type", "authorization_code".to_owned()),
            ("code", code),
            ("redirect_uri", session.redirect_uri.clone()),
            ("client_id", self.config.client_id.clone()),
            ("code_verifier", session.verifier.clone()),
        ];
        if let Some(resource) = &self.config.resource {
            form.push(("resource", resource.clone()));
        }
        let tokens = self.exchange(&form, cancel).await?;
        let summary = tokens.summary();
        self.store.save(key, tokens).await?;
        Ok(summary)
    }
    pub async fn refresh(&self, key: &str, cancel: CancellationToken) -> Result<OAuthTokenSummary> {
        let _guard = self.operation.lock().await;
        let previous = self
            .store
            .load(key)
            .await?
            .ok_or_else(|| anyhow::anyhow!("OAuth credential not found"))?;
        let refresh = previous
            .refresh_token
            .clone()
            .ok_or_else(|| anyhow::anyhow!("OAuth server did not issue refresh token"))?;
        let mut form = vec![
            ("grant_type", "refresh_token".to_owned()),
            ("refresh_token", refresh.clone()),
            ("client_id", self.config.client_id.clone()),
        ];
        if let Some(resource) = &self.config.resource {
            form.push(("resource", resource.clone()));
        }
        let mut tokens = self.exchange(&form, cancel).await?;
        if tokens.refresh_token.is_none() {
            tokens.refresh_token = Some(refresh);
        }
        let summary = tokens.summary();
        self.store.save(key, tokens).await?;
        Ok(summary)
    }
    pub async fn revoke(&self, key: &str, cancel: CancellationToken) -> Result<()> {
        let _guard = self.operation.lock().await;
        let tokens = self
            .store
            .load(key)
            .await?
            .ok_or_else(|| anyhow::anyhow!("OAuth credential not found"))?;
        let endpoint = self
            .config
            .revocation_endpoint
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("OAuth revocation endpoint not configured"))?;
        let url = parse_endpoint(endpoint)?;
        self.policy.authorize_url(&url).await?;
        let hint = if tokens.refresh_token.is_some() {
            "refresh_token"
        } else {
            "access_token"
        };
        let mut form = vec![
            ("token", tokens.refresh_token.unwrap_or(tokens.access_token)),
            ("client_id", self.config.client_id.clone()),
        ];
        form.push(("token_type_hint", hint.into()));
        let response = tokio::select! { _=cancel.cancelled()=>bail!("OAuth revocation cancelled"),response=self.http.post(url).form(&form).send()=>response? };
        if !response.status().is_success() {
            bail!(
                "OAuth revocation rejected (HTTP {})",
                response.status().as_u16()
            );
        }
        self.store.remove(key).await?;
        Ok(())
    }
    pub async fn credential(&self, key: &str) -> Result<Option<OAuthTokens>> {
        self.store.load(key).await
    }
    async fn exchange(
        &self,
        form: &[(&str, String)],
        cancel: CancellationToken,
    ) -> Result<OAuthTokens> {
        let url = parse_endpoint(&self.config.token_endpoint)?;
        self.policy.authorize_url(&url).await?;
        let response = tokio::select! { _=cancel.cancelled()=>bail!("OAuth token request cancelled"),response=self.http.post(url).form(form).send()=>response? };
        if !response.status().is_success() {
            bail!(
                "OAuth token request rejected (HTTP {})",
                response.status().as_u16()
            );
        }
        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            token_type: String,
            refresh_token: Option<String>,
            expires_in: Option<u64>,
            scope: Option<String>,
        }
        let response: TokenResponse = response
            .json()
            .await
            .context("OAuth token response invalid")?;
        if response.access_token.trim().is_empty()
            || !response.token_type.eq_ignore_ascii_case("Bearer")
            || response
                .refresh_token
                .as_ref()
                .is_some_and(|token| token.trim().is_empty())
        {
            bail!("OAuth token response has empty credential or unsupported type");
        }
        Ok(OAuthTokens {
            access_token: response.access_token,
            refresh_token: response.refresh_token,
            expires_at_ms: response
                .expires_in
                .map(|seconds| now_ms().saturating_add(seconds.saturating_mul(1000))),
            scope: response.scope,
        })
    }
}

async fn bind_loopback_range(first: u16, last: u16) -> Result<TcpListener> {
    if !(1300..=1400).contains(&first) || !(1300..=1400).contains(&last) || first > last {
        bail!("OAuth callback port range must be within 1300-1400");
    }
    for port in first..=last {
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => return Ok(listener),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(error) => return Err(error.into()),
        }
    }
    bail!("no OAuth callback port available in {first}-{last}")
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
fn parse_endpoint(value: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(value)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        bail!("OAuth endpoint requires HTTP(S) without credentials or fragment");
    }
    Ok(url)
}
fn state_matches(expected: &str, actual: &str) -> bool {
    let mut diff = expected.len() ^ actual.len();
    for (i, byte) in expected.bytes().enumerate() {
        diff |= usize::from(byte ^ actual.as_bytes().get(i).copied().unwrap_or(0));
    }
    diff == 0
}
async fn receive_callback(session: &AuthorizationSession) -> Result<String> {
    loop {
        let (mut stream, remote) = session.listener.accept().await?;
        if !remote.ip().is_loopback() {
            continue;
        }
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 1024];
        loop {
            let count = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
                .await
                .context("OAuth callback read timed out")??;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            if bytes.len() > 8192 {
                bail!("OAuth callback exceeds header limit");
            }
            if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8(bytes).context("OAuth callback is not UTF-8")?;
        let expected_host = reqwest::Url::parse(&session.redirect_uri)?
            .authority()
            .to_string();
        let host = request.lines().find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("host"))
                .map(|(_, value)| value.trim())
        });
        let parts = request
            .lines()
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>();
        let parsed = parts
            .get(1)
            .and_then(|path| reqwest::Url::parse(&format!("http://{expected_host}{path}")).ok());
        let mut status = "400 Bad Request";
        let mut code = None;
        let mut terminal_error = None;
        if parts.first() == Some(&"GET")
            && host == Some(expected_host.as_str())
            && let Some(url) = parsed
            && url.path() == "/oauth/callback"
        {
            let values = url.query_pairs().collect::<Vec<_>>();
            let states = values
                .iter()
                .filter(|(name, _)| name == "state")
                .collect::<Vec<_>>();
            if states.len() == 1 && state_matches(&session.state, &states[0].1) {
                let codes = values
                    .iter()
                    .filter(|(name, _)| name == "code")
                    .collect::<Vec<_>>();
                if codes.len() == 1 && !codes[0].1.is_empty() {
                    status = "200 OK";
                    code = Some(codes[0].1.to_string());
                } else if values.iter().any(|(name, _)| name == "error") {
                    terminal_error = Some("OAuth authorization denied by authorization server");
                }
            }
        }
        stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\n\r\nAuthorization callback received. Return to the local application.").as_bytes()).await?;
        if let Some(error) = terminal_error {
            bail!(error);
        }
        if let Some(code) = code {
            return Ok(code);
        }
    }
}
