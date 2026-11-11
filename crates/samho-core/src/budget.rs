//! 调用前持久化最坏预算预留；崩溃/错误调用不退回未知实际消耗。
use crate::config::BudgetConfig;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, sync::Arc};
use tokio::{io::AsyncWriteExt, sync::Mutex};
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Consumption {
    pub usd: f64,
    pub tokens: u64,
}
struct Ledger {
    file: tokio::fs::File,
    total: Consumption,
    tasks: BTreeMap<String, Consumption>,
}
#[derive(Clone)]
pub struct BudgetTracker {
    config: BudgetConfig,
    ledger: Arc<Mutex<Ledger>>,
    durable: bool,
}
pub struct Reservation {
    task: String,
    usd: f64,
    tokens: u64,
}
impl BudgetTracker {
    pub async fn open(path: &Path, config: BudgetConfig, durable: bool) -> Result<Self> {
        let mut total = Consumption::default();
        if path.exists() {
            let bytes = tokio::fs::read(path).await?;
            let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
            for line in bytes[..end]
                .split(|b| *b == b'\n')
                .filter(|l| !l.is_empty())
            {
                total = serde_json::from_slice(line)?;
                if !total.usd.is_finite() || total.usd < 0.0 {
                    bail!("invalid budget journal");
                }
            }
            if end < bytes.len() {
                tokio::fs::OpenOptions::new()
                    .write(true)
                    .open(path)
                    .await?
                    .set_len(end as u64)
                    .await?;
            }
        }
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        Ok(Self {
            config,
            ledger: Arc::new(Mutex::new(Ledger {
                file,
                total,
                tasks: BTreeMap::new(),
            })),
            durable,
        })
    }
    pub async fn reserve(
        &self,
        task: &str,
        usd: f64,
        tokens: u64,
        request_limit: Option<f64>,
    ) -> Result<Reservation> {
        if !usd.is_finite() || usd < 0.0 {
            bail!("invalid call reservation");
        }
        let mut ledger = self.ledger.lock().await;
        let consumed = ledger.tasks.get(task).cloned().unwrap_or_default();
        let task_limit = match (request_limit, self.config.max_task_usd) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if task_limit.is_some_and(|l| consumed.usd + usd > l)
            || self
                .config
                .max_total_usd
                .is_some_and(|l| ledger.total.usd + usd > l)
        {
            bail!("task or cumulative budget would be exceeded");
        }
        if consumed.tokens.saturating_add(tokens) > self.config.max_task_tokens
            || self
                .config
                .max_total_tokens
                .is_some_and(|l| ledger.total.tokens.saturating_add(tokens) > l)
        {
            bail!("task or cumulative token ceiling would be exceeded");
        }
        let next = Consumption {
            usd: ledger.total.usd + usd,
            tokens: ledger.total.tokens.saturating_add(tokens),
        };
        Self::write(&mut ledger, &next, self.durable).await?;
        ledger.total = next;
        ledger.tasks.insert(
            task.into(),
            Consumption {
                usd: consumed.usd + usd,
                tokens: consumed.tokens.saturating_add(tokens),
            },
        );
        Ok(Reservation {
            task: task.into(),
            usd,
            tokens,
        })
    }
    /// 用实际用量结算预留。超预留不是错误：最坏情况估计错了，
    /// 真实账目已经持久化；返回 `true` 表示超预留，由调用方发出
    /// 告警事件。后续派发前的 ceiling 检查仍按累计真实用量把关。
    pub async fn settle(&self, reservation: Reservation, actual: Consumption) -> Result<bool> {
        if !actual.usd.is_finite() || actual.usd < 0.0 {
            bail!("invalid provider usage");
        }
        let mut ledger = self.ledger.lock().await;
        let next = Consumption {
            usd: (ledger.total.usd - reservation.usd).max(0.0) + actual.usd,
            tokens: ledger
                .total
                .tokens
                .saturating_sub(reservation.tokens)
                .saturating_add(actual.tokens),
        };
        Self::write(&mut ledger, &next, self.durable).await?;
        ledger.total = next;
        let task = ledger.tasks.entry(reservation.task).or_default();
        task.usd = (task.usd - reservation.usd).max(0.0) + actual.usd;
        task.tokens = task
            .tokens
            .saturating_sub(reservation.tokens)
            .saturating_add(actual.tokens);
        Ok(actual.usd > reservation.usd + 1e-12 || actual.tokens > reservation.tokens)
    }
    async fn write(ledger: &mut Ledger, total: &Consumption, durable: bool) -> Result<()> {
        let mut bytes = serde_json::to_vec(total)?;
        bytes.push(b'\n');
        ledger.file.write_all(&bytes).await?;
        ledger.file.flush().await?;
        if durable {
            ledger.file.sync_data().await?;
        }
        Ok(())
    }
    pub async fn total(&self) -> Consumption {
        self.ledger.lock().await.total.clone()
    }
    pub async fn finish(&self, task: &str) -> Consumption {
        self.ledger
            .lock()
            .await
            .tasks
            .remove(task)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn reservations_bound_concurrent_tasks_and_survive_restart() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("budget.jsonl");
        let config = BudgetConfig {
            max_total_usd: Some(1.0),
            ..Default::default()
        };
        let tracker = BudgetTracker::open(&path, config.clone(), false).await?;
        let first = tracker.reserve("a", 0.7, 100, None).await?;
        assert!(tracker.reserve("b", 0.5, 10, None).await.is_err());
        tracker
            .settle(
                first,
                Consumption {
                    usd: 0.2,
                    tokens: 20,
                },
            )
            .await?;
        tracker.reserve("b", 0.5, 10, None).await?; // 未知失败按预留扣费。
        drop(tracker);
        let reopened = BudgetTracker::open(&path, config, false).await?;
        assert!((reopened.total().await.usd - 0.7).abs() < 1e-9);
        assert!(reopened.reserve("c", 0.4, 10, None).await.is_err());
        Ok(())
    }
}
