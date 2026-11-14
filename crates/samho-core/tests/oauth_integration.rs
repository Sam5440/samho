use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Form, State},
    http::StatusCode,
    routing::post,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use samho_core::mcp::oauth::{MemoryCredentialStore, OAuthClient, OAuthConfig};
use samho_core::test_support::bind_loopback;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
#[derive(Default)]
struct Fixture {
    expected: Mutex<BTreeMap<String, String>>,
    forms: Mutex<Vec<BTreeMap<String, String>>>,
    reject: bool,
    slow: bool,
}
async fn token(
    State(state): State<Arc<Fixture>>,
    Form(form): Form<BTreeMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    state.forms.lock().unwrap().push(form.clone());
    if state.slow {
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    if state.reject {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"invalid_grant","error_description":"secret fixture ignored"})),
        );
    }
    assert_eq!(
        form.get("client_id").map(String::as_str),
        Some("public-local")
    );
    assert_eq!(
        form.get("resource").map(String::as_str),
        Some("https://example.test/mcp")
    );
    if form["grant_type"] == "authorization_code" {
        let expected = state.expected.lock().unwrap();
        assert_eq!(form["redirect_uri"], expected["redirect_uri"]);
        assert_eq!(
            URL_SAFE_NO_PAD.encode(Sha256::digest(form["code_verifier"].as_bytes())),
            expected["code_challenge"]
        );
        assert_eq!(form["code"], "fixture-code");
        (
            StatusCode::OK,
            Json(
                json!({"access_token":"fixture-access-secret","token_type":"Bearer","refresh_token":"fixture-refresh-secret","expires_in":3600,"scope":"tools"}),
            ),
        )
    } else {
        assert_eq!(form["grant_type"], "refresh_token");
        assert_eq!(form["refresh_token"], "fixture-refresh-secret");
        (
            StatusCode::OK,
            Json(
                json!({"access_token":"rotated-access-secret","token_type":"Bearer","refresh_token":"rotated-refresh-secret","expires_in":1800}),
            ),
        )
    }
}
async fn revoke(
    State(state): State<Arc<Fixture>>,
    Form(form): Form<BTreeMap<String, String>>,
) -> StatusCode {
    state.forms.lock().unwrap().push(form.clone());
    assert_eq!(form["token"], "rotated-refresh-secret");
    assert_eq!(form["token_type_hint"], "refresh_token");
    StatusCode::OK
}
async fn fixture(
    state: Arc<Fixture>,
    timeout_ms: u64,
) -> Result<(Arc<OAuthClient>, String, tokio::task::JoinHandle<()>)> {
    let listener = bind_loopback().await?;
    let base = format!("http://{}", listener.local_addr()?);
    let app = Router::new()
        .route("/token", post(token))
        .route("/revoke", post(revoke))
        .with_state(state);
    let running = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = Arc::new(OAuthClient::new(
        OAuthConfig {
            authorization_endpoint: format!("{base}/authorize"),
            token_endpoint: format!("{base}/token"),
            revocation_endpoint: Some(format!("{base}/revoke")),
            client_id: "public-local".into(),
            scopes: vec!["tools".into()],
            resource: Some("https://example.test/mcp".into()),
            timeout_ms,
            callback_port_range: Some((1301, 1399)),
        },
        Arc::new(MemoryCredentialStore::default()),
    )?);
    Ok((client, base, running))
}
fn parameters(url: &str) -> BTreeMap<String, String> {
    reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}
async fn callback(
    redirect: &str,
    state: &str,
    param: &str,
    value: &str,
) -> Result<reqwest::StatusCode> {
    let mut url = reqwest::Url::parse(redirect)?;
    url.query_pairs_mut()
        .append_pair("state", state)
        .append_pair(param, value);
    Ok(reqwest::get(url).await?.status())
}
#[tokio::test]
async fn pkce_loopback_state_refresh_rotation_and_revoke_use_real_http() -> Result<()> {
    let state = Arc::new(Fixture::default());
    let (client, _, server) = fixture(state.clone(), 2000).await?;
    let session = client.begin_authorization().await?;
    let params = parameters(&session.authorization_url);
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["response_type"], "code");
    assert_eq!(params["scope"], "tools");
    assert_eq!(params["state"].len(), 43);
    assert!(!format!("{session:?}").contains(&params["state"]));
    *state.expected.lock().unwrap() = params.clone();
    let redirect = session.redirect_uri.clone();
    let worker = client.clone();
    let authorization = tokio::spawn(async move {
        worker
            .complete_authorization("fixture", session, CancellationToken::new())
            .await
    });
    assert_eq!(
        callback(&redirect, "wrong-state", "code", "fixture-code").await?,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        callback(&redirect, &params["state"], "code", "fixture-code").await?,
        StatusCode::OK
    );
    let summary = authorization.await??;
    assert!(summary.has_access_token && summary.has_refresh_token);
    assert!(summary.expires_at_ms.is_some());
    let credential = client.credential("fixture").await?.unwrap();
    assert_eq!(credential.bearer(), "fixture-access-secret");
    assert!(!format!("{credential:?}").contains("fixture-access-secret"));
    assert!(!serde_json::to_string(&summary)?.contains("secret"));
    client.refresh("fixture", CancellationToken::new()).await?;
    assert_eq!(
        client.credential("fixture").await?.unwrap().bearer(),
        "rotated-access-secret"
    );
    client.revoke("fixture", CancellationToken::new()).await?;
    assert!(client.credential("fixture").await?.is_none());
    assert_eq!(state.forms.lock().unwrap().len(), 3);
    server.abort();
    Ok(())
}
#[tokio::test]
async fn rejected_denied_cancelled_and_expired_sessions_do_not_create_credentials() -> Result<()> {
    let state = Arc::new(Fixture {
        reject: true,
        ..Default::default()
    });
    let (client, _, server) = fixture(state, 2000).await?;
    let session = client.begin_authorization().await?;
    let params = parameters(&session.authorization_url);
    let redirect = session.redirect_uri.clone();
    let worker = client.clone();
    let authorization = tokio::spawn(async move {
        worker
            .complete_authorization("denied", session, CancellationToken::new())
            .await
    });
    callback(&redirect, &params["state"], "error", "access_denied").await?;
    assert!(
        authorization
            .await?
            .unwrap_err()
            .to_string()
            .contains("denied")
    );
    assert!(client.credential("denied").await?.is_none());
    let session = client.begin_authorization().await?;
    let params = parameters(&session.authorization_url);
    let redirect = session.redirect_uri.clone();
    let worker = client.clone();
    let authorization = tokio::spawn(async move {
        worker
            .complete_authorization("rejected", session, CancellationToken::new())
            .await
    });
    callback(&redirect, &params["state"], "code", "fixture-code").await?;
    let error = authorization.await?.unwrap_err().to_string();
    assert!(error.contains("401"));
    assert!(!error.contains("secret"));
    assert!(client.credential("rejected").await?.is_none());
    let session = client.begin_authorization().await?;
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(
        client
            .complete_authorization("cancelled", session, cancelled)
            .await
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    server.abort();
    let (client, _, server) = fixture(Arc::new(Fixture::default()), 60).await?;
    let session = client.begin_authorization().await?;
    assert!(
        client
            .complete_authorization("expired", session, CancellationToken::new())
            .await
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    assert!(client.credential("expired").await?.is_none());
    server.abort();
    Ok(())
}
#[tokio::test]
async fn reserved_authorization_query_and_non_https_remote_endpoints_are_rejected() -> Result<()> {
    let (client, base, server) = fixture(Arc::new(Fixture::default()), 500).await?;
    drop(client);
    let config = OAuthConfig {
        authorization_endpoint: format!("{base}/authorize?state=attacker"),
        token_endpoint: format!("{base}/token"),
        revocation_endpoint: None,
        client_id: "public-local".into(),
        scopes: vec![],
        resource: None,
        timeout_ms: 500,
        callback_port_range: Some((1301, 1399)),
    };
    assert!(
        OAuthClient::new(config.clone(), Arc::new(MemoryCredentialStore::default()))?
            .begin_authorization()
            .await
            .unwrap_err()
            .to_string()
            .contains("reserved")
    );
    let config = OAuthConfig {
        authorization_endpoint: "http://8.8.8.8/authorize".into(),
        ..config
    };
    assert!(
        OAuthClient::new(config, Arc::new(MemoryCredentialStore::default()))?
            .begin_authorization()
            .await
            .is_err()
    );
    server.abort();
    Ok(())
}
