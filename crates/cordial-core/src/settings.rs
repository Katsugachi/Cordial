//! User-facing settings for Cordial.

use serde::{Deserialize, Serialize};

/// How tool calls are approved before they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    /// Tools run without asking, except writes outside the workspace which
    /// always require approval.
    #[default]
    Auto,
    /// Every tool call is shown to the user for approval first.
    Manual,
}

/// Effort budget: how hard the agent may work (and how long it may run)
/// before answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    /// Answer fast: few tool calls, short searches, terse replies.
    Low,
    #[default]
    Medium,
    /// Thorough: more tool calls, deeper searches, fuller answers.
    High,
}

impl Effort {
    /// Maximum number of tool-call rounds in a single user turn.
    pub fn max_tool_rounds(self) -> u32 {
        match self {
            Effort::Low => 3,
            Effort::Medium => 8,
            Effort::High => 16,
        }
    }

    /// Maximum search results folded into context per search call.
    pub fn search_results(self) -> usize {
        match self {
            Effort::Low => 3,
            Effort::Medium => 5,
            Effort::High => 8,
        }
    }

    /// Sampling temperature. LFM3 recommends 0.1; we only nudge upward for
    /// higher effort so multi-step answers read naturally.
    pub fn temperature(self) -> f32 {
        match self {
            Effort::Low => 0.1,
            Effort::Medium => 0.1,
            Effort::High => 0.2,
        }
    }
}

/// Persisted application settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Context window in tokens. LFM3-VL-2B supports 128K.
    pub context_tokens: u32,
    pub permission_mode: PermissionMode,
    pub effort: Effort,
    /// Absolute path of the model file. When empty, Cordial manages the
    /// default download (LFM3-VL-2B Q4_K_M).
    pub model_path: String,
    /// Absolute path of the mmproj (vision projector) file. Empty = default.
    pub mmproj_path: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            context_tokens: 128 * 1024,
            permission_mode: PermissionMode::Auto,
            effort: Effort::Medium,
            model_path: String::new(),
            mmproj_path: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_context_is_128k() {
        assert_eq!(Settings::default().context_tokens, 131_072);
    }

    #[test]
    fn effort_rounds_are_monotonic() {
        assert!(Effort::Low.max_tool_rounds() < Effort::Medium.max_tool_rounds());
        assert!(Effort::Medium.max_tool_rounds() < Effort::High.max_tool_rounds());
    }

    #[test]
    fn settings_round_trip_json() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
