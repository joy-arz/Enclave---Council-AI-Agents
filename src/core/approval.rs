//! Approval policy system for tool execution control
//! Based on nca-cli's approval pattern with wildcard matching

use serde::{Deserialize, Serialize};

/// Permission tier levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionTier {
    /// Tool execution is automatically allowed
    Allowed,
    /// Tool execution requires user approval
    Ask,
    /// Tool execution is denied
    Denied,
}

/// Permission mode for the agent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PermissionMode {
    /// Everything allowed (autonomous mode)
    BypassPermissions,
    /// Read-only mode (no file modifications)
    Plan,
    /// File edits allowed, destructive requires approval
    AcceptEdits,
    /// Read-only + file edits denied (strict proposal mode)
    DontAsk,
    /// Ask for everything not explicitly allowed (default)
    #[default]
    Default,
}

/// Approval policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalPolicy {
    /// Current permission mode
    mode: PermissionMode,
    /// Tool-specific allow/deny patterns
    allow_list: Vec<String>,
    /// Tools that require explicit approval
    deny_list: Vec<String>,
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            mode: PermissionMode::Default,
            allow_list: Vec::new(),
            deny_list: Vec::new(),
        }
    }
}

impl ApprovalPolicy {
    /// Create a new approval policy
    #[allow(dead_code)]
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            mode,
            allow_list: Vec::new(),
            deny_list: Vec::new(),
        }
    }

    /// Add an allow pattern (supports wildcards: "execute_bash:git *")
    #[allow(dead_code)]
    pub fn add_allow(&mut self, pattern: &str) {
        self.allow_list.push(pattern.to_string());
    }

    /// Add a deny pattern
    #[allow(dead_code)]
    pub fn add_deny(&mut self, pattern: &str) {
        self.deny_list.push(pattern.to_string());
    }

    /// Check if a tool call is allowed
    pub fn check(&self, tool_name: &str, tool_input: &str) -> PermissionTier {
        // BypassPermissions mode allows everything
        if self.mode == PermissionMode::BypassPermissions {
            return PermissionTier::Allowed;
        }

        // Check deny list first
        for pattern in &self.deny_list {
            if wildcard_matches(pattern, &format!("{}:{}", tool_name, tool_input)) {
                return PermissionTier::Denied;
            }
        }

        // Check allow list
        for pattern in &self.allow_list {
            if wildcard_matches(pattern, &format!("{}:{}", tool_name, tool_input)) {
                return PermissionTier::Allowed;
            }
        }

        // Default based on mode
        match self.mode {
            PermissionMode::Plan | PermissionMode::DontAsk => PermissionTier::Denied,
            PermissionMode::AcceptEdits => {
                // Allow read operations, deny destructive ones
                if tool_name == "read_file" || tool_name == "list_directory" || tool_name == "grep" {
                    PermissionTier::Allowed
                } else {
                    PermissionTier::Ask
                }
            }
            PermissionMode::Default => PermissionTier::Ask,
            PermissionMode::BypassPermissions => PermissionTier::Allowed,
        }
    }

    /// Suggest an allow pattern based on tool call
    #[allow(dead_code)]
    pub fn suggest_allow_pattern(tool_name: &str, tool_input: &str) -> String {
        // Extract command from input if it looks like a shell command
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(tool_input) {
            if let Some(cmd) = json.get("command").and_then(|v| v.as_str()) {
                // Suggest pattern for command type
                let cmd_base = cmd.split_whitespace().next().unwrap_or("*");
                return format!("{}:{} *", tool_name, cmd_base);
            }
        }
        // Fallback: if input is short/simple, suggest general pattern
        if tool_input.len() < 50 {
            format!("{}:*", tool_name)
        } else {
            format!("{}:{}", tool_name, tool_input.chars().take(30).collect::<String>())
        }
    }
}

/// Check if a pattern matches using wildcard rules
/// Supports:
/// - Exact match: "read_file"
/// - Prefix wildcard: "execute_bash:*"
/// - Substring wildcard: "*:git *"
fn wildcard_matches(pattern: &str, text: &str) -> bool {
    // Simple exact match
    if !pattern.contains('*') {
        return pattern == text;
    }

    // Split pattern into parts
    let parts: Vec<&str> = pattern.split('*').collect();
    let text_lower = text.to_lowercase();
    let pattern_lower = pattern.to_lowercase();

    // If pattern starts with literal (no wildcard prefix)
    if !pattern.starts_with('*') && !pattern_lower.starts_with('*') {
        let first_literal = parts[0].to_lowercase();
        if !text_lower.starts_with(&first_literal) {
            return false;
        }
    }

    // If pattern ends with literal (no wildcard suffix)
    if !pattern.ends_with('*') && !pattern_lower.ends_with('*') {
        let last_literal = parts.last().unwrap().to_lowercase();
        if !text_lower.ends_with(&last_literal) {
            return false;
        }
    }

    // Check all literal parts appear in order
    let mut search_text = text_lower.as_str();
    for part in parts.iter() {
        let part_lower = part.to_lowercase();
        if part_lower.is_empty() {
            continue;
        }
        if let Some(pos) = search_text.find(&part_lower) {
            search_text = &search_text[pos + part_lower.len()..];
        } else {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        assert!(wildcard_matches("read_file", "read_file"));
        assert!(!wildcard_matches("read_file", "write_file"));
    }

    #[test]
    fn test_wildcard_suffix() {
        assert!(wildcard_matches("execute_bash:*", "execute_bash:git status"));
        assert!(wildcard_matches("execute_bash:*", "execute_bash:ls -la"));
        assert!(!wildcard_matches("execute_bash:*", "execute_other:ls"));
    }

    #[test]
    fn test_wildcard_prefix() {
        assert!(wildcard_matches("*:git status", "execute_bash:git status"));
        assert!(wildcard_matches("*:git *", "execute_bash:git status"));
    }

    #[test]
    fn test_approval_policy_default() {
        let policy = ApprovalPolicy::default();
        assert_eq!(policy.check("read_file", "{}"), PermissionTier::Ask);
    }

    #[test]
    fn test_approval_policy_bypass() {
        let policy = ApprovalPolicy::new(PermissionMode::BypassPermissions);
        assert_eq!(policy.check("read_file", "{}"), PermissionTier::Allowed);
        assert_eq!(policy.check("run_shell_command", "rm -rf /"), PermissionTier::Allowed);
    }

    #[test]
    fn test_approval_policy_deny_list() {
        let mut policy = ApprovalPolicy::default();
        policy.add_deny("run_shell_command:*rm*");
        assert_eq!(policy.check("run_shell_command", "rm -rf /"), PermissionTier::Denied);
        assert_eq!(policy.check("run_shell_command", "ls -la"), PermissionTier::Ask);
    }

    #[test]
    fn test_approval_policy_allow_list() {
        let mut policy = ApprovalPolicy::default();
        policy.add_allow("read_file:*");
        assert_eq!(policy.check("read_file", "Cargo.toml"), PermissionTier::Allowed);
    }

    #[test]
    fn test_suggest_allow_pattern() {
        let pattern = ApprovalPolicy::suggest_allow_pattern("run_shell_command", r#"{"command": "git status"}"#);
        assert!(pattern.contains("git"));
    }
}