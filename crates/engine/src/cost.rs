//! 服务端确认的 token 用量；预算由 ai_runtime 控制，费用由 pricing 计算。

use serde::{Deserialize, Serialize};

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
