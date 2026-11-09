use crate::model::{ModelDescriptor, ModelTier};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeConfig {
    pub workspace: PathBuf,
    pub data_dir: PathBuf,
    pub http_bind: String,
    pub system_prompt: String,
    pub max_turns: u32,
    pub max_concurrent_tasks: usize,
    pub request_timeout_ms: u64,
    pub max_tool_output_bytes: usize,
    pub log_content: bool,
    pub durable_logs: bool,
    pub skill_roots: Vec<PathBuf>,
    pub models: Vec<ModelDescriptor>,
    pub routing: RoutingConfig,
    pub tools: ToolPolicy,
    pub mcp_servers: Vec<crate::mcp::McpServerConfig>,
    pub streaming: bool,
    pub stream_buffer: usize,
    pub context: crate::context::ContextConfig,
    pub retry: RetryConfig,
    pub budget: BudgetConfig,
    pub storage: StorageConfig,
    pub logs: LogConfig,
    pub ui: UiConfig,
    pub workspaces: Vec<WorkspaceConfig>,
    pub network: crate::tools::NetworkPolicyConfig,
    pub remote_facets: Vec<crate::facet::FacetConfig>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RoutingConfig {
    pub rules: Vec<RoutingRule>,
    pub tier_penalty: f64,
    pub latency_weight: f64,
    pub cost_weight: f64,
    pub output_reserve_tokens: u32,
    pub tiers: Vec<TierDefinition>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TierDefinition {
    pub id: String,
    pub rank: i32,
    pub min_complexity: u8,
    pub max_complexity: u8,
    #[serde(default)]
    pub requires_reasoning: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub reroute: bool,
}
impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff_ms: 100,
            max_backoff_ms: 5000,
            reroute: true,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BudgetConfig {
    pub max_task_usd: Option<f64>,
    pub max_total_usd: Option<f64>,
    pub max_task_tokens: u64,
    pub max_total_tokens: Option<u64>,
}
impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_task_usd: None,
            max_total_usd: None,
            max_task_tokens: 1_000_000,
            max_total_tokens: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    pub backend: String,
}
impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: "file".into(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogConfig {
    pub rotation_bytes: u64,
    pub max_archives: usize,
}
impl Default for LogConfig {
    fn default() -> Self {
        Self {
            rotation_bytes: 64 * 1024 * 1024,
            max_archives: 8,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    pub theme: String,
    pub submit_key: String,
    pub new_session_key: String,
    pub cancel_key: String,
}
impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            submit_key: "Ctrl+Enter".into(),
            new_session_key: "Alt+N".into(),
            cancel_key: "Ctrl+K".into(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub data_dir: PathBuf,
}
impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            rules: vec![
                RoutingRule {
                    keywords: vec![
                        "architecture".into(),
                        "proof".into(),
                        "重写".into(),
                        "架构".into(),
                        "证明".into(),
                    ],
                    complexity: 9,
                },
                RoutingRule {
                    keywords: vec![
                        "debug".into(),
                        "refactor".into(),
                        "debugging".into(),
                        "调试".into(),
                        "重构".into(),
                    ],
                    complexity: 7,
                },
                RoutingRule {
                    keywords: vec![
                        "translate".into(),
                        "summarize".into(),
                        "翻译".into(),
                        "摘要".into(),
                    ],
                    complexity: 2,
                },
            ],
            tier_penalty: 20.0,
            latency_weight: 0.001,
            cost_weight: 5.0,
            output_reserve_tokens: 1024,
            tiers: [
                ("nano", 0, 0, 1, false),
                ("fast", 1, 2, 3, false),
                ("balanced", 2, 4, 5, false),
                ("reasoning", 3, 6, 7, true),
                ("frontier", 4, 8, 10, false),
                ("specialist", 2, 255, 255, false),
            ]
            .into_iter()
            .map(
                |(id, rank, min_complexity, max_complexity, requires_reasoning)| TierDefinition {
                    id: id.into(),
                    rank,
                    min_complexity,
                    max_complexity,
                    requires_reasoning,
                },
            )
            .collect(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingRule {
    pub keywords: Vec<String>,
    pub complexity: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolPolicy {
    pub read: bool,
    pub write: bool,
    pub edit: bool,
    pub list_files: bool,
    pub glob: bool,
    pub grep: bool,
    pub webfetch: bool,
    pub todo: bool,
    pub shell: bool,
    pub mcp: bool,
    pub shell_timeout_ms: u64,
}
impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            read: true,
            write: true,
            edit: true,
            list_files: true,
            glob: true,
            grep: true,
            webfetch: true,
            todo: true,
            shell: true,
            mcp: true,
            shell_timeout_ms: 30_000,
        }
    }
}
impl Default for RuntimeConfig {
    fn default() -> Self {
        let models = [
            ("demo-nano", ModelTier::Nano),
            ("demo-fast", ModelTier::Fast),
            ("demo-balanced", ModelTier::Balanced),
            ("demo-reasoning", ModelTier::Reasoning),
            ("demo-frontier", ModelTier::Frontier),
            ("demo-specialist", ModelTier::Specialist),
        ]
        .into_iter()
        .map(|(id, tier)| ModelDescriptor::demo(id, tier))
        .collect();
        Self { workspace: ".".into(), data_dir: ".rpi".into(), http_bind: "127.0.0.1:7432".into(),
            system_prompt: "You are a coding assistant. Use tools when needed. Skills are workflow guidance. Report failures accurately.".into(),
            max_turns: 20, max_concurrent_tasks: 4, request_timeout_ms: 120_000,
            max_tool_output_bytes: 64 * 1024, log_content: false, durable_logs: true,
            skill_roots: vec![".agents/skills".into(), ".pi/skills".into()],
            models, routing: RoutingConfig::default(), tools: ToolPolicy::default(), mcp_servers: Vec::new(),
            streaming: true, stream_buffer: 32, context: crate::context::ContextConfig::default(), retry: RetryConfig::default(),
            budget: BudgetConfig::default(), storage: StorageConfig::default(), logs: LogConfig::default(), ui: UiConfig::default(), workspaces: Vec::new(), network: crate::tools::NetworkPolicyConfig::default(), remote_facets: Vec::new() }
    }
}
impl RuntimeConfig {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut config: Self = match path {
            Some(path) => toml::from_str(
                &std::fs::read_to_string(path)
                    .with_context(|| format!("read {}", path.display()))?,
            )?,
            None => Self::default(),
        };
        if let Ok(value) = std::env::var("RPI_HTTP_BIND") {
            config.http_bind = value;
        }
        if let Ok(value) = std::env::var("RPI_WORKSPACE") {
            config.workspace = value.into();
        }
        if let Ok(value) = std::env::var("RPI_DATA_DIR") {
            config.data_dir = value.into();
        }
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        if self.models.is_empty() {
            bail!("at least one model is required");
        }
        if self.max_turns == 0
            || self.max_concurrent_tasks == 0
            || self.request_timeout_ms == 0
            || self.max_tool_output_bytes == 0
            || self.stream_buffer == 0
            || self.stream_buffer > 65536
            || self.retry.max_attempts == 0
            || self.retry.max_attempts > 100
            || self.retry.max_backoff_ms < self.retry.initial_backoff_ms
            || self.budget.max_task_tokens == 0
        {
            bail!("runtime limits must be positive");
        }
        if !["file", "sqlite"].contains(&self.storage.backend.as_str()) {
            bail!("storage.backend must be file or sqlite");
        }
        if !self.context.target_fraction.is_finite()
            || !(0.1..=0.95).contains(&self.context.target_fraction)
            || self.context.summary_max_bytes < 256
        {
            bail!("invalid context projection limits");
        }
        for budget in [self.budget.max_task_usd, self.budget.max_total_usd]
            .into_iter()
            .flatten()
        {
            if !budget.is_finite() || budget < 0.0 {
                bail!("invalid budget ceiling");
            }
        }
        let mut tiers = std::collections::BTreeSet::new();
        let mut ranges = std::collections::BTreeSet::new();
        for tier in &self.routing.tiers {
            if tier.id.is_empty()
                || !tiers.insert(tier.id.clone())
                || tier.min_complexity > tier.max_complexity
            {
                bail!("invalid or duplicate tier definition");
            }
            if tier.min_complexity == 255 && tier.max_complexity == 255 {
                continue;
            }
            if tier.max_complexity > 10 {
                bail!("tier complexity must be 0..10 or 255 for manual-only");
            }
            for complexity in tier.min_complexity..=tier.max_complexity {
                if !ranges.insert(complexity) {
                    bail!("overlapping tier complexity ranges");
                }
            }
        }
        if ranges.len() != 11 {
            bail!("tier definitions must cover all complexity values 0..10");
        }
        let mut workspace_ids = std::collections::BTreeSet::new();
        for workspace in &self.workspaces {
            if workspace.id == "default"
                || workspace.id.is_empty()
                || !workspace
                    .id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                || !workspace_ids.insert(&workspace.id)
            {
                bail!("invalid workspace id");
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        for model in &self.models {
            if model.id.trim().is_empty() || model.model.trim().is_empty() {
                bail!("model identifiers must be non-empty");
            }
            if model
                .intelligence_tier
                .as_ref()
                .is_some_and(|tier| !tiers.contains(tier))
            {
                bail!("unknown intelligence_tier for model {}", model.id);
            }
            if !["max_tokens", "max_completion_tokens"]
                .contains(&model.output_token_parameter.as_str())
            {
                bail!("output_token_parameter must be max_tokens or max_completion_tokens");
            }
            if !ids.insert(&model.id) {
                bail!("duplicate model id {}", model.id);
            }
            if model.context_window == 0
                || model.max_output_tokens == 0
                || model.max_output_tokens > model.context_window
            {
                bail!("invalid token limits for model {}", model.id);
            }
            if !model.input_usd_per_million.is_finite()
                || !model.output_usd_per_million.is_finite()
                || model.input_usd_per_million < 0.0
                || model.output_usd_per_million < 0.0
            {
                bail!("invalid model costs");
            }
            if model.provider == crate::model::ProviderKind::ChatCompletions
                && model.base_url.is_none()
            {
                bail!("chat model {} needs base_url", model.id);
            }
        }
        if self.routing.output_reserve_tokens == 0
            || !self.routing.tier_penalty.is_finite()
            || !self.routing.latency_weight.is_finite()
            || !self.routing.cost_weight.is_finite()
            || self.routing.tier_penalty < 0.0
            || self.routing.latency_weight < 0.0
            || self.routing.cost_weight < 0.0
        {
            bail!("invalid routing weights");
        }
        for rule in &self.routing.rules {
            if rule.complexity > 10
                || rule.keywords.is_empty()
                || rule.keywords.iter().any(|word| word.is_empty())
            {
                bail!("invalid classifier rule");
            }
        }
        Ok(())
    }
    pub fn write_toml(&self, path: &Path) -> Result<()> {
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}
