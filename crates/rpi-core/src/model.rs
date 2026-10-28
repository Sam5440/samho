use crate::config::RoutingConfig;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    Nano,
    Fast,
    Balanced,
    Reasoning,
    Frontier,
    Specialist,
}
impl ModelTier {
    pub fn name(self) -> &'static str {
        match self {
            Self::Nano => "nano",
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::Reasoning => "reasoning",
            Self::Frontier => "frontier",
            Self::Specialist => "specialist",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Demo,
    ChatCompletions,
    Custom,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelDescriptor {
    pub id: String,
    pub model: String,
    pub provider: ProviderKind,
    pub tier: ModelTier,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub supports_reasoning: bool,
    pub reasoning_effort: Option<String>,
    /// 预留额度中除 `max_output_tokens` 之外再为思考 token 预留的数量。
    /// 某些网关对推理模型不把思考 token 计入 `max_tokens`，缺省
    /// （None）时按 `supports_reasoning` 预留等额于 `max_output_tokens`。
    pub reasoning_reserve_tokens: Option<u32>,
    pub streaming: bool,
    pub intelligence_tier: Option<String>,
    pub output_token_parameter: String,
    pub input_usd_per_million: f64,
    pub output_usd_per_million: f64,
    pub latency_ms: u64,
    pub specialties: Vec<String>,
    pub extra_body: serde_json::Value,
}
impl Default for ModelDescriptor {
    fn default() -> Self {
        Self {
            id: "demo-balanced".into(),
            model: "demo".into(),
            provider: ProviderKind::Demo,
            tier: ModelTier::Balanced,
            base_url: None,
            api_key_env: None,
            context_window: 128_000,
            max_output_tokens: 4096,
            supports_tools: true,
            supports_vision: false,
            supports_reasoning: false,
            reasoning_effort: None,
            reasoning_reserve_tokens: None,
            streaming: false,
            intelligence_tier: None,
            output_token_parameter: "max_tokens".into(),
            input_usd_per_million: 0.0,
            output_usd_per_million: 0.0,
            latency_ms: 50,
            specialties: Vec::new(),
            extra_body: serde_json::json!({}),
        }
    }
}
impl ModelDescriptor {
    /// 预算预留使用的输出上限：`max_output_tokens` 加思考余量。
    /// 推理模型的思考 token 不受网关 `max_tokens` 约束时，实际
    /// `completion_tokens` 会超过 `max_output_tokens`，预留必须覆盖。
    pub fn budget_output_reserve(&self) -> u32 {
        let reasoning = self
            .reasoning_reserve_tokens
            .unwrap_or(if self.supports_reasoning {
                self.max_output_tokens
            } else {
                0
            });
        self.max_output_tokens.saturating_add(reasoning)
    }
    pub fn tier_name(&self) -> &str {
        self.intelligence_tier
            .as_deref()
            .unwrap_or(self.tier.name())
    }
    pub fn demo(id: &str, tier: ModelTier) -> Self {
        Self {
            id: id.into(),
            tier,
            supports_reasoning: matches!(tier, ModelTier::Reasoning | ModelTier::Frontier),
            specialties: if tier == ModelTier::Specialist {
                vec!["code".into()]
            } else {
                Vec::new()
            },
            ..Self::default()
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TaskProfile {
    pub automatic: bool,
    pub complexity: u8,
    pub context_tokens: u32,
    pub requires_tools: bool,
    pub requires_vision: bool,
    pub requires_reasoning: bool,
    pub specialty: Option<String>,
    pub budget_usd: Option<f64>,
    pub max_latency_ms: Option<u64>,
    pub tier: Option<ModelTier>,
    pub model: Option<String>,
    pub intelligence_tier: Option<String>,
}
impl Default for TaskProfile {
    fn default() -> Self {
        Self {
            automatic: true,
            complexity: 4,
            context_tokens: 0,
            requires_tools: false,
            requires_vision: false,
            requires_reasoning: false,
            specialty: None,
            budget_usd: None,
            max_latency_ms: None,
            tier: None,
            model: None,
            intelligence_tier: None,
        }
    }
}
impl TaskProfile {
    pub fn classify(complexity: u8) -> Self {
        Self {
            automatic: false,
            complexity,
            ..Self::default()
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteDecision {
    pub model: ModelDescriptor,
    pub desired_tier: ModelTier,
    pub score: f64,
    pub estimated_cost_usd: f64,
    pub rationale: Vec<String>,
    pub rejected: Vec<String>,
    pub desired_tier_name: String,
}
pub trait TaskRouter: Send + Sync {
    fn classify(&self, prompt: &str, profile: &TaskProfile) -> TaskProfile {
        let _ = prompt;
        profile.clone()
    }
    fn decide(&self, task: &TaskProfile, excluded_models: &[String]) -> Result<RouteDecision>;
}
#[derive(Clone)]
pub struct ModelRouter {
    models: Vec<ModelDescriptor>,
    policy: RoutingConfig,
}
impl ModelRouter {
    pub fn new(models: Vec<ModelDescriptor>, policy: RoutingConfig) -> Self {
        Self { models, policy }
    }
    pub fn models(&self) -> &[ModelDescriptor] {
        &self.models
    }
    pub fn classify_prompt(&self, prompt: &str, profile: &TaskProfile) -> TaskProfile {
        let mut profile = profile.clone();
        if !profile.automatic
            || profile.model.is_some()
            || profile.tier.is_some()
            || profile.intelligence_tier.is_some()
        {
            return profile;
        }
        let lower = prompt.to_lowercase();
        profile.complexity = self
            .policy
            .rules
            .iter()
            .find(|rule| {
                rule.keywords
                    .iter()
                    .any(|word| lower.contains(&word.to_lowercase()))
            })
            .map(|rule| rule.complexity)
            .unwrap_or(if prompt.chars().count() <= 32 { 1 } else { 4 });
        profile
    }
    pub fn route(&self, task: &TaskProfile) -> Result<RouteDecision> {
        self.route_excluding(task, &[])
    }
    pub fn route_excluding(
        &self,
        task: &TaskProfile,
        excluded: &[String],
    ) -> Result<RouteDecision> {
        if task.complexity > 10 {
            bail!("complexity must be within 0..=10");
        }
        if let Some(budget) = task.budget_usd
            && (!budget.is_finite() || budget < 0.0)
        {
            bail!("invalid task budget");
        }
        let desired = task.tier.unwrap_or(match task.complexity {
            0..=1 => ModelTier::Nano,
            2..=3 => ModelTier::Fast,
            4..=5 => ModelTier::Balanced,
            6..=7 => ModelTier::Reasoning,
            _ => ModelTier::Frontier,
        });
        let tier_name = task
            .intelligence_tier
            .as_deref()
            .or(task.tier.map(ModelTier::name))
            .or_else(|| {
                self.policy
                    .tiers
                    .iter()
                    .find(|t| {
                        task.complexity >= t.min_complexity && task.complexity <= t.max_complexity
                    })
                    .map(|t| t.id.as_str())
            })
            .unwrap_or(desired.name());
        let desired_definition = self
            .policy
            .tiers
            .iter()
            .find(|t| t.id == tier_name)
            .ok_or_else(|| anyhow::anyhow!("unknown intelligence tier {tier_name}"))?;
        if task.intelligence_tier.is_some() && task.tier.is_some_and(|t| t.name() != tier_name) {
            bail!("conflicting tier and intelligence_tier constraints");
        }
        let mut rejected = Vec::new();
        let mut candidates = Vec::new();
        for model in &self.models {
            if excluded.contains(&model.id) {
                rejected.push(format!("{}: excluded after retryable failure", model.id));
                continue;
            }
            if task
                .intelligence_tier
                .as_ref()
                .is_some_and(|tier| model.tier_name() != tier)
            {
                rejected.push(format!("{}: custom tier mismatch", model.id));
                continue;
            }
            if task.tier.is_some_and(|tier| model.tier != tier) {
                rejected.push(format!("{}: explicit tier mismatch", model.id));
                continue;
            }
            if task.model.as_ref().is_some_and(|id| *id != model.id) {
                continue;
            }
            let output = model
                .max_output_tokens
                .max(self.policy.output_reserve_tokens);
            let estimated_cost = (task.context_tokens as f64 * model.input_usd_per_million
                + output as f64 * model.output_usd_per_million)
                / 1_000_000.0;
            let reason = if task.requires_tools && !model.supports_tools {
                Some("no tools")
            } else if task.requires_vision && !model.supports_vision {
                Some("no vision")
            } else if (task.requires_reasoning || desired_definition.requires_reasoning)
                && !model.supports_reasoning
            {
                Some("no reasoning")
            } else if task.context_tokens.saturating_add(output) > model.context_window {
                Some("context capacity")
            } else if task
                .budget_usd
                .is_some_and(|budget| estimated_cost > budget)
            {
                Some("budget exceeded")
            } else if task
                .max_latency_ms
                .is_some_and(|latency| model.latency_ms > latency)
            {
                Some("latency constraint")
            } else if task
                .specialty
                .as_ref()
                .is_some_and(|specialty| !model.specialties.contains(specialty))
            {
                Some("specialty mismatch")
            } else {
                None
            };
            if let Some(reason) = reason {
                rejected.push(format!("{}: {reason}", model.id));
                continue;
            }
            let actual_rank = self
                .policy
                .tiers
                .iter()
                .find(|t| t.id == model.tier_name())
                .map_or_else(|| tier_score(model.tier), |t| t.rank);
            let distance =
                (actual_rank as i64 - desired_definition.rank as i64).unsigned_abs() as f64;
            let score = 100.0
                - distance * self.policy.tier_penalty
                - model.latency_ms as f64 * self.policy.latency_weight
                - estimated_cost * self.policy.cost_weight;
            candidates.push(RouteDecision {
                model: model.clone(),
                desired_tier: desired,
                score,
                estimated_cost_usd: estimated_cost,
                rationale: vec![
                    format!(
                        "complexity={}; desired={desired:?}; actual={:?}",
                        task.complexity, model.tier
                    ),
                    format!(
                        "estimated_usd={estimated_cost:.6}; latency_hint={}ms; context={}",
                        model.latency_ms, task.context_tokens
                    ),
                ],
                rejected: Vec::new(),
                desired_tier_name: tier_name.to_owned(),
            });
        }
        candidates.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.model.id.cmp(&b.model.id))
        });
        let mut decision = candidates.into_iter().next().ok_or_else(|| {
            anyhow::anyhow!(
                "no model satisfies task constraints: {}",
                rejected.join("; ")
            )
        })?;
        decision.rejected = rejected;
        Ok(decision)
    }
}
impl TaskRouter for ModelRouter {
    fn classify(&self, prompt: &str, profile: &TaskProfile) -> TaskProfile {
        self.classify_prompt(prompt, profile)
    }
    fn decide(&self, task: &TaskProfile, excluded_models: &[String]) -> Result<RouteDecision> {
        self.route_excluding(task, excluded_models)
    }
}
fn tier_score(tier: ModelTier) -> i32 {
    match tier {
        ModelTier::Nano => 0,
        ModelTier::Fast => 1,
        ModelTier::Balanced | ModelTier::Specialist => 2,
        ModelTier::Reasoning => 3,
        ModelTier::Frontier => 4,
    }
}
