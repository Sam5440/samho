use crate::events::{Redactor, now_ms};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
};
use tokio::{
    io::{AsyncSeekExt, AsyncWriteExt, SeekFrom},
    sync::Mutex,
};

pub const SESSION_SCHEMA_VERSION: u32 = 2;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub status: String,
    pub messages: Vec<Value>,
    pub last_model: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub fork_cutoff: Option<usize>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub project_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub status: String,
    pub last_model: Option<String>,
    pub message_count: usize,
    pub parent_id: Option<String>,
    pub fork_cutoff: Option<usize>,
    pub archived: bool,
    #[serde(default)]
    pub project_id: Option<String>,
}
impl From<&Session> for SessionSummary {
    fn from(s: &Session) -> Self {
        Self {
            id: s.id.clone(),
            title: s.title.clone(),
            created_at: s.created_at,
            updated_at: s.updated_at,
            status: s.status.clone(),
            last_model: s.last_model.clone(),
            message_count: s.messages.len(),
            parent_id: s.parent_id.clone(),
            fork_cutoff: s.fork_cutoff,
            archived: s.archived,
            project_id: s.project_id.clone(),
        }
    }
}
impl Session {
    pub fn new(id: String, system_prompt: String) -> Self {
        let t = now_ms();
        Self {
            id,
            title: "新会话".into(),
            created_at: t,
            updated_at: t,
            status: "ready".into(),
            messages: vec![serde_json::json!({"role":"system","content":system_prompt})],
            last_model: None,
            parent_id: None,
            fork_cutoff: None,
            archived: false,
            project_id: None,
        }
    }
}
/// Backends publish each append or migration atomically, or return an error.
#[async_trait]
pub trait JournalBackend: Send + Sync {
    async fn load(&self) -> Result<Vec<Vec<u8>>>;
    async fn append(&self, record: &[u8], durable: bool) -> Result<()>;
    async fn replace(&self, records: &[Vec<u8>], durable: bool) -> Result<()>;
    async fn finish_recovery(&self) -> Result<()> {
        Ok(())
    }
    fn location(&self) -> PathBuf;
    fn kind(&self) -> &'static str;
}
struct FileState {
    file: Option<tokio::fs::File>,
    valid_end: Option<u64>,
}
pub struct FileJournalBackend {
    path: PathBuf,
    writer: Mutex<FileState>,
}
impl FileJournalBackend {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(p) = path.parent() {
            tokio::fs::create_dir_all(p).await?;
        }
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .await?;
        Ok(Self {
            path,
            writer: Mutex::new(FileState {
                file: Some(file),
                valid_end: None,
            }),
        })
    }
}
#[async_trait]
impl JournalBackend for FileJournalBackend {
    async fn load(&self) -> Result<Vec<Vec<u8>>> {
        let mut state = self.writer.lock().await;
        let bytes = tokio::fs::read(&self.path).await?;
        let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |p| p + 1);
        state.valid_end = Some(end as u64);
        Ok(bytes[..end]
            .split(|b| *b == b'\n')
            .filter(|v| !v.is_empty())
            .map(Vec::from)
            .collect())
    }
    async fn finish_recovery(&self) -> Result<()> {
        let mut state = self.writer.lock().await;
        if let Some(end) = state.valid_end.take() {
            state
                .file
                .as_mut()
                .context("journal writer closed")?
                .set_len(end)
                .await?;
        }
        Ok(())
    }
    async fn append(&self, record: &[u8], durable: bool) -> Result<()> {
        let mut state = self.writer.lock().await;
        let file = state.file.as_mut().context("journal writer closed")?;
        let offset = file.seek(SeekFrom::End(0)).await?;
        let mut bytes = record.to_vec();
        bytes.push(b'\n');
        if let Err(e) = async {
            file.write_all(&bytes).await?;
            file.flush().await?;
            if durable {
                file.sync_data().await?;
            }
            Ok::<_, std::io::Error>(())
        }
        .await
        {
            let _ = file.set_len(offset).await;
            return Err(e.into());
        }
        Ok(())
    }
    async fn replace(&self, records: &[Vec<u8>], durable: bool) -> Result<()> {
        let mut state = self.writer.lock().await;
        let tmp = self
            .path
            .with_extension(format!("migration-{}", uuid::Uuid::new_v4()));
        let mut candidate = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp)
            .await?;
        for r in records {
            candidate.write_all(r).await?;
            candidate.write_all(b"\n").await?;
        }
        candidate.flush().await?;
        if durable {
            candidate.sync_all().await?;
        }
        drop(candidate);
        tokio::fs::copy(&self.path, self.path.with_extension("legacy.jsonl")).await?;
        drop(state.file.take());
        let published = tokio::fs::rename(&tmp, &self.path).await;
        state.file = Some(
            tokio::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.path)
                .await?,
        );
        published?;
        state.valid_end = None;
        Ok(())
    }
    fn location(&self) -> PathBuf {
        self.path.clone()
    }
    fn kind(&self) -> &'static str {
        "file"
    }
}
pub struct SqliteJournalBackend {
    path: PathBuf,
    connection: Arc<StdMutex<rusqlite::Connection>>,
}
impl SqliteJournalBackend {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(p) = path.parent() {
            tokio::fs::create_dir_all(p).await?;
        }
        let location = path.clone();
        let connection=tokio::task::spawn_blocking(move||->Result<rusqlite::Connection>{
            let c=rusqlite::Connection::open(location)?;
            c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS journal (sequence INTEGER PRIMARY KEY AUTOINCREMENT, body BLOB NOT NULL);")?;Ok(c)
        }).await??;
        Ok(Self {
            path,
            connection: Arc::new(StdMutex::new(connection)),
        })
    }
}
#[async_trait]
impl JournalBackend for SqliteJournalBackend {
    async fn load(&self) -> Result<Vec<Vec<u8>>> {
        let conn = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|_| anyhow::anyhow!("SQLite lock poisoned"))?;
            let mut query = guard.prepare("SELECT body FROM journal ORDER BY sequence")?;
            Ok(query
                .query_map([], |r| r.get::<_, Vec<u8>>(0))?
                .collect::<rusqlite::Result<_>>()?)
        })
        .await?
    }
    async fn append(&self, record: &[u8], durable: bool) -> Result<()> {
        let conn = self.connection.clone();
        let bytes = record.to_vec();
        tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|_| anyhow::anyhow!("SQLite lock poisoned"))?;
            guard.pragma_update(None, "synchronous", if durable { "FULL" } else { "NORMAL" })?;
            guard.execute("INSERT INTO journal(body) VALUES (?1)", [bytes])?;
            Ok(())
        })
        .await?
    }
    async fn replace(&self, records: &[Vec<u8>], durable: bool) -> Result<()> {
        let conn = self.connection.clone();
        let records = records.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| anyhow::anyhow!("SQLite lock poisoned"))?;
            guard.pragma_update(None, "synchronous", if durable { "FULL" } else { "NORMAL" })?;
            let tx = guard.transaction()?;
            tx.execute("DELETE FROM journal", [])?;
            for r in records {
                tx.execute("INSERT INTO journal(body) VALUES (?1)", [r])?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?
    }
    fn location(&self) -> PathBuf {
        self.path.clone()
    }
    fn kind(&self) -> &'static str {
        "sqlite"
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Meta {
    id: String,
    title: String,
    created_at: u64,
    updated_at: u64,
    status: String,
    last_model: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    fork_cutoff: Option<usize>,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    project_id: Option<String>,
}
impl Meta {
    fn from_session(s: &Session) -> Self {
        Self {
            id: s.id.clone(),
            title: s.title.clone(),
            created_at: s.created_at,
            updated_at: s.updated_at,
            status: s.status.clone(),
            last_model: s.last_model.clone(),
            parent_id: s.parent_id.clone(),
            fork_cutoff: s.fork_cutoff,
            archived: s.archived,
            project_id: s.project_id.clone(),
        }
    }
    fn project(self, messages: Vec<Value>) -> Session {
        Session {
            id: self.id,
            title: self.title,
            created_at: self.created_at,
            updated_at: self.updated_at,
            status: self.status,
            last_model: self.last_model,
            messages,
            parent_id: self.parent_id,
            fork_cutoff: self.fork_cutoff,
            archived: self.archived,
            project_id: self.project_id,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Delta {
    schema_version: u32,
    sequence: u64,
    previous_checksum: String,
    metadata: Meta,
    truncate_to: usize,
    append_messages: Vec<Value>,
}
#[derive(Serialize, Deserialize)]
struct Record {
    #[serde(flatten)]
    payload: Delta,
    checksum: String,
}
fn digest(delta: &Delta) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(delta)?)))
}
/// 由 committed 映射求级联删除集合（目标 + 全部 fork 后代）。
fn cascade_doomed(
    committed: &BTreeMap<String, Session>,
    id: &str,
) -> std::collections::BTreeSet<String> {
    let mut doomed: std::collections::BTreeSet<String> = std::iter::once(id.to_string()).collect();
    loop {
        let mut grew = false;
        for s in committed.values() {
            if let Some(parent) = &s.parent_id
                && doomed.contains(parent)
                && doomed.insert(s.id.clone())
            {
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    doomed
}

/// 旧数据目录遗留 journal 的校验辅助：对"payload 原字节"做 SHA-256。
/// serde flatten 写出的行是 `{"schema_version":…,"append_messages":[…],"checksum":"…"}` ——
/// payload 与 checksum 共用同一个外层对象，外层收尾 `}` 位于行尾；
/// payload 原字节即"checksum 键之前的内容 + 该收尾 `}`"。
///
/// 旧字段集 journal 的 stored checksum 正是对这段原字节计算的；当前版本的
/// stored checksum 则来自 Struct Delta 重序列化（写键序一致，故 round-trip
/// digest 不变）。两者都不匹配即数据损坏。不再经 serde_json::Value 往返 ——
/// 默认 Map 按字母序排列，会改写键序字节、无法复算写入时的 digest。
fn legacy_payload_digest(raw_record: &[u8]) -> Result<String> {
    let marker = b"\"checksum\":";
    let marker_pos = raw_record
        .windows(marker.len())
        .rposition(|window| window == marker)
        .context("journal record lacks a checksum field")?;
    // marker 前必须是分隔逗号；含逗号前的 payload 全段。
    if marker_pos == 0 || raw_record[marker_pos - 1] != b',' {
        bail!("checksum marker has no leading comma");
    }
    let mut payload = raw_record[..marker_pos - 1].to_vec();
    // serde flatten 让外层收尾 `}` 落在行尾；校验并补上凑齐 payload 原字节。
    if raw_record.last() != Some(&b'}') {
        bail!("record does not end with the outer closing brace");
    }
    payload.push(b'}');
    Ok(format!("{:x}", Sha256::digest(&payload)))
}

/// 探测 journal 的 schema 代次（恒等关系：写入时 digest 用什么序列化，
/// 重放就只有用什么序列化才校验得过）：
/// - `digest(Record struct 重序列化) == stored` → 当前 schema；
/// - 否则 `按原始字节 payload digest == stored` → 旧字段集 journal（可迁移）；
/// - 两者皆非 → 数据损坏，按校验失败处理。
///
/// 当前 schema 的所有写入都发自同一 struct，round-trip 后 digest 不变，
/// 因此该判据不会把现行 journal 误判为 legacy。
fn legacy_meta_journal(records: &[Vec<u8>]) -> Result<bool> {
    if records.is_empty() {
        return Ok(false);
    }
    let mut all_legacy = true;
    for (index, bytes) in records.iter().enumerate() {
        let v: Value = serde_json::from_slice(bytes)
            .with_context(|| format!("corrupt session journal record {}", index + 1))?;
        if v.get("schema_version").is_none() {
            // 非平铺 Delta（旧整段快照等）：交给 open_with_backend 常规路径。
            return Ok(false);
        }
        let r: Record = serde_json::from_value(v.clone())
            .with_context(|| format!("corrupt journal record {}", index + 1))?;
        if digest(&r.payload)? == r.checksum {
            all_legacy = false;
        } else if legacy_payload_digest(bytes)? != r.checksum {
            bail!("session checksum/sequence mismatch at {}", index + 1);
        }
    }
    Ok(all_legacy)
}

/// 按 Delta 语义重放一条记录到 committed（拷贝自原 open_with_backend 内联逻辑），
/// 返回会话 id 与投影后的完整会话。
fn replay_delta(committed: &mut BTreeMap<String, Session>, p: &Delta) -> Result<(String, Session)> {
    let mut messages = if let Some(s) = committed.get(&p.metadata.id) {
        s.messages.clone()
    } else if let Some(id) = &p.metadata.parent_id {
        let source = committed.get(id).context("missing fork parent")?;
        let cutoff = p.metadata.fork_cutoff.context("missing fork cutoff")?;
        if cutoff > source.messages.len() {
            bail!("fork cutoff exceeds parent");
        }
        source.messages[..cutoff].to_vec()
    } else {
        Vec::new()
    };
    if p.truncate_to > messages.len() {
        bail!("delta cutoff exceeds session");
    }
    messages.truncate(p.truncate_to);
    messages.extend(p.append_messages.clone());
    let id = p.metadata.id.clone();
    let session = p.metadata.clone().project(messages);
    committed.insert(id.clone(), session.clone());
    Ok((id, session))
}
struct JournalState {
    committed: BTreeMap<String, Session>,
    sequence: u64,
    checksum: String,
}
#[derive(Clone)]
pub struct SessionStore {
    sessions: Arc<Mutex<BTreeMap<String, Arc<Mutex<Session>>>>>,
    journal: Arc<Mutex<JournalState>>,
    backend: Arc<dyn JournalBackend>,
    redactor: Redactor,
    durable: bool,
    path: PathBuf,
}
impl SessionStore {
    pub async fn open(path: &Path, redactor: Redactor, durable: bool) -> Result<Self> {
        Self::open_with_backend(
            Arc::new(FileJournalBackend::open(path).await?),
            redactor,
            durable,
        )
        .await
    }
    pub async fn open_with_backend(
        backend: Arc<dyn JournalBackend>,
        redactor: Redactor,
        durable: bool,
    ) -> Result<Self> {
        let records = backend.load().await?;
        let mut committed = BTreeMap::<String, Session>::new();
        let mut sequence = 0;
        let mut checksum = String::new();
        // 先探测 journal 的 schema 代次：旧字段集（archived/project_id 加入
        // 之前）的 stored digest 是按文件原始键集算的，用当前 struct 重算必
        // 失配。legacy journal 按原始键集校验整条链，通过则全部记录流式重放，
        // 最后统一走 full-snapshot 重写迁移。
        let mut legacy = legacy_meta_journal(&records).unwrap_or(false);
        if legacy {
            tracing::info!(
                "session journal uses a pre-archive Meta field set; replaying with legacy digest and migrating"
            );
        }
        for (index, bytes) in records.iter().enumerate() {
            let v: Value = serde_json::from_slice(bytes)
                .with_context(|| format!("corrupt session journal record {}", index + 1))?;
            if v.get("schema_version").is_none() {
                if sequence != 0 {
                    bail!("legacy snapshot after versioned delta");
                }
                let s: Session = serde_json::from_value(v).context("corrupt legacy snapshot")?;
                committed.insert(s.id.clone(), s);
                legacy = true;
                continue;
            }
            // 平铺 Delta 记录：legacy 判定来自 probe（journal 级），单条记录
            // 再按对应口径校验 checksum，防"探判定后又改写"的窗口。
            let r: Record = serde_json::from_value(v.clone())?;
            let p = &r.payload;
            if p.schema_version != SESSION_SCHEMA_VERSION {
                bail!("unsupported session schema {}", p.schema_version);
            }
            let expected_checksum = if legacy {
                legacy_payload_digest(bytes).context("legacy journal payload invalid")?
            } else {
                digest(p)?
            };
            if p.sequence != sequence + 1
                || p.previous_checksum != checksum
                || expected_checksum != r.checksum
            {
                bail!("session checksum/sequence mismatch at {}", index + 1);
            }
            replay_delta(&mut committed, p)?;
            sequence = p.sequence;
            checksum = r.checksum;
        }
        if legacy {
            let mut migrated = Vec::new();
            sequence = 0;
            checksum.clear();
            for s in committed.values() {
                let mut metadata = Meta::from_session(s);
                metadata.parent_id = None;
                metadata.fork_cutoff = None;
                let payload = Delta {
                    schema_version: SESSION_SCHEMA_VERSION,
                    sequence: sequence + 1,
                    previous_checksum: checksum.clone(),
                    metadata,
                    truncate_to: 0,
                    append_messages: s.messages.clone(),
                };
                checksum = digest(&payload)?;
                sequence += 1;
                migrated.push(serde_json::to_vec(&Record {
                    payload,
                    checksum: checksum.clone(),
                })?);
            }
            backend
                .replace(&migrated, durable)
                .await
                .context("session migration failed")?;
        } else {
            backend.finish_recovery().await?;
        }
        let mut sessions = BTreeMap::new();
        for (id, s) in &committed {
            let mut s = s.clone();
            if s.status == "running" {
                s.status = "interrupted".into();
            }
            sessions.insert(id.clone(), Arc::new(Mutex::new(s)));
        }
        let path = backend.location();
        Ok(Self {
            sessions: Arc::new(Mutex::new(sessions)),
            journal: Arc::new(Mutex::new(JournalState {
                committed,
                sequence,
                checksum,
            })),
            backend,
            redactor,
            durable,
            path,
        })
    }
    pub async fn create_or_get(
        &self,
        id: Option<String>,
        system_prompt: String,
    ) -> Result<Arc<Mutex<Session>>> {
        let mut sessions = self.sessions.lock().await;
        if let Some(id) = id {
            return sessions
                .get(&id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("unknown session {id}"));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let s = Arc::new(Mutex::new(Session::new(id.clone(), system_prompt)));
        sessions.insert(id, s.clone());
        Ok(s)
    }
    pub async fn commit(&self, s: &Session) -> Result<()> {
        let s: Session = serde_json::from_value(self.redactor.value(serde_json::to_value(s)?))?;
        let mut j = self.journal.lock().await;
        // 前缀比较借用 committed 中的历史，不再整体克隆 base（长会话每次
        // checkpoint 的复制成本随历史线性增长，这里去掉一次全量 clone）。
        let prefix = match j.committed.get(&s.id) {
            Some(old) => old
                .messages
                .iter()
                .zip(&s.messages)
                .take_while(|(a, b)| a == b)
                .count(),
            None => match &s.parent_id {
                Some(id) => {
                    let parent = j.committed.get(id).context("uncommitted fork parent")?;
                    let cutoff = s.fork_cutoff.context("missing fork cutoff")?;
                    if cutoff > parent.messages.len() {
                        bail!("invalid fork cutoff");
                    }
                    parent.messages[..cutoff]
                        .iter()
                        .zip(&s.messages)
                        .take_while(|(a, b)| a == b)
                        .count()
                }
                None => 0,
            },
        };
        let payload = Delta {
            schema_version: SESSION_SCHEMA_VERSION,
            sequence: j.sequence + 1,
            previous_checksum: j.checksum.clone(),
            metadata: Meta::from_session(&s),
            truncate_to: prefix,
            append_messages: s.messages[prefix..].to_vec(),
        };
        let checksum = digest(&payload)?;
        let bytes = serde_json::to_vec(&Record {
            payload,
            checksum: checksum.clone(),
        })?;
        self.backend.append(&bytes, self.durable).await?;
        j.sequence += 1;
        j.checksum = checksum;
        j.committed.insert(s.id.clone(), s);
        Ok(())
    }
    pub async fn fork(
        &self,
        parent_id: &str,
        cutoff: usize,
        title: Option<String>,
    ) -> Result<Session> {
        let source = self
            .sessions
            .lock()
            .await
            .get(parent_id)
            .cloned()
            .context("unknown fork parent")?;
        let parent = source.lock().await;
        if parent.status == "running" {
            bail!("cannot fork a running session");
        }
        if cutoff == 0 || cutoff > parent.messages.len() {
            bail!("fork cutoff must preserve system message");
        }
        let mut pending = std::collections::BTreeSet::new();
        for message in &parent.messages[..cutoff] {
            if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    if let Some(id) = call.get("id").and_then(Value::as_str) {
                        pending.insert(id.to_owned());
                    }
                }
            }
            if message.get("role").and_then(Value::as_str) == Some("tool")
                && let Some(id) = message.get("tool_call_id").and_then(Value::as_str)
            {
                pending.remove(id);
            }
        }
        if !pending.is_empty() {
            bail!("fork cutoff splits an unfinished tool-call group");
        }
        self.commit(&parent).await?;
        let t = now_ms();
        let id = uuid::Uuid::new_v4().to_string();
        let child = Session {
            id: id.clone(),
            title: title.unwrap_or_else(|| format!("{} · 分支", parent.title)),
            created_at: t,
            updated_at: t,
            status: "ready".into(),
            messages: parent.messages[..cutoff].to_vec(),
            last_model: parent.last_model.clone(),
            parent_id: Some(parent_id.into()),
            fork_cutoff: Some(cutoff),
            archived: false,
            // 分支默认留在父会话的项目里。
            project_id: parent.project_id.clone(),
        };
        self.commit(&child).await?;
        drop(parent);
        self.sessions
            .lock()
            .await
            .insert(id, Arc::new(Mutex::new(child.clone())));
        Ok(child)
    }
    /// Metadata-only updates ride the same checksum-chained delta journal as
    /// message commits, so renames and archive flags survive restarts.
    pub async fn rename(&self, id: &str, title: String) -> Result<Session> {
        self.update_metadata(id, |s| s.title = title).await
    }
    pub async fn set_archived(&self, id: &str, archived: bool) -> Result<Session> {
        self.update_metadata(id, |s| s.archived = archived).await
    }
    /// 项目归属走同一条元数据增量日志；None 表示移出项目。
    pub async fn set_project(&self, id: &str, project_id: Option<String>) -> Result<Session> {
        self.update_metadata(id, |s| s.project_id = project_id)
            .await
    }
    async fn update_metadata(&self, id: &str, apply: impl FnOnce(&mut Session)) -> Result<Session> {
        let handle = self
            .sessions
            .lock()
            .await
            .get(id)
            .cloned()
            .context("unknown session")?;
        let mut s = handle.lock().await;
        apply(&mut s);
        s.updated_at = now_ms();
        self.commit(&s).await?;
        Ok(s.clone())
    }
    /// Removes a session and every fork descendant. The journal is rewritten
    /// as one full-snapshot delta per surviving session, so the checksum chain
    /// stays verifiable across restarts.
    ///
    /// 锁顺序与 checkpoint（session → journal）一致：sessions → doomed 会话
    /// 句柄 → journal。旧实现先持 journal 再锁会话句柄，与并发的会话
    /// checkpoint 形成“任务持 session 等 journal / 删除持 journal 等 session”
    /// 的环，可能在级联删除与提交并发时死锁。
    pub async fn delete_cascade(&self, id: &str) -> Result<Vec<String>> {
        let mut sessions = self.sessions.lock().await;
        if !sessions.contains_key(id) {
            bail!("unknown session {id}");
        }
        // 句柄 Arc 存放在外层：锁守卫的生命周期必须覆盖到函数结束，
        // 且先声明保证 Drop 顺序在 handles 之后。
        let mut doomed_arcs: Vec<Arc<Mutex<Session>>> = Vec::new();
        let handles = loop {
            let doomed = {
                let j = self.journal.lock().await;
                cascade_doomed(&j.committed, id)
            };
            doomed_arcs.clear();
            let mut complete = true;
            for did in &doomed {
                match sessions.get(did).cloned() {
                    // 克隆 Arc 再加锁：句柄的生命周期独立于 sessions 借用，
                    // 后续才能从 map 中移除条目。
                    Some(handle) => doomed_arcs.push(handle),
                    // journal 中出现了尚未入 map 的条目（fork 提交进行中）：
                    // 重读 doomed 集合后重试。
                    None => {
                        complete = false;
                        break;
                    }
                }
            }
            if !complete {
                continue;
            }
            let mut locked = Vec::with_capacity(doomed_arcs.len());
            for handle in &doomed_arcs {
                locked.push(handle.lock().await);
            }
            // 持有全部 doomed 句柄期间，fork（需锁父会话）无法推进，集合稳定；
            // 再校验一次 journal 一致后落锁重写。
            let stable = {
                let j = self.journal.lock().await;
                cascade_doomed(&j.committed, id) == doomed
            };
            if stable {
                break locked;
            }
        };
        for handle in &handles {
            if handle.status == "running" {
                bail!("cannot delete a running session");
            }
        }
        let doomed: std::collections::BTreeSet<String> =
            handles.iter().map(|s| s.id.clone()).collect();
        let mut j = self.journal.lock().await;
        let mut records = Vec::new();
        let mut sequence = 0u64;
        let mut checksum = String::new();
        let mut committed = BTreeMap::new();
        for (sid, s) in j.committed.iter() {
            if doomed.contains(sid) {
                continue;
            }
            let payload = Delta {
                schema_version: SESSION_SCHEMA_VERSION,
                sequence: sequence + 1,
                previous_checksum: checksum.clone(),
                metadata: Meta::from_session(s),
                truncate_to: 0,
                append_messages: s.messages.clone(),
            };
            checksum = digest(&payload)?;
            sequence += 1;
            records.push(serde_json::to_vec(&Record {
                payload,
                checksum: checksum.clone(),
            })?);
            committed.insert(sid.clone(), s.clone());
        }
        self.backend.replace(&records, self.durable).await?;
        j.sequence = sequence;
        j.checksum = checksum;
        j.committed = committed;
        for did in &doomed {
            sessions.remove(did);
        }
        Ok(doomed.into_iter().collect())
    }
    pub async fn list(&self) -> Result<Vec<Session>> {
        let items: Vec<_> = self.sessions.lock().await.values().cloned().collect();
        let mut out = Vec::new();
        for item in items {
            out.push(item.lock().await.clone());
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        Ok(out)
    }
    pub async fn list_summaries(&self) -> Vec<SessionSummary> {
        let items: Vec<_> = self.sessions.lock().await.values().cloned().collect();
        let mut out = Vec::new();
        for item in items {
            out.push(SessionSummary::from(&*item.lock().await));
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        out
    }
    pub async fn get(&self, id: &str) -> Result<Option<Session>> {
        let s = self.sessions.lock().await.get(id).cloned();
        match s {
            Some(s) => Ok(Some(s.lock().await.clone())),
            None => Ok(None),
        }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub async fn journal_metadata(&self) -> Value {
        let j = self.journal.lock().await;
        serde_json::json!({"schema_version":SESSION_SCHEMA_VERSION,"backend":self.backend.kind(),"records":j.sequence,"last_checksum":j.checksum})
    }
    pub async fn assert_consistent(&self) -> Result<()> {
        for s in self.list().await? {
            if s.messages.is_empty() {
                bail!("empty session {}", s.id);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Redactor;
    use serde_json::json;

    async fn store(dir: &Path) -> SessionStore {
        let backend = Arc::new(
            FileJournalBackend::open(dir.join("sessions.jsonl"))
                .await
                .unwrap(),
        );
        SessionStore::open_with_backend(backend, Redactor::new(Vec::new()), false)
            .await
            .unwrap()
    }

    async fn seed_session(store: &SessionStore, title: &str) -> Session {
        let handle = store.create_or_get(None, "系统指令".into()).await.unwrap();
        let mut s = handle.lock().await;
        s.title = title.into();
        s.messages
            .push(json!({"role":"user","content":format!("{title} 的提问")}));
        s.messages
            .push(json!({"role":"assistant","content":"回答"}));
        store.commit(&s).await.unwrap();
        s.clone()
    }

    #[tokio::test]
    async fn rename_and_archive_persist_through_journal_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.jsonl");
        let store = store(dir.path()).await;
        let s = seed_session(&store, "原始标题").await;

        let renamed = store.rename(&s.id, "新标题".into()).await.unwrap();
        assert_eq!(renamed.title, "新标题");
        let archived = store.set_archived(&s.id, true).await.unwrap();
        assert!(archived.archived);

        // 重放 journal：元数据变更必须与内存状态一致。
        let reopened = SessionStore::open(&path, Redactor::new(Vec::new()), false)
            .await
            .unwrap();
        let reloaded = reopened.get(&s.id).await.unwrap().unwrap();
        assert_eq!(reloaded.title, "新标题");
        assert!(reloaded.archived);
        assert_eq!(reloaded.messages.len(), 3);
        let summary = reopened
            .list_summaries()
            .await
            .into_iter()
            .find(|v| v.id == s.id)
            .unwrap();
        assert!(summary.archived);
    }

    #[tokio::test]
    async fn delete_cascade_removes_fork_descendants_and_stays_replayable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.jsonl");
        let store = store(dir.path()).await;
        let parent = seed_session(&store, "父会话").await;
        let survivor = seed_session(&store, "无关会话").await;
        let child = store
            .fork(&parent.id, 3, Some("子分支".into()))
            .await
            .unwrap();
        let grandchild = store
            .fork(&child.id, 2, Some("孙分支".into()))
            .await
            .unwrap();

        let removed = store.delete_cascade(&parent.id).await.unwrap();
        assert_eq!(removed.len(), 3);
        assert!(removed.contains(&parent.id));
        assert!(removed.contains(&child.id));
        assert!(removed.contains(&grandchild.id));
        assert!(store.get(&parent.id).await.unwrap().is_none());
        assert!(store.get(&grandchild.id).await.unwrap().is_none());
        assert!(store.get(&survivor.id).await.unwrap().is_some());

        // 重放重写后的 journal：checksum 链完整，幸存会话消息无损。
        let reopened = SessionStore::open(&path, Redactor::new(Vec::new()), false)
            .await
            .unwrap();
        let kept = reopened.get(&survivor.id).await.unwrap().unwrap();
        assert_eq!(kept.title, "无关会话");
        assert_eq!(kept.messages.len(), 3);
        assert_eq!(reopened.list_summaries().await.len(), 1);
    }

    #[tokio::test]
    async fn delete_rejects_running_sessions_and_unknown_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path()).await;
        let s = seed_session(&store, "运行中").await;
        {
            let handle = store.sessions.lock().await.get(&s.id).cloned().unwrap();
            handle.lock().await.status = "running".into();
        }
        assert!(store.delete_cascade(&s.id).await.is_err());
        assert!(store.delete_cascade("missing").await.is_err());
        assert!(store.get(&s.id).await.unwrap().is_some());
    }

    /// 回归：级联删除与并发 checkpoint 提交不得死锁（旧实现的锁顺序反转
    /// 会在“删除持 journal 等会话 / 提交持会话等 journal”处挂死）。
    /// 并发提交可能在删除后复活 journal 条目（commit 的既有语义），
    /// 本测试只断言删除在有界时间内完成且幸存会话无损。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn delete_cascade_concurrent_with_commits_does_not_deadlock() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path()).await;
        let victim = seed_session(&store, "待删除").await;
        let survivor = seed_session(&store, "幸存者").await;

        let commit_store = store.clone();
        let commit_victim = victim.clone();
        let commit_survivor = survivor.clone();
        let committer = tokio::spawn(async move {
            for _ in 0..200 {
                let mut s = commit_victim.clone();
                s.messages.push(json!({"role":"user","content":"并发提交"}));
                let _ = commit_store.commit(&s).await;
                let _ = commit_store.commit(&commit_survivor).await;
                tokio::task::yield_now().await;
            }
        });

        let delete_store = store.clone();
        let target = victim.id.clone();
        let deleted = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            delete_store.delete_cascade(&target),
        )
        .await
        .expect("delete_cascade 不得死锁（10s 超时）");
        committer.abort();
        let _ = committer.await;
        deleted.expect("并发提交场景下删除目标会话应当成功");
        assert!(store.get(&survivor.id).await.unwrap().is_some());
    }

    /// 回归：旧 Meta 字段集（无 archived/project_id）写就的数据目录遗留 journal
    /// 曾导致启动报 "session checksum/sequence mismatch at 1"。现在必须：
    /// 1) 按文件原始键集校验成功并自动迁移成新 schema（full-snapshot 重写）；
    /// 2) 迁移后的会话消息无损；3) 迁移后新写入的校验链自洽、重启可复放。
    #[tokio::test]
    async fn legacy_meta_journal_is_detected_migrated_and_replayable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.jsonl");
        let sid = "42424242-4242-4242-4242-424242424242".to_string();
        // 手工构造旧字段集 record：Meta 不含 archived/project_id 键，
        // checksum 按该键集整体序列化计算。
        let mut legacy_meta = serde_json::Map::new();
        for (key, value) in [
            ("id", json!(sid.clone())),
            ("title", json!("遗留会话")),
            ("created_at", json!(1730000000000u64)),
            ("updated_at", json!(1730000000000u64)),
            ("status", json!("done")),
            ("last_model", json!(serde_json::Value::Null)),
            ("parent_id", json!(serde_json::Value::Null)),
            ("fork_cutoff", json!(serde_json::Value::Null)),
        ] {
            legacy_meta.insert(key.into(), value);
        }
        let payload = json!({
            "schema_version": 2,
            "sequence": 1,
            "previous_checksum": "",
            "metadata": legacy_meta,
            "truncate_to": 0,
            "append_messages": [
                json!({"role":"user","content":"旧问题"}),
                json!({"role":"assistant","content":"旧回答"}),
            ],
        });
        // serde_json 默认 Map = BTreeMap（字母序）。生产写入端用 serde 序列化
        // struct（声明序），这里直接复刻生产端：用声明序的 raw 字符串计算
        // stored digest，避免测试 fixture 因 Map 排序与真实 journal 漂移。
        let payload_raw = {
            let mut raw = String::new();
            raw.push_str("{\"schema_version\":2,\"sequence\":1,\"previous_checksum\":\"\",");
            raw.push_str(&format!(
                "\"metadata\":{}",
                serde_json::to_string(&payload["metadata"]).unwrap()
            ));
            raw.push_str(",\"truncate_to\":0,");
            raw.push_str(&format!(
                "\"append_messages\":{}",
                serde_json::to_string(&payload["append_messages"]).unwrap()
            ));
            raw.push('}');
            raw
        };
        let digest = format!("{:x}", Sha256::digest(payload_raw.as_bytes()));
        // 行字节也按生产端声明序写入：payload_raw 去掉收尾 `}` 然后
        // 接 checksum 字段与 outer 收尾 —— 与 serde flatten 输出一致。
        let mut line = String::new();
        line.push_str(&payload_raw[..payload_raw.len() - 1]); // 去掉 payload 收尾 }
        line.push_str(&format!(",\"checksum\":\"{}\"}}", digest));
        line.push('\n');
        std::fs::write(&path, line).unwrap();

        let store = SessionStore::open(&path, Redactor::new(Vec::new()), false)
            .await
            .unwrap();
        let recovered = store.get(&sid).await.unwrap().expect("legacy 会话应恢复");
        assert_eq!(recovered.title, "遗留会话");
        assert_eq!(recovered.messages.len(), 2);
        assert!(!recovered.archived, "迁移补默认值");

        // 迁移把 journal 重写为新 schema：直接读文件，记录的 metadata 含新键，且可重放恢复。
        let bytes = std::fs::read(&path).unwrap();
        let migrated: Vec<Record> = bytes
            .split(|b| *b == b'\n')
            .filter(|v| !v.is_empty())
            .map(|chunk| serde_json::from_slice(chunk).unwrap())
            .collect();
        assert_eq!(migrated.len(), 1);
        assert!(
            bytes.windows(10).any(|w| w == b"\"archived\""),
            "迁移后应写入 archived 键"
        );
        assert!(
            bytes.windows(12).any(|w| w == b"\"project_id\""),
            "迁移后应写入 project_id 键"
        );

        // 迁移后继续写新提交，重启复放应完全一致。
        let handle = store
            .create_or_get(Some(sid.clone()), "系统指令".into())
            .await
            .unwrap();
        handle
            .lock()
            .await
            .messages
            .push(json!({"role":"user","content":"新提交"}));
        store.commit(&handle.lock().await.clone()).await.unwrap();
        let reopened = SessionStore::open(&path, Redactor::new(Vec::new()), false)
            .await
            .unwrap();
        let after = reopened.get(&sid).await.unwrap().unwrap();
        assert_eq!(after.messages.len(), 3);
        assert_eq!(
            after
                .messages
                .last()
                .and_then(|m| m.get("content"))
                .and_then(Value::as_str),
            Some("新提交")
        );
    }
}
