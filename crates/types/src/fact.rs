use crate::{Usage, canonical, provider};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    #[serde(default)]
    pub detail: provider::Detail,
    #[serde(default)]
    pub canonical: canonical::Record,
    pub index: usize,
    pub kind: String,
    pub timestamp: String,
    pub role: String,
    pub phase: String,
    pub text: String,
    pub model: String,
    pub tool: String,
    pub call_id: String,
    pub usage: Usage,
    pub excluded: bool,
    pub usage_key: String,
    pub effort: String,
    pub usage_eligible: bool,
    #[serde(default)]
    pub guardian_prompt: bool,
}
