//! 会话附属面板绑定（浏览器标签 / 终端引用）。浏览器状态是纯前端数据
//! （URL/mode），终端以服务端活体为准（重启后由 /api/term?session_id 反真，
//! 这里只记录引用）。持久化为 data_dir/attachments.json 整表快照，
//! 写入模式与 projects.json 一致：临时文件 + 同步 + 改名发布。

use crate::events::{Redactor, now_ms};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BrowserTabRef {
    pub url: String,
    pub mode: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SessionAttachments {
    /// 附属终端 id（本轮服务运行中的活体；重启后客户端用 /api/term?session_id 反真）。
    #[serde(default)]
    pub terminal_ids: Vec<String>,
    /// 附属浏览器标签（url + mode），按打开顺序。
    #[serde(default)]
    pub browser_tabs: Vec<BrowserTabRef>,
    /// 最后活跃时间（ms），仅用于展示/清理。
    #[serde(default)]
    pub updated_at: u64,
}

#[derive(Default, Clone)]
pub struct AttachmentStore {
    records: Arc<Mutex<BTreeMap<String, SessionAttachments>>>,
    path: PathBuf,
    durable: bool,
}

impl AttachmentStore {
    pub async fn open(path: impl AsRef<Path>, durable: bool) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut records = BTreeMap::new();
        match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let list: BTreeMap<String, SessionAttachments> =
                    serde_json::from_slice(&bytes).context("attachments.json 损坏")?;
                records = list;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(anyhow::Error::new(error).context(format!("read {}", path.display())));
            }
        }
        Ok(Self {
            records: Arc::new(Mutex::new(records)),
            path,
            durable,
        })
    }

    pub async fn get(&self, session_id: &str) -> Option<SessionAttachments> {
        self.records.lock().await.get(session_id).cloned()
    }

    /// 整体替换该会话的附属记录（客户端持有完整视图，PUT 语义）；
    /// 终端 id 列表由服务端 /api/term?session_id 探活后修正。
    pub async fn set(&self, session_id: &str, attachment: SessionAttachments) -> Result<()> {
        let mut guard = self.records.lock().await;
        let mut candidate = guard.clone();
        let mut value = attachment;
        value.updated_at = now_ms();
        candidate.insert(session_id.to_string(), value);
        self.persist(&candidate).await?;
        *guard = candidate;
        Ok(())
    }

    /// 删除会话时清理附属记录（删除返回被删除值）。
    pub async fn remove(&self, session_id: &str) -> Result<Option<SessionAttachments>> {
        let mut guard = self.records.lock().await;
        let mut candidate = guard.clone();
        let removed = candidate.remove(session_id);
        if removed.is_some() {
            self.persist(&candidate).await?;
            *guard = candidate;
        }
        Ok(removed)
    }

    async fn persist(&self, records: &BTreeMap<String, SessionAttachments>) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let body = serde_json::to_vec_pretty(records)?;
        let tmp = self
            .path
            .with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
        {
            let mut file = tokio::fs::File::create(&tmp).await?;
            file.write_all(&body).await?;
            file.flush().await?;
            if self.durable {
                file.sync_all().await?;
            }
        }
        tokio::fs::rename(&tmp, &self.path).await?;
        Ok(())
    }
}

/// URL 查询里可能带 token 等；红actor 只对 URL 文本做模式 redact。
pub fn redact_browser_url(url: &str, redactor: &Redactor) -> String {
    redactor.text(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn attachments_roundtrip_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("attachments.json");
        let store = AttachmentStore::open(&path, false).await.unwrap();
        assert_eq!(store.get("s1").await, None);

        let value = SessionAttachments {
            terminal_ids: vec!["term-1".into()],
            browser_tabs: vec![BrowserTabRef {
                url: "https://example.com/文档".into(),
                mode: "proxy".into(),
            }],
            updated_at: 0,
        };
        store.set("s1", value.clone()).await.unwrap();
        assert_eq!(
            store.get("s1").await.unwrap().browser_tabs,
            value.browser_tabs
        );
        assert!(store.get("s1").await.unwrap().updated_at > 0);

        // 重启（重开文件）后仍在。
        let reopened = AttachmentStore::open(&path, false).await.unwrap();
        assert_eq!(
            reopened.get("s1").await.unwrap().terminal_ids,
            vec!["term-1".to_string()]
        );

        assert!(reopened.remove("missing").await.unwrap().is_none());
        let removed = reopened.remove("s1").await.unwrap().unwrap();
        assert_eq!(removed.terminal_ids, vec!["term-1".to_string()]);
        assert_eq!(reopened.get("s1").await, None);
    }
}
