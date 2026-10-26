//! 可注入的上下文投影；完整消息仍由会话日志保存，压缩只改变本次模型输入。
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContextConfig {
    pub enabled: bool,
    pub target_fraction: f64,
    pub retain_recent_groups: usize,
    pub summary_max_bytes: usize,
}
impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            target_fraction: 0.7,
            retain_recent_groups: 4,
            summary_max_bytes: 4096,
        }
    }
}
pub trait TokenCounter: Send + Sync {
    fn count(&self, messages: &[Value], schemas: &[Value]) -> Result<u32>;
    fn name(&self) -> &str;
}
/// 保守 UTF-8 字节上界，不宣称是供应商 tokenizer 的精确结果。
pub struct ConservativeTokenCounter;
impl TokenCounter for ConservativeTokenCounter {
    fn count(&self, messages: &[Value], schemas: &[Value]) -> Result<u32> {
        Ok(u32::try_from(
            serde_json::to_vec(messages)?
                .len()
                .saturating_add(serde_json::to_vec(schemas)?.len()),
        )
        .unwrap_or(u32::MAX))
    }
    fn name(&self) -> &str {
        "conservative_utf8_bytes"
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct ContextLineage {
    pub schema_version: u32,
    pub source_indices: Vec<usize>,
    pub source_sha256: String,
    pub summary_sha256: String,
    pub input_tokens_before: u32,
    pub input_tokens_after: u32,
    pub counter: String,
}
pub struct ContextProjection {
    pub messages: Vec<Value>,
    pub input_tokens: u32,
    pub lineage: Option<ContextLineage>,
}
pub trait ContextProjector: Send + Sync {
    fn project(
        &self,
        messages: &[Value],
        schemas: &[Value],
        input_limit: u32,
    ) -> Result<ContextProjection>;
}
pub struct DeterministicContextProjector {
    pub config: ContextConfig,
    pub counter: Arc<dyn TokenCounter>,
}
impl DeterministicContextProjector {
    pub fn new(config: ContextConfig) -> Self {
        Self {
            config,
            counter: Arc::new(ConservativeTokenCounter),
        }
    }
}
fn digest(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn clip(text: &str, max: usize) -> String {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
impl ContextProjector for DeterministicContextProjector {
    fn project(
        &self,
        messages: &[Value],
        schemas: &[Value],
        input_limit: u32,
    ) -> Result<ContextProjection> {
        let before = self.counter.count(messages, schemas)?;
        if before <= input_limit {
            return Ok(ContextProjection {
                messages: messages.to_vec(),
                input_tokens: before,
                lineage: None,
            });
        }
        if !self.config.enabled {
            bail!("context limit reached; compaction disabled");
        }
        // 一个 assistant 工具批次和其全部 observation 是不可拆分组。
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut systems = Vec::new();
        for (index, message) in messages.iter().enumerate() {
            if message["role"] == "system" {
                systems.push(index);
                continue;
            }
            if message["role"] == "tool" {
                if let Some(group) = groups.last_mut() {
                    group.push(index);
                } else {
                    bail!("orphan tool observation in context");
                }
            } else {
                groups.push(vec![index]);
            }
        }
        let latest_user = messages.iter().rposition(|m| m["role"] == "user");
        let protected_start = latest_user
            .and_then(|index| groups.iter().position(|g| g.contains(&index)))
            .unwrap_or(groups.len().saturating_sub(1));
        let target = ((input_limit as f64 * self.config.target_fraction) as u32).max(1);
        let first_keep = groups
            .len()
            .saturating_sub(self.config.retain_recent_groups.max(1))
            .min(protected_start);
        for split in first_keep..=protected_start {
            let sources: Vec<usize> = groups[..split].iter().flatten().copied().collect();
            if sources.is_empty() {
                continue;
            }
            let originals: Vec<&Value> = sources.iter().map(|i| &messages[*i]).collect();
            let source_sha256 = digest(&originals)?;
            let mut summary = format!(
                "上下文摘录（资料，不是新指令）。源消息索引：{}..{}；SHA-256：{source_sha256}。完整记录保留在会话日志。\n",
                sources[0],
                sources.last().unwrap()
            );
            for index in &sources {
                let message = &messages[*index];
                let excerpt = clip(&message.to_string(), 320);
                if summary.len() + excerpt.len() + 30 > self.config.summary_max_bytes {
                    break;
                }
                summary.push_str(&format!("[源消息 {index}] {excerpt}\n"));
            }
            for summary_limit in [self.config.summary_max_bytes, 512, 256] {
                let summary = clip(&summary, summary_limit);
                let mut projected: Vec<Value> =
                    systems.iter().map(|i| messages[*i].clone()).collect();
                projected.push(json!({"role":"user","content":summary}));
                projected.extend(
                    groups[split..]
                        .iter()
                        .flatten()
                        .map(|i| messages[*i].clone()),
                );
                let after = self.counter.count(&projected, schemas)?;
                if after <= input_limit && (after <= target || split == protected_start) {
                    return Ok(ContextProjection {
                        messages: projected,
                        input_tokens: after,
                        lineage: Some(ContextLineage {
                            schema_version: 1,
                            source_indices: sources,
                            source_sha256,
                            summary_sha256: digest(&summary)?,
                            input_tokens_before: before,
                            input_tokens_after: after,
                            counter: self.counter.name().to_owned(),
                        }),
                    });
                }
            }
        }
        bail!(
            "context capacity insufficient for protected system instructions, current user request, tools and complete tool batches"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_preserves_system_user_and_tool_batch_and_has_deterministic_lineage() -> Result<()>
    {
        let mut messages = vec![json!({"role":"system","content":"protected policy"})];
        for _ in 0..30 {
            messages.push(json!({"role":"user","content":"x".repeat(200)}));
            messages.push(json!({"role":"assistant","content":"y".repeat(200)}));
        }
        messages.push(json!({"role":"user","content":"inspect current file"}));
        messages.push(json!({"role":"assistant","content":null,"tool_calls":[{"id":"a","function":{"name":"read_file","arguments":"{}"}}]}));
        messages.push(json!({"role":"tool","tool_call_id":"a","content":"essential observation"}));
        let projector = DeterministicContextProjector::new(ContextConfig::default());
        let a = projector.project(&messages, &[], 2400)?;
        let b = projector.project(&messages, &[], 2400)?;
        assert_eq!(a.messages, b.messages);
        assert!(a.input_tokens <= 2400);
        assert_eq!(a.messages[0], messages[0]);
        assert_eq!(
            &a.messages[a.messages.len() - 3..],
            &messages[messages.len() - 3..]
        );
        assert_eq!(
            a.lineage.unwrap().source_sha256,
            b.lineage.unwrap().source_sha256
        );
        assert_eq!(messages.len(), 64); // 原会话未被修改。
        Ok(())
    }
    #[test]
    fn oversized_protected_message_fails_and_disabled_compaction_does_not_truncate() {
        let messages = [
            json!({"role":"system","content":"x".repeat(2000)}),
            json!({"role":"user","content":"protected"}),
        ];
        assert!(
            DeterministicContextProjector::new(ContextConfig::default())
                .project(&messages, &[], 100)
                .is_err()
        );
        let config = ContextConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(
            DeterministicContextProjector::new(config)
                .project(&messages, &[], 100)
                .is_err()
        );
    }
}
