use crate::session::JournalBackend;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, broadcast};

pub type TraceId = String;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraceEvent {
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub trace_id: String,
    pub session_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub kind: String,
    pub object_id: Option<String>,
    pub payload: Value,
}
impl TraceEvent {
    pub fn new(
        trace_id: &str,
        session_id: &str,
        span_id: &str,
        kind: &str,
        payload: Value,
    ) -> Self {
        Self {
            sequence: 0,
            timestamp_ms: now_ms(),
            trace_id: trace_id.into(),
            session_id: session_id.into(),
            span_id: span_id.into(),
            parent_span_id: None,
            kind: kind.into(),
            object_id: None,
            payload,
        }
    }
}
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Clone)]
pub struct Redactor {
    secrets: Arc<Vec<String>>,
    pattern: Arc<regex::Regex>,
}
impl Redactor {
    pub fn from_env() -> Self {
        let secrets = std::env::vars()
            .filter(|(key, value)| {
                let key = key.to_ascii_lowercase();
                value.len() >= 8
                    && (key.contains("api_key")
                        || key.contains("token")
                        || key.contains("secret")
                        || key.contains("password"))
            })
            .map(|(_, value)| value)
            .collect();
        Self::new(secrets)
    }
    pub fn new(secrets: Vec<String>) -> Self {
        Self {
            secrets: Arc::new(secrets),
            pattern: Arc::new(
                regex::Regex::new(r"(?i)(Bearer\s+[a-z0-9._~+/=-]+|sk-[a-z0-9_-]{8,})")
                    .expect("redaction regex"),
            ),
        }
    }
    pub fn text(&self, text: &str) -> String {
        let mut text = text.to_string();
        for secret in self.secrets.iter() {
            if !secret.is_empty() {
                text = text.replace(secret, "[REDACTED]");
            }
        }
        self.pattern.replace_all(&text, "[REDACTED]").into_owned()
    }
    /// Redact cumulative streaming snapshots without releasing a split credential prefix.
    pub fn streaming_text(&self, text: &str, finished: bool) -> String {
        if finished {
            return self.text(text);
        }
        let mut safe_end = text
            .char_indices()
            .rev()
            .find(|(_, character)| character.is_whitespace())
            .map_or(0, |(offset, character)| offset + character.len_utf8());
        for secret in self.secrets.iter().filter(|secret| !secret.is_empty()) {
            for (prefix_len, _) in secret.char_indices().skip(1) {
                if text.ends_with(&secret[..prefix_len]) {
                    safe_end = safe_end.min(text.len() - prefix_len);
                }
            }
        }
        let candidate = text[..safe_end].trim_end();
        if candidate.to_ascii_lowercase().ends_with("bearer") {
            safe_end = safe_end.min(candidate.len() - "bearer".len());
        }
        self.text(&text[..safe_end])
    }
    pub fn value(&self, value: Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.text(&text)),
            Value::Array(items) => {
                Value::Array(items.into_iter().map(|item| self.value(item)).collect())
            }
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(key, value)| {
                        let lower = key.to_ascii_lowercase();
                        let sensitive = [
                            "authorization",
                            "api_key",
                            "apikey",
                            "password",
                            "secret",
                            "access_token",
                            "refresh_token",
                            "bearer_token",
                        ]
                        .contains(&lower.as_str());
                        (
                            key,
                            if sensitive {
                                Value::String("[REDACTED]".into())
                            } else {
                                self.value(value)
                            },
                        )
                    })
                    .collect(),
            ),
            value => value,
        }
    }
}

pub const TRACE_SCHEMA_VERSION: u32 = 1;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EventLogOptions {
    pub rotation_bytes: u64,
    pub max_archives: usize,
}
impl Default for EventLogOptions {
    fn default() -> Self {
        Self {
            rotation_bytes: 0,
            max_archives: 4,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EventQuery {
    pub after_sequence: u64,
    pub limit: usize,
    pub trace_id: Option<String>,
    pub session_id: Option<String>,
    pub kind: Option<String>,
}
impl Default for EventQuery {
    fn default() -> Self {
        Self {
            after_sequence: 0,
            limit: 100,
            trace_id: None,
            session_id: None,
            kind: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventPage {
    pub events: Vec<TraceEvent>,
    pub next_cursor: Option<u64>,
    pub has_more: bool,
    pub first_available_sequence: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpanTimeline {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub object_id: Option<String>,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub start_ms: u64,
    pub end_ms: u64,
    pub duration_ms: u64,
    pub event_count: usize,
    pub kinds: Vec<String>,
}
struct Segment {
    id: u64,
    reader: StdMutex<std::fs::File>,
}
#[derive(Clone)]
enum RecordLocation {
    File(Arc<Segment>, u64, usize),
    Memory(Arc<TraceEvent>),
    Backend(usize),
}
#[derive(Clone)]
struct IndexEntry {
    trace_id: String,
    session_id: String,
    kind: String,
    location: RecordLocation,
}
impl IndexEntry {
    fn from_event(e: &TraceEvent, location: RecordLocation) -> Self {
        Self {
            trace_id: e.trace_id.clone(),
            session_id: e.session_id.clone(),
            kind: e.kind.clone(),
            location,
        }
    }
    fn matches(&self, q: &EventQuery) -> bool {
        q.trace_id.as_ref().is_none_or(|v| v == &self.trace_id)
            && q.session_id.as_ref().is_none_or(|v| v == &self.session_id)
            && q.kind.as_ref().is_none_or(|v| v == &self.kind)
    }
}
struct Archive {
    path: PathBuf,
    id: u64,
}
struct Inner {
    path: Option<PathBuf>,
    file: Option<tokio::fs::File>,
    backend: Option<Arc<dyn JournalBackend>>,
    next_sequence: u64,
    event_count: u64,
    current_bytes: u64,
    current_segment: Option<Arc<Segment>>,
    archives: Vec<Archive>,
    options: EventLogOptions,
}
#[derive(Clone)]
pub struct EventLog {
    inner: Arc<Mutex<Inner>>,
    index: Arc<RwLock<BTreeMap<u64, IndexEntry>>>,
    sender: broadcast::Sender<TraceEvent>,
    redactor: Redactor,
    durable: bool,
}
fn index_record(
    index: &mut BTreeMap<u64, IndexEntry>,
    next: &mut u64,
    line: &[u8],
    location: RecordLocation,
) -> Result<()> {
    let event: TraceEvent =
        serde_json::from_slice(line).context("corrupt interior trace record")?;
    if event.sequence < *next {
        bail!("non-monotonic trace sequence");
    }
    *next = event
        .sequence
        .checked_add(1)
        .context("trace sequence overflow")?;
    index.insert(event.sequence, IndexEntry::from_event(&event, location));
    Ok(())
}
impl EventLog {
    pub async fn open(path: impl AsRef<Path>, durable: bool, redactor: Redactor) -> Result<Self> {
        Self::open_with_options(path, EventLogOptions::default(), durable, redactor).await
    }
    pub async fn open_with_options(
        path: impl AsRef<Path>,
        options: EventLogOptions,
        durable: bool,
        redactor: Redactor,
    ) -> Result<Self> {
        if options.rotation_bytes > 0 && options.max_archives == 0 {
            bail!("rotation requires at least one retained archive");
        }
        let path = path.as_ref().to_path_buf();
        let parent = path.parent().unwrap_or(Path::new("."));
        tokio::fs::create_dir_all(parent).await?;
        let name = path
            .file_name()
            .context("event path needs filename")?
            .to_string_lossy()
            .into_owned();
        let prefix = format!("{name}.segment-");
        let mut archived = Vec::new();
        let mut directory = tokio::fs::read_dir(parent).await?;
        while let Some(entry) = directory.next_entry().await? {
            let filename = entry.file_name().to_string_lossy().into_owned();
            if let Some(id) = filename
                .strip_prefix(&prefix)
                .and_then(|s| s.parse::<u64>().ok())
            {
                archived.push(Archive {
                    path: entry.path(),
                    id,
                });
            }
        }
        archived.sort_by_key(|s| s.id);
        if !path.exists() {
            tokio::fs::write(&path, []).await?;
        }
        let active_id = archived.last().map_or(1, |s| s.id + 1);
        let mut sources: Vec<_> = archived
            .iter()
            .map(|s| (s.path.clone(), s.id, false))
            .collect();
        sources.push((path.clone(), active_id, true));
        let mut index = BTreeMap::new();
        let mut next = 1;
        let mut current_segment = None;
        let mut current_bytes = 0;
        let mut repair = None;
        for (source, id, active) in sources {
            let bytes = tokio::fs::read(&source).await?;
            let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |p| p + 1);
            if !active && end != bytes.len() {
                bail!("unterminated archived trace record");
            }
            let segment = Arc::new(Segment {
                id,
                reader: StdMutex::new(std::fs::File::open(&source)?),
            });
            let mut offset = 0;
            for line in bytes[..end].split_inclusive(|b| *b == b'\n') {
                let payload = &line[..line.len() - 1];
                if !payload.is_empty() {
                    index_record(
                        &mut index,
                        &mut next,
                        payload,
                        RecordLocation::File(segment.clone(), offset, payload.len()),
                    )?;
                }
                offset += line.len() as u64;
            }
            if active {
                current_segment = Some(segment);
                current_bytes = end as u64;
                if end < bytes.len() {
                    repair = Some(end as u64);
                }
            }
        }
        // Interior validation finishes before any recovery write.
        if let Some(end) = repair {
            tokio::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .await?
                .set_len(end)
                .await?;
        }
        let file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await?;
        let count = index.len() as u64;
        let (sender, _) = broadcast::channel(256);
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                path: Some(path),
                file: Some(file),
                backend: None,
                next_sequence: next,
                event_count: count,
                current_bytes,
                current_segment,
                archives: archived,
                options,
            })),
            index: Arc::new(RwLock::new(index)),
            sender,
            redactor,
            durable,
        })
    }
    /// Custom and SQLite journals use their row order; file mode uses byte-offset indexing.
    pub async fn open_with_backend(
        backend: Arc<dyn JournalBackend>,
        options: EventLogOptions,
        durable: bool,
        redactor: Redactor,
    ) -> Result<Self> {
        if options.rotation_bytes != 0 {
            bail!(
                "custom journal rotation must be implemented by its backend; use rotation_bytes=0"
            );
        }
        let records = backend.load().await?;
        let mut index = BTreeMap::new();
        let mut next = 1;
        for (ordinal, line) in records.iter().enumerate() {
            index_record(
                &mut index,
                &mut next,
                line,
                RecordLocation::Backend(ordinal),
            )?;
        }
        backend.finish_recovery().await?;
        let (sender, _) = broadcast::channel(256);
        let count = index.len() as u64;
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                path: None,
                file: None,
                backend: Some(backend),
                next_sequence: next,
                event_count: count,
                current_bytes: 0,
                current_segment: None,
                archives: Vec::new(),
                options,
            })),
            index: Arc::new(RwLock::new(index)),
            sender,
            redactor,
            durable,
        })
    }
    pub fn memory() -> Self {
        let (sender, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                path: None,
                file: None,
                backend: None,
                next_sequence: 1,
                event_count: 0,
                current_bytes: 0,
                current_segment: None,
                archives: Vec::new(),
                options: EventLogOptions::default(),
            })),
            index: Arc::new(RwLock::new(BTreeMap::new())),
            sender,
            redactor: Redactor::new(Vec::new()),
            durable: false,
        }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<TraceEvent> {
        self.sender.subscribe()
    }
    pub fn redactor(&self) -> &Redactor {
        &self.redactor
    }
    pub async fn append(&self, mut event: TraceEvent) -> Result<u64> {
        let mut inner = self.inner.lock().await;
        event.sequence = inner.next_sequence;
        event.payload = self.redactor.value(event.payload);
        let mut bytes = serde_json::to_vec(&event)?;
        let payload_length = bytes.len();
        bytes.push(b'\n');
        if inner.path.is_some()
            && inner.options.rotation_bytes > 0
            && inner.current_bytes > 0
            && inner.current_bytes + bytes.len() as u64 > inner.options.rotation_bytes
        {
            self.rotate_locked(&mut inner).await?;
        }
        let location = if inner.file.is_some() {
            let offset = inner.current_bytes;
            let file = inner.file.as_mut().expect("checked file");
            if let Err(error) = async {
                file.write_all(&bytes).await?;
                file.flush().await?;
                if self.durable {
                    file.sync_data().await?;
                }
                Ok::<_, std::io::Error>(())
            }
            .await
            {
                let _ = file.set_len(offset).await;
                return Err(error.into());
            }
            inner.current_bytes += bytes.len() as u64;
            RecordLocation::File(
                inner.current_segment.clone().expect("file segment"),
                offset,
                payload_length,
            )
        } else if let Some(backend) = &inner.backend {
            backend
                .append(&bytes[..payload_length], self.durable)
                .await?;
            RecordLocation::Backend(inner.event_count as usize)
        } else {
            RecordLocation::Memory(Arc::new(event.clone()))
        };
        self.index
            .write()
            .map_err(|_| anyhow::anyhow!("event index poisoned"))?
            .insert(event.sequence, IndexEntry::from_event(&event, location));
        inner.next_sequence = inner
            .next_sequence
            .checked_add(1)
            .context("trace sequence overflow")?;
        inner.event_count += 1;
        let _ = self.sender.send(event.clone());
        Ok(event.sequence)
    }
    async fn rotate_locked(&self, inner: &mut Inner) -> Result<()> {
        let path = inner.path.clone().context("not a file journal")?;
        let id = inner
            .current_segment
            .as_ref()
            .context("missing segment")?
            .id;
        let target = path.with_file_name(format!(
            "{}.segment-{id:020}",
            path.file_name()
                .context("missing filename")?
                .to_string_lossy()
        ));
        if let Some(file) = inner.file.as_mut() {
            file.flush().await?;
            if self.durable {
                file.sync_all().await?;
            }
        }
        drop(inner.file.take());
        if let Err(e) = tokio::fs::rename(&path, &target).await {
            inner.file = Some(
                tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .await?,
            );
            return Err(e.into());
        }
        let file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .await?;
        let segment = Arc::new(Segment {
            id: id + 1,
            reader: StdMutex::new(std::fs::File::open(&path)?),
        });
        inner.file = Some(file);
        inner.current_segment = Some(segment);
        inner.current_bytes = 0;
        inner.archives.push(Archive { path: target, id });
        while inner.archives.len() > inner.options.max_archives {
            let old = &inner.archives[0];
            tokio::fs::remove_file(&old.path).await?;
            let retired = old.id;
            self.index
                .write()
                .map_err(|_| anyhow::anyhow!("event index poisoned"))?
                .retain(|_, e| !matches!(&e.location,RecordLocation::File(s,_,_) if s.id==retired));
            inner.archives.remove(0);
        }
        inner.event_count = self
            .index
            .read()
            .map_err(|_| anyhow::anyhow!("event index poisoned"))?
            .len() as u64;
        Ok(())
    }
    pub async fn len(&self) -> u64 {
        self.inner.lock().await.event_count
    }
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
    pub async fn query(&self, query: EventQuery) -> Result<EventPage> {
        if query.limit == 0 || query.limit > 10_000 {
            bail!("event page limit must be 1..10000");
        }
        let (selected, first) = {
            let index = self
                .index
                .read()
                .map_err(|_| anyhow::anyhow!("event index poisoned"))?;
            let selected = index
                .range((
                    std::ops::Bound::Excluded(query.after_sequence),
                    std::ops::Bound::Unbounded,
                ))
                .filter(|(_, e)| e.matches(&query))
                .take(query.limit + 1)
                .map(|(s, e)| (*s, e.clone()))
                .collect::<Vec<_>>();
            (selected, index.first_key_value().map(|(s, _)| *s))
        };
        let more = selected.len() > query.limit;
        let selected = selected.into_iter().take(query.limit).collect::<Vec<_>>();
        let backend = { self.inner.lock().await.backend.clone() };
        let backend_records = if selected
            .iter()
            .any(|(_, e)| matches!(e.location, RecordLocation::Backend(_)))
        {
            Some(backend.context("missing journal backend")?.load().await?)
        } else {
            None
        };
        let events = tokio::task::spawn_blocking(move || -> Result<Vec<TraceEvent>> {
            use std::io::{Read, Seek, SeekFrom};
            let mut events = Vec::with_capacity(selected.len());
            for (sequence, entry) in selected {
                let event = match entry.location {
                    RecordLocation::Memory(e) => (*e).clone(),
                    RecordLocation::Backend(i) => serde_json::from_slice(
                        backend_records
                            .as_ref()
                            .and_then(|r| r.get(i))
                            .context("journal index row missing")?,
                    )?,
                    RecordLocation::File(segment, offset, len) => {
                        let mut file = segment
                            .reader
                            .lock()
                            .map_err(|_| anyhow::anyhow!("segment reader poisoned"))?;
                        file.seek(SeekFrom::Start(offset))?;
                        let mut bytes = vec![0; len];
                        file.read_exact(&mut bytes)?;
                        serde_json::from_slice(&bytes)?
                    }
                };
                if event.sequence != sequence {
                    bail!("event index/journal mismatch");
                }
                events.push(event);
            }
            Ok(events)
        })
        .await??;
        let next_cursor = events.last().map(|e| e.sequence);
        Ok(EventPage {
            events,
            next_cursor,
            has_more: more,
            first_available_sequence: first,
        })
    }
    pub async fn events(&self, trace_id: Option<&str>) -> Result<Vec<TraceEvent>> {
        let mut cursor = 0;
        let mut events = Vec::new();
        loop {
            let page = self
                .query(EventQuery {
                    after_sequence: cursor,
                    limit: 1000,
                    trace_id: trace_id.map(str::to_owned),
                    ..Default::default()
                })
                .await?;
            if let Some(next) = page.next_cursor {
                cursor = next;
            }
            events.extend(page.events);
            if !page.has_more {
                break;
            }
        }
        Ok(events)
    }
    pub async fn export_jsonl(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let (watermark, source, backend_source) = {
            let i = self.inner.lock().await;
            (
                i.next_sequence - 1,
                i.path.clone(),
                i.backend.as_ref().map(|b| b.location()),
            )
        };
        if source.as_deref() == Some(path) || backend_source.as_deref() == Some(path) {
            bail!("cannot export over the live journal");
        }
        let temp = path.with_extension(format!("export-{}", uuid::Uuid::new_v4()));
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .await?;
        let mut cursor = 0;
        loop {
            let page = self
                .query(EventQuery {
                    after_sequence: cursor,
                    limit: 1000,
                    ..Default::default()
                })
                .await?;
            for e in &page.events {
                if e.sequence > watermark {
                    break;
                }
                file.write_all(&serde_json::to_vec(e)?).await?;
                file.write_all(b"\n").await?;
                cursor = e.sequence;
            }
            if !page.has_more || page.events.last().is_none_or(|e| e.sequence >= watermark) {
                break;
            }
        }
        file.flush().await?;
        if self.durable {
            file.sync_all().await?;
        }
        drop(file);
        tokio::fs::rename(temp, path).await?;
        Ok(())
    }
    pub async fn timeline(&self, trace_id: &str) -> Result<Vec<SpanTimeline>> {
        let mut spans = BTreeMap::<String, SpanTimeline>::new();
        for event in self.events(Some(trace_id)).await? {
            let span = spans
                .entry(event.span_id.clone())
                .or_insert_with(|| SpanTimeline {
                    trace_id: trace_id.into(),
                    span_id: event.span_id.clone(),
                    parent_span_id: event.parent_span_id.clone(),
                    object_id: event.object_id.clone(),
                    first_sequence: event.sequence,
                    last_sequence: event.sequence,
                    start_ms: event.timestamp_ms,
                    end_ms: event.timestamp_ms,
                    duration_ms: 0,
                    event_count: 0,
                    kinds: Vec::new(),
                });
            span.last_sequence = event.sequence;
            span.start_ms = span.start_ms.min(event.timestamp_ms);
            span.end_ms = span.end_ms.max(event.timestamp_ms);
            span.duration_ms = span.end_ms.saturating_sub(span.start_ms);
            span.event_count += 1;
            if !span.kinds.contains(&event.kind) {
                span.kinds.push(event.kind);
            }
        }
        let mut out: Vec<_> = spans.into_values().collect();
        out.sort_by_key(|s| s.first_sequence);
        Ok(out)
    }
    pub async fn schema_metadata(&self) -> Value {
        let inner = self.inner.lock().await;
        serde_json::json!({"schema_version":TRACE_SCHEMA_VERSION,"record_format":"TraceEvent JSONL","cursor":"exclusive sequence","next_sequence":inner.next_sequence,
            "retained_events":inner.event_count,"rotation_bytes":inner.options.rotation_bytes,"max_archives":inner.options.max_archives,
            "query_backend":if inner.path.is_some(){"file_byte_offset_index"}else if inner.backend.is_some(){"journal_rows"}else{"memory"},
            "duration_clock":"wall clock; timeline duration is observational"})
    }
}
