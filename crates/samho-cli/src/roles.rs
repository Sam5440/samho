//! 任务类型（路由层级）→ 模型覆盖。
//!
//! 运行时的 `TaskRouter` 按复杂度解析出目标层级后，`RoleRouter` 查
//! `model_roles.json` 里该层级绑定的模型并强制改写 `profile.model`，
//! 使「不同类型的任务用不同模型」成为服务端语义（composer 里的显式
//! 模型选择仍然最优先）。每个工作区运行时持有独立的一份。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use samho_core::{ModelDescriptor, ModelRouter, RouteDecision, TaskProfile, TaskRouter};
use serde_json::{Value, json};

/// 模型清单里作为「任务类型默认模型」的 id（不区分大小写）。
const DEFAULT_MODEL_ID: &str = "glm-5.3-flash";

pub(crate) struct ModelRoles {
    /// None = 不落盘（测试用 detached 实例）。
    path: Option<PathBuf>,
    tiers: Vec<String>,
    /// tier id → 模型 id；空串表示该类型跟随自动路由。
    map: std::sync::RwLock<BTreeMap<String, String>>,
    default_model: Option<String>,
}

impl ModelRoles {
    pub(crate) fn detached() -> Arc<Self> {
        Arc::new(Self {
            path: None,
            tiers: Vec::new(),
            map: std::sync::RwLock::new(BTreeMap::new()),
            default_model: None,
        })
    }

    /// 读取持久化文件；缺失的层级补默认模型（GLM-5.3-Flash 若已配置，
    /// 否则空 = 跟随自动路由）。未知层级与未知模型条目直接丢弃。
    pub(crate) fn load(path: PathBuf, config: &samho_core::RuntimeConfig) -> Self {
        let tiers: Vec<String> = config
            .routing
            .tiers
            .iter()
            .map(|tier| tier.id.clone())
            .collect();
        let model_ids: BTreeSet<String> =
            config.models.iter().map(|model| model.id.clone()).collect();
        let default_model = default_model_for(&config.models);
        let mut map = BTreeMap::new();
        if let Some(entries) = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|saved| saved.get("roles").and_then(Value::as_object).cloned())
        {
            for (tier, model) in entries {
                let model = model.as_str().unwrap_or_default().to_string();
                if tiers.contains(&tier) && (model.is_empty() || model_ids.contains(&model)) {
                    map.insert(tier, model);
                }
            }
        }
        for tier in &tiers {
            map.entry(tier.clone())
                .or_insert_with(|| default_model.clone().unwrap_or_default());
        }
        Self {
            path: Some(path),
            tiers,
            map: std::sync::RwLock::new(map),
            default_model,
        }
    }

    /// 某任务类型的模型覆盖；None 或空串 = 跟随自动路由。
    pub(crate) fn override_for(&self, tier: &str) -> Option<String> {
        let map = self.map.read().ok()?;
        map.get(tier).filter(|model| !model.is_empty()).cloned()
    }

    pub(crate) fn snapshot(&self) -> BTreeMap<String, String> {
        self.map.read().map(|map| map.clone()).unwrap_or_default()
    }

    pub(crate) fn tier_ids(&self) -> &[String] {
        &self.tiers
    }

    pub(crate) fn default_model(&self) -> Option<String> {
        self.default_model.clone()
    }

    /// 覆盖整张表（仅接受已知层级；模型 id 由调用方校验）。
    pub(crate) fn apply(&self, roles: BTreeMap<String, String>) -> Result<()> {
        for tier in roles.keys() {
            if !self.tiers.contains(tier) {
                anyhow::bail!("未知任务类型：{tier}");
            }
        }
        let mut map = self
            .map
            .write()
            .map_err(|_| anyhow::anyhow!("角色表锁失败"))?;
        *map = roles;
        Ok(())
    }

    /// temp + rename 原子落盘；detached 实例为 no-op。
    pub(crate) async fn persist(&self) -> Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let payload = json!({ "roles": self.snapshot() }).to_string();
        let parent = path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let temp = parent.join(format!(
            "model_roles.json.tmp-{}",
            uuid::Uuid::new_v4().simple()
        ));
        tokio::fs::create_dir_all(&parent).await?;
        tokio::fs::write(&temp, payload).await?;
        tokio::fs::rename(&temp, path)
            .await
            .with_context(|| format!("发布 {}", path.display()))?;
        Ok(())
    }
}

fn default_model_for(models: &[ModelDescriptor]) -> Option<String> {
    models
        .iter()
        .find(|model| model.id.eq_ignore_ascii_case(DEFAULT_MODEL_ID))
        .map(|model| model.id.clone())
}

/// 包装运行时默认路由器：decide 出结果后按任务类型改绑模型。
/// 任务自带显式模型（composer 选择 / API 指定）时不介入。
pub(crate) struct RoleRouter {
    inner: ModelRouter,
    roles: Arc<ModelRoles>,
}

impl RoleRouter {
    pub(crate) fn new(inner: ModelRouter, roles: Arc<ModelRoles>) -> Self {
        Self { inner, roles }
    }
}

impl TaskRouter for RoleRouter {
    fn classify(&self, prompt: &str, profile: &TaskProfile) -> TaskProfile {
        self.inner.classify(prompt, profile)
    }

    fn decide(&self, task: &TaskProfile, excluded: &[String]) -> Result<RouteDecision> {
        let decision = self.inner.decide(task, excluded)?;
        if task.model.is_some() {
            return Ok(decision);
        }
        let Some(model_id) = self.roles.override_for(&decision.desired_tier_name) else {
            return Ok(decision);
        };
        if model_id == decision.model.id {
            return Ok(decision);
        }
        let mut scoped = task.clone();
        scoped.model = Some(model_id);
        self.inner.decide(&scoped, excluded)
    }
}

/// 生产装配：ModelRoles + RoleRouter 包住配置路由器。
/// serve 默认运行时、静态与动态工作区都走这里，保证角色语义一致。
pub(crate) async fn build_runtime(
    config: samho_core::RuntimeConfig,
) -> Result<(Arc<samho_core::AgentRuntime>, Arc<ModelRoles>)> {
    let roles = Arc::new(ModelRoles::load(
        config.data_dir.join("model_roles.json"),
        &config,
    ));
    let router = Arc::new(RoleRouter::new(
        ModelRouter::new(config.models.clone(), config.routing.clone()),
        roles.clone(),
    ));
    let runtime = samho_core::AgentRuntime::builder(config)
        .router(router)
        .build()
        .await?;
    Ok((runtime, roles))
}

#[cfg(test)]
mod tests {
    use super::*;
    use samho_core::{ModelTier, RuntimeConfig};

    fn test_config(model_tiers: &[(&str, ModelTier)]) -> RuntimeConfig {
        RuntimeConfig {
            models: model_tiers
                .iter()
                .map(|(id, tier)| ModelDescriptor::demo(id, *tier))
                .collect(),
            ..RuntimeConfig::default()
        }
    }

    /// 带 tempdir 的角色表：无持久化文件时全层级种子为空（跟随路由）。
    fn roles_for(config: &RuntimeConfig) -> Arc<ModelRoles> {
        let dir = tempfile::tempdir().unwrap();
        Arc::new(ModelRoles::load(
            dir.path().join("model_roles.json"),
            config,
        ))
    }

    #[test]
    fn role_override_rebinds_route_decision() {
        let config = test_config(&[
            ("demo-nano", ModelTier::Nano),
            ("demo-big", ModelTier::Frontier),
        ]);
        let roles = roles_for(&config);
        roles
            .apply([("nano".into(), "demo-big".into())].into_iter().collect())
            .unwrap();
        let router = RoleRouter::new(
            ModelRouter::new(config.models.clone(), config.routing.clone()),
            roles,
        );
        let decision = router.decide(&TaskProfile::classify(0), &[]).unwrap();
        assert_eq!(decision.model.id, "demo-big");
    }

    #[test]
    fn explicit_task_model_beats_role() {
        let config = test_config(&[
            ("demo-nano", ModelTier::Nano),
            ("demo-big", ModelTier::Frontier),
        ]);
        let roles = roles_for(&config);
        roles
            .apply([("nano".into(), "demo-big".into())].into_iter().collect())
            .unwrap();
        let router = RoleRouter::new(
            ModelRouter::new(config.models.clone(), config.routing.clone()),
            roles,
        );
        let task = TaskProfile {
            model: Some("demo-nano".into()),
            ..TaskProfile::classify(0)
        };
        assert_eq!(router.decide(&task, &[]).unwrap().model.id, "demo-nano");
    }

    #[test]
    fn empty_role_falls_through_to_router() {
        let config = test_config(&[
            ("demo-nano", ModelTier::Nano),
            ("demo-big", ModelTier::Frontier),
        ]);
        let roles = roles_for(&config);
        roles
            .apply([("nano".into(), String::new())].into_iter().collect())
            .unwrap();
        let router = RoleRouter::new(
            ModelRouter::new(config.models.clone(), config.routing.clone()),
            roles,
        );
        assert_eq!(
            router
                .decide(&TaskProfile::classify(0), &[])
                .unwrap()
                .model
                .id,
            "demo-nano"
        );
    }

    #[tokio::test]
    async fn load_seeds_default_persists_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model_roles.json");
        let mut config = test_config(&[
            ("glm-5.3-flash", ModelTier::Balanced),
            ("demo-nano", ModelTier::Nano),
        ]);
        config.data_dir = dir.path().to_path_buf();
        let roles = ModelRoles::load(path.clone(), &config);
        // 默认全部指向 glm-5.3-flash。
        for tier in roles.tier_ids() {
            assert_eq!(roles.override_for(tier).as_deref(), Some("glm-5.3-flash"));
        }
        roles
            .apply(
                [
                    ("nano".into(), "demo-nano".into()),
                    ("fast".into(), String::new()),
                ]
                .into_iter()
                .collect(),
            )
            .unwrap();
        roles.persist().await.unwrap();

        let reloaded = ModelRoles::load(path.clone(), &config);
        assert_eq!(reloaded.override_for("nano").as_deref(), Some("demo-nano"));
        // 空串 = 跟随自动路由，override_for 返回 None。
        assert_eq!(reloaded.override_for("fast"), None);
        assert_eq!(
            reloaded.override_for("balanced").as_deref(),
            Some("glm-5.3-flash")
        );
        assert!(
            reloaded
                .apply([("nope".into(), String::new())].into_iter().collect())
                .is_err()
        );
        // 未知模型条目在读取时被丢弃，回落默认。
        let mut broken = serde_json::Map::new();
        for tier in reloaded.tier_ids() {
            broken.insert(tier.clone(), json!("no-such-model"));
        }
        std::fs::write(
            &path,
            serde_json::to_string(&json!({ "roles": broken })).unwrap(),
        )
        .unwrap();
        let healed = ModelRoles::load(dir.path().join("model_roles.json"), &config);
        assert_eq!(
            healed.override_for("nano").as_deref(),
            Some("glm-5.3-flash")
        );
    }
}
