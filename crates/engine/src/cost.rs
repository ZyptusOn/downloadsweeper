//! token 用量统计、成本换算与预算熔断。

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::config::Pricing;

#[derive(Debug, Clone, Default, Serialize, Deserialize, Copy)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    #[serde(default)]
    pub cache_write_1h_tokens: u64,
    #[serde(default)]
    pub cache_details_known: bool,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.prompt_tokens.saturating_add(self.completion_tokens)
    }

    pub fn add(&mut self, other: &Usage) {
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.cache_write_1h_tokens = self
            .cache_write_1h_tokens
            .saturating_add(other.cache_write_1h_tokens);
        self.cache_details_known = if self.total() == 0 {
            other.cache_details_known
        } else {
            self.cache_details_known && other.cache_details_known
        };
        self.prompt_tokens = self.prompt_tokens.saturating_add(other.prompt_tokens);
        self.completion_tokens = self
            .completion_tokens
            .saturating_add(other.completion_tokens);
    }
}

impl std::ops::AddAssign<&Usage> for Usage {
    fn add_assign(&mut self, rhs: &Usage) {
        self.add(rhs);
    }
}

/// 线程安全的用量与成本追踪器。
pub struct CostTracker {
    pricing: Pricing,
    total: Mutex<Usage>,
    budget: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum BudgetError {
    #[error("token 预算已耗尽：已用 {used} / 预算 {budget}")]
    Exceeded { used: u64, budget: u64 },
}

impl CostTracker {
    pub fn new(pricing: Pricing, budget: Option<u64>) -> Self {
        Self {
            pricing,
            total: Mutex::new(Usage::default()),
            budget,
        }
    }

    /// 累加一次调用用量；若触达预算返回错误（调用方应中断任务并记录轨迹）。
    pub fn add(&self, u: &Usage) -> Result<(), BudgetError> {
        let mut total = self.total.lock().expect("cost mutex poisoned");
        total.add(u);
        if let Some(budget) = self.budget {
            if total.total() > budget {
                return Err(BudgetError::Exceeded {
                    used: total.total(),
                    budget,
                });
            }
        }
        Ok(())
    }

    pub fn total(&self) -> Usage {
        *self.total.lock().expect("cost mutex poisoned")
    }

    /// 估算已花费美元。
    pub fn cost_usd(&self) -> f64 {
        let u = self.total();
        (u.prompt_tokens as f64 / 1000.0) * self.pricing.input_per_1k_usd
            + (u.completion_tokens as f64 / 1000.0) * self.pricing.output_per_1k_usd
    }
}
