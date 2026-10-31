//! User-facing settings and the two behavioural knobs Cordial exposes:
//! permission mode and effort.

use serde::{Deserialize, Serialize};

/// How tool calls are approved before they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    /// Read and network tools run immediately. Local writes need approval once
    /// per tool, then are remembered for the session.
    #[default]
    Auto,
    /// Every tool call waits for approval.
    Manual,
}

impl PermissionMode {
    pub fn label(self) -> &'static str {
        match self {
            PermissionMode::Auto => "Auto",
            PermissionMode::Manual => "Manual",
        }
    }
}

/// Effort budget: how hard the agent may work before answering.
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
    pub fn label(self) -> &'static str {
        match self {
            Effort::Low => "Low",
            Effort::Medium => "Medium",
            Effort::High => "High",
        }
    }

    /// Maximum number of tool-call rounds inside a single user turn.
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

    /// Ceiling on tokens generated per model round. Generation still stops at
    /// end-of-turn, so this only matters when reasoning runs long: a 2B model
    /// can spend several hundred tokens thinking about a hard question, and a
    /// tight cap would cut the answer off entirely.
    pub fn max_output_tokens(self) -> u32 {
        match self {
            Effort::Low => 512,
            Effort::Medium => 1_536,
            Effort::High => 3_072,
        }
    }

    /// Maximum characters of a fetched page that go into context.
    pub fn fetch_chars(self) -> usize {
        match self {
            Effort::Low => 4_000,
            Effort::Medium => 10_000,
            Effort::High => 24_000,
        }
    }

    /// Sampling temperature. Liquid AI recommends 0.1 for LFM3.
    pub fn temperature(self) -> f32 {
        match self {
            Effort::Low => 0.1,
            Effort::Medium => 0.1,
            Effort::High => 0.2,
        }
    }

    /// One line handed to the model so it can size its answer.
    pub fn guidance(self) -> &'static str {
        match self {
            Effort::Low => {
                "Effort: low. Answer in one or two sentences. Use at most one tool call."
            }
            Effort::Medium => {
                "Effort: medium. Answer in a short paragraph. Use tools when they improve accuracy."
            }
            Effort::High => {
                "Effort: high. Answer thoroughly but stay concise. Prefer two sources when facts matter."
            }
        }
    }
}

/// Persisted application settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Context window in tokens. LFM3-VL-2B supports 128K; Cordial defaults to it.
    pub context_tokens: u32,
    pub permission_mode: PermissionMode,
    pub effort: Effort,
    /// Absolute path of the model file. Empty means "use the managed download".
    pub model_path: String,
    /// Absolute path of the mmproj (vision projector). Empty means managed.
    pub mmproj_path: String,
    /// Folder the file tools may read and write. Empty means the default.
    pub workspace_dir: String,
    /// Worker threads for inference. 0 selects a sensible default.
    pub threads: u32,
    /// Allow the model to reach the network (search, fetch, browser).
    pub web_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            context_tokens: 128 * 1024,
            permission_mode: PermissionMode::Auto,
            effort: Effort::Medium,
            model_path: String::new(),
            mmproj_path: String::new(),
            workspace_dir: String::new(),
            threads: 0,
            web_enabled: true,
        }
    }
}

impl Settings {
    /// Clamps values that a settings file (or an old version) could get wrong.
    pub fn sanitised(mut self, max_ctx: u32) -> Self {
        let upper = if max_ctx == 0 { 128 * 1024 } else { max_ctx };
        self.context_tokens = self.context_tokens.clamp(2_048, upper.max(2_048));
        let cores = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(4);
        self.threads = if self.threads == 0 {
            cores.saturating_sub(1).max(1)
        } else {
            self.threads.clamp(1, cores.max(1))
        };
        self
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
    fn effort_budgets_grow_monotonically() {
        assert!(Effort::Low.max_tool_rounds() < Effort::Medium.max_tool_rounds());
        assert!(Effort::Medium.max_tool_rounds() < Effort::High.max_tool_rounds());
        assert!(Effort::Low.max_output_tokens() < Effort::High.max_output_tokens());
    }

    #[test]
    fn settings_round_trip_json() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn settings_survive_bad_values() {
        let raw = r#"{"context_tokens": 5, "threads": 900}"#;
        let parsed: Settings = serde_json::from_str(raw).unwrap();
        let sane = parsed.sanitised(131_072);
        assert_eq!(sane.context_tokens, 2_048);
        assert!(sane.threads <= 64);
    }

    #[test]
    fn permission_mode_parses_from_lowercase() {
        let parsed: PermissionMode = serde_json::from_str("\"manual\"").unwrap();
        assert_eq!(parsed, PermissionMode::Manual);
    }
}
