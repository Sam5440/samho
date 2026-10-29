use crate::events::now_ms;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
    pub created_at: u64,
}

pub fn validate_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 64 {
        bail!("项目名称不能为空且不超过 64 字符");
    }
    Ok(trimmed.to_string())
}

pub fn validate_color(color: Option<String>) -> Result<Option<String>> {
    match color {
        None => Ok(None),
        Some(value) if value.trim().is_empty() => Ok(None),
        Some(value) => {
            let hex = value
                .trim()
                .strip_prefix('#')
                .context("颜色须为 #rrggbb 形式")?;
            if hex.len() != 6 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                bail!("颜色须为 #rrggbb 形式");
            }
            Ok(Some(format!("#{hex}").to_lowercase()))
        }
    }
}

/// 项目清单（会话分类的宿主）。持久化为 data_dir/projects.json 整表快照，
/// 写入模式与仓库规范一致：临时文件 + 同步 + 改名发布。
#[derive(Clone)]
pub struct ProjectStore {
    projects: Arc<Mutex<BTreeMap<String, Project>>>,
    path: PathBuf,
    durable: bool,
}

impl ProjectStore {
    pub async fn open(path: impl AsRef<Path>, durable: bool) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut projects = BTreeMap::new();
        if let Ok(bytes) = tokio::fs::read(&path).await {
            let list: Vec<Project> =
                serde_json::from_slice(&bytes).context("projects.json 损坏")?;
            for project in list {
                projects.insert(project.id.clone(), project);
            }
        }
        Ok(Self {
            projects: Arc::new(Mutex::new(projects)),
            path,
            durable,
        })
    }

    pub async fn list(&self) -> Vec<Project> {
        self.projects.lock().await.values().cloned().collect()
    }

    pub async fn get(&self, id: &str) -> Option<Project> {
        self.projects.lock().await.get(id).cloned()
    }

    pub async fn create(&self, name: String, color: Option<String>) -> Result<Project> {
        let name = validate_name(&name)?;
        let color = validate_color(color)?;
        let mut guard = self.projects.lock().await;
        if guard.values().any(|p| p.name == name) {
            bail!("同名项目已存在");
        }
        let project = Project {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            color,
            created_at: now_ms(),
        };
        guard.insert(project.id.clone(), project.clone());
        self.persist(&guard).await?;
        Ok(project)
    }

    /// name=None 保持；color=Some(None) 清除颜色、Some(Some(hex)) 设置。
    pub async fn update(
        &self,
        id: &str,
        name: Option<String>,
        color: Option<Option<String>>,
    ) -> Result<Project> {
        let mut guard = self.projects.lock().await;
        if let Some(name) = name {
            let name = validate_name(&name)?;
            if guard.values().any(|p| p.id != id && p.name == name) {
                bail!("同名项目已存在");
            }
            guard.get_mut(id).context("项目不存在")?.name = name;
        }
        if let Some(color) = color {
            let color = validate_color(color)?;
            guard.get_mut(id).context("项目不存在")?.color = color;
        }
        let project = guard.get(id).context("项目不存在")?.clone();
        self.persist(&guard).await?;
        Ok(project)
    }

    pub async fn delete(&self, id: &str) -> Result<Project> {
        let mut guard = self.projects.lock().await;
        let project = guard.remove(id).context("项目不存在")?;
        self.persist(&guard).await?;
        Ok(project)
    }

    async fn persist(&self, projects: &BTreeMap<String, Project>) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let body = serde_json::to_vec_pretty(&projects.values().collect::<Vec<_>>())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn persists_and_validates_projects() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projects.json");
        let store = ProjectStore::open(&path, false).await.unwrap();
        assert!(store.list().await.is_empty());

        let project = store
            .create(" 前端重构 ".into(), Some("#7FAE32".into()))
            .await
            .unwrap();
        assert_eq!(project.name, "前端重构");
        assert_eq!(project.color.as_deref(), Some("#7fae32"));

        assert!(store.create("前端重构".into(), None).await.is_err());
        assert!(store.create("".into(), None).await.is_err());
        assert!(store.create("x".repeat(65), None).await.is_err());
        assert!(
            store
                .update(&project.id, None, Some(Some("blue".into())))
                .await
                .is_err()
        );

        let renamed = store
            .update(&project.id, Some("评测基准".into()), Some(None))
            .await
            .unwrap();
        assert_eq!(renamed.name, "评测基准");
        assert!(renamed.color.is_none());

        // 重启（重开文件）后仍在。
        let reopened = ProjectStore::open(&path, false).await.unwrap();
        assert_eq!(reopened.list().await.len(), 1);
        assert_eq!(reopened.get(&project.id).await.unwrap().name, "评测基准");

        reopened.delete(&project.id).await.unwrap();
        assert!(reopened.list().await.is_empty());
        assert!(reopened.delete(&project.id).await.is_err());
    }
}
