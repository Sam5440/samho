use anyhow::Result;
use async_trait::async_trait;
use rpi_core::events::{EventLog, EventLogOptions, EventQuery, Redactor, TraceEvent};
use rpi_core::object::{
    InvocationContext, InvocationMiddleware, Object, ObjectEnvelope, ObjectKind, ObjectRegistry,
};
use rpi_core::session::{FileJournalBackend, JournalBackend, SessionStore, SqliteJournalBackend};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

fn redactor() -> Redactor {
    Redactor::new(Vec::new())
}
#[test]
fn streaming_redaction_never_releases_cross_chunk_secret_prefixes() {
    let redactor = Redactor::new(vec!["two word secret".into(), "秘密令牌".into()]);
    for text in [
        "hello sk-",
        "hello Bear",
        "hello Bearer ",
        "hello Bearer abc",
        "hello two word ",
        "hello 秘密",
    ] {
        assert_eq!(redactor.streaming_text(text, false), "hello ");
    }
    assert_eq!(
        redactor.streaming_text("hello safe word", false),
        "hello safe "
    );
    assert_eq!(
        redactor.streaming_text("hello 秘密令牌", true),
        "hello [REDACTED]"
    );
    assert_eq!(
        redactor.streaming_text("ok Bearer opaque-secret done ", false),
        "ok [REDACTED] done "
    );
}
async fn exercise(backend: Arc<dyn JournalBackend>) -> Result<Value> {
    let store = SessionStore::open_with_backend(backend.clone(), redactor(), true).await?;
    let session = store.create_or_get(None, "system".into()).await?;
    let mut s = session.lock().await;
    store.commit(&s).await?;
    for i in 0..20 {
        s.messages
            .push(json!({"role":"user","content":format!("message {i}")}));
        store.commit(&s).await?;
    }
    s.messages[10] = json!({"role":"user","content":"edited"});
    store.commit(&s).await?;
    let id = s.id.clone();
    drop(s);
    let child = store.fork(&id, 10, Some("branch".into())).await?;
    assert_eq!(child.parent_id.as_deref(), Some(id.as_str()));
    assert_eq!(child.messages.len(), 10);
    {
        let mut parent = session.lock().await;
        parent
            .messages
            .push(json!({"role":"user","content":"parent only"}));
        store.commit(&parent).await?;
    }
    drop(store);
    let reopened = SessionStore::open_with_backend(backend.clone(), redactor(), true).await?;
    let branch = reopened.get(&child.id).await?.unwrap();
    assert_eq!(branch.messages.len(), 10);
    assert_eq!(branch.title, "branch");
    assert_eq!(reopened.get(&id).await?.unwrap().messages.len(), 22);
    let rows = backend.load().await?;
    assert_eq!(rows.len(), 25);
    let appended: usize = rows
        .iter()
        .map(|r| {
            serde_json::from_slice::<Value>(r).unwrap()["append_messages"]
                .as_array()
                .unwrap()
                .len()
        })
        .sum();
    assert!(appended < 45, "journal grew as full snapshots");
    Ok(json!({"rows":rows.len(),"messages":branch.messages.len(),"append":appended}))
}
#[tokio::test]
async fn file_and_sqlite_delta_backends_have_identical_projection() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let file = exercise(Arc::new(
        FileJournalBackend::open(dir.path().join("session.jsonl")).await?,
    ))
    .await?;
    let sqlite = exercise(Arc::new(
        SqliteJournalBackend::open(dir.path().join("session.sqlite")).await?,
    ))
    .await?;
    assert_eq!(file, sqlite);
    Ok(())
}
#[tokio::test]
async fn legacy_snapshots_migrate_and_complete_checksum_corruption_is_rejected() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("sessions.jsonl");
    let mut legacy = rpi_core::session::Session::new("legacy".into(), "system".into());
    legacy.status = "running".into();
    let line = serde_json::to_string(&legacy)?;
    std::fs::write(&path, format!("{line}\npartial"))?;
    let store = SessionStore::open(&path, redactor(), true).await?;
    assert_eq!(store.get("legacy").await?.unwrap().status, "interrupted");
    drop(store);
    assert!(path.with_extension("legacy.jsonl").exists());
    let bytes = std::fs::read_to_string(&path)?;
    let mut value: Value = serde_json::from_str(bytes.trim())?;
    value["metadata"]["title"] = json!("tamper");
    let corrupt = format!("{}\nunfinished", serde_json::to_string(&value)?);
    std::fs::write(&path, &corrupt)?;
    assert!(SessionStore::open(&path, redactor(), true).await.is_err());
    assert_eq!(std::fs::read_to_string(&path)?, corrupt);
    Ok(())
}
#[tokio::test]
async fn valid_delta_tail_is_recovered_and_fork_rejects_running_and_split_tool_groups() -> Result<()>
{
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("sessions.jsonl");
    let store = SessionStore::open(&path, redactor(), false).await?;
    let s = store.create_or_get(None, "system".into()).await?;
    let id;
    {
        let mut session = s.lock().await;
        id = session.id.clone();
        session.status = "running".into();
        store.commit(&session).await?;
    }
    assert!(store.fork(&id, 1, None).await.is_err());
    {
        let mut session = s.lock().await;
        session.status = "ready".into();
        session
            .messages
            .push(json!({"role":"assistant","tool_calls":[{"id":"call-1"}]}));
        session
            .messages
            .push(json!({"role":"tool","tool_call_id":"call-1","content":"done"}));
        store.commit(&session).await?;
    }
    assert!(store.fork(&id, 2, None).await.is_err());
    assert!(store.fork(&id, 3, None).await.is_ok());
    drop(s);
    drop(store);
    let valid = std::fs::read(&path)?;
    let mut bytes = valid.clone();
    bytes.extend_from_slice(b"{partial");
    std::fs::write(&path, bytes)?;
    let recovered = SessionStore::open(&path, redactor(), false).await?;
    assert_eq!(std::fs::metadata(&path)?.len(), valid.len() as u64);
    assert_eq!(recovered.get(&id).await?.unwrap().messages.len(), 3);
    Ok(())
}
struct FailingBackend {
    rows: Mutex<Vec<Vec<u8>>>,
    fail: AtomicBool,
}
#[async_trait]
impl JournalBackend for FailingBackend {
    async fn load(&self) -> Result<Vec<Vec<u8>>> {
        Ok(self.rows.lock().unwrap().clone())
    }
    async fn append(&self, record: &[u8], _: bool) -> Result<()> {
        if self.fail.load(Ordering::Relaxed) {
            anyhow::bail!("ENOSPC fixture");
        }
        self.rows.lock().unwrap().push(record.to_vec());
        Ok(())
    }
    async fn replace(&self, records: &[Vec<u8>], _: bool) -> Result<()> {
        *self.rows.lock().unwrap() = records.to_vec();
        Ok(())
    }
    fn location(&self) -> PathBuf {
        PathBuf::from("fault-fixture")
    }
    fn kind(&self) -> &'static str {
        "fault"
    }
}
#[tokio::test]
async fn disk_failure_does_not_advance_session_or_event_commit() -> Result<()> {
    let backend = Arc::new(FailingBackend {
        rows: Mutex::new(Vec::new()),
        fail: AtomicBool::new(true),
    });
    let store = SessionStore::open_with_backend(backend.clone(), redactor(), false).await?;
    let s = store.create_or_get(None, "system".into()).await?;
    assert!(store.commit(&*s.lock().await).await.is_err());
    assert_eq!(store.journal_metadata().await["records"], 0);
    backend.fail.store(false, Ordering::Relaxed);
    store.commit(&*s.lock().await).await?;
    assert_eq!(store.journal_metadata().await["records"], 1);
    let events_backend = Arc::new(FailingBackend {
        rows: Mutex::new(Vec::new()),
        fail: AtomicBool::new(true),
    });
    let events = EventLog::open_with_backend(
        events_backend.clone(),
        EventLogOptions::default(),
        false,
        redactor(),
    )
    .await?;
    assert!(events.append(event(0)).await.is_err());
    assert_eq!(events.len().await, 0);
    events_backend.fail.store(false, Ordering::Relaxed);
    assert_eq!(events.append(event(0)).await?, 1);
    Ok(())
}
fn event(i: usize) -> TraceEvent {
    TraceEvent::new(
        if i.is_multiple_of(2) { "even" } else { "odd" },
        "session",
        "span",
        "fixture",
        json!({"i":i}),
    )
}
#[tokio::test]
async fn event_cursor_rotation_retention_recovery_and_timeline() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("events.jsonl");
    let options = EventLogOptions {
        rotation_bytes: 900,
        max_archives: 2,
    };
    let log = EventLog::open_with_options(&path, options.clone(), false, redactor()).await?;
    for i in 0..20 {
        assert_eq!(log.append(event(i)).await?, i as u64 + 1);
    }
    assert!(log.len().await < 20);
    let first = log
        .query(EventQuery {
            limit: 2,
            ..Default::default()
        })
        .await?;
    assert!(first.first_available_sequence.unwrap() > 1);
    assert_eq!(first.events.len(), 2);
    assert!(first.has_more);
    let second = log
        .query(EventQuery {
            after_sequence: first.next_cursor.unwrap(),
            limit: 2,
            ..Default::default()
        })
        .await?;
    assert!(second.events[0].sequence > first.events[1].sequence);
    assert_eq!(log.timeline("even").await?.len(), 1);
    let rows = log.events(None).await?;
    drop(log);
    let reopened = EventLog::open_with_options(&path, options, false, redactor()).await?;
    assert_eq!(
        serde_json::to_value(reopened.events(None).await?)?,
        serde_json::to_value(rows)?
    );
    assert_eq!(reopened.append(event(21)).await?, 21);
    assert_eq!(
        reopened.schema_metadata().await["query_backend"],
        "file_byte_offset_index"
    );
    Ok(())
}
#[tokio::test]
async fn sqlite_event_journal_paging_and_restart_preserve_sequence() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let backend = Arc::new(SqliteJournalBackend::open(dir.path().join("events.sqlite")).await?);
    let log = EventLog::open_with_backend(
        backend.clone(),
        EventLogOptions::default(),
        true,
        redactor(),
    )
    .await?;
    for i in 0..7 {
        log.append(event(i)).await?;
    }
    let page = log
        .query(EventQuery {
            trace_id: Some("even".into()),
            limit: 2,
            ..Default::default()
        })
        .await?;
    assert_eq!(
        page.events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert!(page.has_more);
    drop(log);
    let reopened =
        EventLog::open_with_backend(backend, EventLogOptions::default(), true, redactor()).await?;
    assert_eq!(reopened.append(event(8)).await?, 8);
    Ok(())
}
struct FixtureObject {
    name: String,
    value: u64,
    fail: bool,
    history: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl Object for FixtureObject {
    fn envelope(&self) -> ObjectEnvelope {
        ObjectEnvelope::new(&self.name, ObjectKind::Tool, "fixture")
    }
    async fn invoke(&self, _: &str, input: Value, _: InvocationContext) -> Result<Value> {
        Ok(json!({"version":self.value,"input":input}))
    }
    async fn activate(&self) -> Result<()> {
        self.history
            .lock()
            .unwrap()
            .push(format!("start:{}", self.value));
        if self.fail {
            anyhow::bail!("fixture activation failure");
        }
        Ok(())
    }
    async fn dispose(&self) -> Result<()> {
        self.history
            .lock()
            .unwrap()
            .push(format!("stop:{}", self.value));
        Ok(())
    }
}
fn object(value: u64, fail: bool, history: &Arc<Mutex<Vec<String>>>) -> Arc<dyn Object> {
    Arc::new(FixtureObject {
        name: "tool:x".into(),
        value,
        fail,
        history: history.clone(),
    })
}
fn context() -> InvocationContext {
    InvocationContext {
        session_id: "s".into(),
        trace_id: "t".into(),
        span_id: "p".into(),
        cancel: CancellationToken::new(),
    }
}
#[tokio::test]
async fn replacement_drains_leases_and_rolls_back_failure_or_timeout() -> Result<()> {
    let registry = ObjectRegistry::default();
    let history = Arc::new(Mutex::new(Vec::new()));
    registry.register(object(1, false, &history))?;
    registry.activate().await?;
    let generation = registry.identity("tool:x")?.generation;
    assert!(
        registry
            .hot_replace(
                "tool:x",
                object(2, true, &history),
                Duration::from_millis(10)
            )
            .await
            .is_err()
    );
    assert_eq!(registry.identity("tool:x")?.generation, generation);
    let lease = registry.acquire("tool:x")?;
    assert!(
        registry
            .hot_replace(
                "tool:x",
                object(3, false, &history),
                Duration::from_millis(10)
            )
            .await
            .is_err()
    );
    assert!(registry.acquire("tool:x").is_ok());
    drop(lease);
    let lease = registry.acquire("tool:x")?;
    let copy = registry.clone();
    let next = object(4, false, &history);
    let replacement = tokio::spawn(async move {
        copy.hot_replace("tool:x", next, Duration::from_secs(1))
            .await
    });
    tokio::time::sleep(Duration::from_millis(15)).await;
    assert!(registry.acquire("tool:x").is_err());
    assert!(!history.lock().unwrap().contains(&"stop:1".into()));
    drop(lease);
    let new = replacement.await??;
    assert!(new.generation > generation);
    assert_eq!(
        registry
            .invoke_registered("tool:x", "call", json!(null), context())
            .await?["version"],
        4
    );
    registry.dispose().await?;
    Ok(())
}
struct Hook(Arc<Mutex<Vec<String>>>);
#[async_trait]
impl InvocationMiddleware for Hook {
    async fn before(
        &self,
        _: &rpi_core::object::ObjectIdentity,
        _: &str,
        input: &mut Value,
        _: &InvocationContext,
    ) -> Result<()> {
        self.0.lock().unwrap().push("before".into());
        *input = json!("hooked");
        Ok(())
    }
    async fn after(
        &self,
        _: &rpi_core::object::ObjectIdentity,
        _: &str,
        _: &Result<Value>,
        _: &InvocationContext,
    ) -> Result<()> {
        self.0.lock().unwrap().push("after".into());
        Ok(())
    }
}
#[tokio::test]
async fn scope_visibility_dependency_rules_and_middleware_are_enforced() -> Result<()> {
    let registry = ObjectRegistry::default();
    let history = Arc::new(Mutex::new(Vec::new()));
    registry.register_scope("left", "root")?;
    registry.register_scope("right", "root")?;
    registry.register_in_scope(object(1, false, &history), "left")?;
    registry.activate().await?;
    assert!(registry.get_in_scope("tool:x", "right").is_err());
    assert!(registry.get_in_scope("tool:x", "left").is_ok());
    registry.add_middleware(Arc::new(Hook(history.clone())))?;
    let value = registry
        .invoke_registered("tool:x", "call", json!(null), context())
        .await?;
    assert_eq!(value["input"], "hooked");
    assert!(
        history
            .lock()
            .unwrap()
            .ends_with(&["before".into(), "after".into()])
    );
    registry
        .dispose_scope("left", Duration::from_secs(1))
        .await?;
    assert!(registry.get("tool:x").is_err());
    Ok(())
}
