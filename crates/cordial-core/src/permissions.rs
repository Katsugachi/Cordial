//! Permission decisions.
//!
//! Two modes, as the product asks for:
//!
//! * **Auto** — reads, isolated computations and network calls run immediately; anything that writes to
//!   disk asks once, then is remembered for the session.
//! * **Manual** — every tool call waits for the user.
//!
//! Unknown tools are treated as writes, so a typo in a tool name can never turn
//! into an unreviewed action.

use std::collections::HashSet;

use crate::session::ToolCall;
use crate::settings::PermissionMode;
use crate::tools::{self, Risk};

/// What the agent must do before running a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Allow,
    Ask { risk: Risk },
}

#[derive(Debug, Clone)]
pub struct PermissionPolicy {
    mode: PermissionMode,
    grants: HashSet<String>,
}

impl PermissionPolicy {
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            mode,
            grants: HashSet::new(),
        }
    }

    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: PermissionMode) {
        self.mode = mode;
    }

    /// Remembers an approval for the rest of the session.
    pub fn grant(&mut self, tool: &str) {
        self.grants.insert(tool.to_string());
    }

    pub fn is_granted(&self, tool: &str) -> bool {
        self.grants.contains(tool)
    }

    pub fn clear_grants(&mut self) {
        self.grants.clear();
    }

    pub fn grants(&self) -> Vec<String> {
        let mut out: Vec<String> = self.grants.iter().cloned().collect();
        out.sort();
        out
    }

    /// Decides whether `call` may run.
    pub fn evaluate(&self, call: &ToolCall) -> Permission {
        let risk = tools::spec_for(&call.name)
            .map(|spec| spec.risk)
            .unwrap_or(Risk::Write);

        if self.grants.contains(&call.name) {
            return Permission::Allow;
        }
        match self.mode {
            PermissionMode::Manual => Permission::Ask { risk },
            PermissionMode::Auto => match risk {
                Risk::Write => Permission::Ask { risk },
                Risk::Read | Risk::Network | Risk::Compute => Permission::Allow,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{ToolArgument, ToolCall};

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: vec![ToolArgument {
                name: "path".into(),
                value: "a.txt".into(),
            }],
        }
    }

    #[test]
    fn auto_allows_reads_and_asks_for_writes() {
        let policy = PermissionPolicy::new(PermissionMode::Auto);
        assert_eq!(policy.evaluate(&call("read_file")), Permission::Allow);
        assert_eq!(policy.evaluate(&call("web_search")), Permission::Allow);
        assert!(matches!(
            policy.evaluate(&call("write_file")),
            Permission::Ask { .. }
        ));
    }

    #[test]
    fn manual_asks_for_everything() {
        let policy = PermissionPolicy::new(PermissionMode::Manual);
        assert!(matches!(
            policy.evaluate(&call("read_file")),
            Permission::Ask { .. }
        ));
        assert!(matches!(
            policy.evaluate(&call("web_fetch")),
            Permission::Ask { .. }
        ));
    }

    #[cfg(feature = "sandbox-python")]
    #[test]
    fn isolated_python_follows_the_permission_mode() {
        assert_eq!(PermissionPolicy::new(PermissionMode::Auto).evaluate(&call("python_calculate")), Permission::Allow);
        assert_eq!(PermissionPolicy::new(PermissionMode::Manual).evaluate(&call("python_calculate")), Permission::Ask { risk: Risk::Compute });
    }

    #[test]
    fn unknown_tools_are_treated_as_writes() {
        let policy = PermissionPolicy::new(PermissionMode::Auto);
        assert!(matches!(
            policy.evaluate(&call("definitely_not_a_tool")),
            Permission::Ask { .. }
        ));
    }

    #[test]
    fn grants_persist_for_the_session() {
        let mut policy = PermissionPolicy::new(PermissionMode::Auto);
        policy.grant("write_file");
        assert_eq!(policy.evaluate(&call("write_file")), Permission::Allow);
        policy.clear_grants();
        assert!(matches!(
            policy.evaluate(&call("write_file")),
            Permission::Ask { .. }
        ));
    }

    #[test]
    fn switching_to_manual_keeps_existing_grants() {
        let mut policy = PermissionPolicy::new(PermissionMode::Auto);
        policy.grant("read_file");
        policy.set_mode(PermissionMode::Manual);
        assert_eq!(policy.evaluate(&call("read_file")), Permission::Allow);
        assert!(matches!(
            policy.evaluate(&call("list_files")),
            Permission::Ask { .. }
        ));
    }
}
