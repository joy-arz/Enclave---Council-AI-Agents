/// Permission modes system for tool execution control
///
/// This module provides a comprehensive permission system with:
/// - PermissionMode: Defines how restrictive the permission system is
/// - PermissionCatalog: Categorizes tools by their permission requirements
/// - WildcardPattern: Supports glob patterns for tool:args matching
/// - ApprovalPolicy: Main policy engine for checking tool permissions
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Permission tier levels - determines what happens when a tool is called
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionTier {
    /// Tool execution is automatically allowed
    Allowed,
    /// Tool execution requires user approval
    Ask,
    /// Tool execution is denied
    Denied,
}

/// Permission mode for the agent - defines the default behavior
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    /// Ask for approval on all tool calls (default)
    Ask,
    /// Read-only mode - no file modifications or shell commands
    Plan,
    /// Only use auto-approved tools (no prompting)
    DontAsk,
    /// File edits auto-approved, shell needs approval
    AcceptEdits,
    /// All tools available, but use caution
    BypassPermissions,
}

impl Default for PermissionMode {
    fn default() -> Self {
        PermissionMode::Ask
    }
}

/// Wildcard permission pattern supporting tool:args format.
///
/// Examples:
/// - `execute_bash:git *` - allow git with any arguments
/// - `write_file:src/*.rs` - allow writing to src rust files
/// - `*` - allow everything (bypass)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WildcardPattern {
    /// Tool name, can be `*` for any tool
    pub tool: String,
    /// Arguments pattern, can be `*` for any arguments, or a glob pattern
    pub args: String,
}

impl WildcardPattern {
    /// Parse a pattern string like "tool:args" or just "tool" (implies "tool:*")
    pub fn parse(pattern: &str) -> Option<Self> {
        let (tool, args) = if let Some(idx) = pattern.find(':') {
            (&pattern[..idx], &pattern[idx + 1..])
        } else {
            (pattern, "*")
        };

        if tool.is_empty() {
            return None;
        }

        Some(Self {
            tool: tool.to_string(),
            args: args.to_string(),
        })
    }

    /// Check if this pattern matches the given tool and arguments
    pub fn matches(&self, tool: &str, args: &str) -> bool {
        // Tool must match
        if self.tool != "*" && self.tool != tool {
            return false;
        }

        // Args must match
        if self.args == "*" {
            return true;
        }

        // Use glob matching for args
        if let Ok(pattern) = glob::Pattern::new(&self.args) {
            pattern.matches(args)
        } else {
            // Fallback to wildcard matching if glob pattern is invalid
            wildcard_matches(&self.args, args)
        }
    }
}

/// Permission catalog defining which tools are auto-approved per mode.
pub struct PermissionCatalog {
    /// Tools available without approval in Plan mode (read-only)
    plan_allowed: HashSet<&'static str>,
    /// Tools available without approval in DontAsk mode
    dont_ask_allowed: HashSet<&'static str>,
    /// Tools that need approval even in AcceptEdits (dangerous operations)
    edits_approval_needed: HashSet<&'static str>,
    /// Dangerous tools that always need approval regardless of mode
    dangerous: HashSet<&'static str>,
}

impl Default for PermissionCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionCatalog {
    /// Create a new permission catalog with default tool categorization
    pub fn new() -> Self {
        Self {
            plan_allowed: [
                "list_directory",
                "read_file",
                "grep",
                "search_code",
                "glob",
                "path_exists",
                "read_directory",
                "get_context",
                "git_status",
                "git_diff",
                "query_symbols",
                "web_search",
                "fetch_url",
                "ask_question",
            ]
            .into(),

            dont_ask_allowed: [
                "list_directory",
                "read_file",
                "grep",
                "search_code",
                "glob",
                "path_exists",
                "read_directory",
                "get_context",
                "git_status",
                "git_diff",
                "query_symbols",
                "web_search",
                "fetch_url",
                "ask_question",
                "write_file",
                "create_directory",
                "edit_file",
                "apply_patch",
                "replace_match",
                "rename_path",
            ]
            .into(),

            edits_approval_needed: [
                "run_shell_command",
                "delete_path",
                "move_path",
                "copy_path",
                "spawn_subagent",
                "execute_bash",
            ]
            .into(),

            dangerous: [
                "run_shell_command",
                "delete_path",
                "spawn_subagent",
                "execute_bash",
            ]
            .into(),
        }
    }

    /// Check if a tool is allowed (auto-approved) for the given mode
    pub fn is_allowed(&self, tool: &str, mode: PermissionMode) -> bool {
        match mode {
            PermissionMode::Ask => false,
            PermissionMode::Plan => self.plan_allowed.contains(tool),
            PermissionMode::DontAsk => self.dont_ask_allowed.contains(tool),
            PermissionMode::AcceptEdits => {
                !self.edits_approval_needed.contains(tool) && self.dont_ask_allowed.contains(tool)
            }
            PermissionMode::BypassPermissions => true,
        }
    }

    /// Check if a tool needs approval for the given mode
    pub fn needs_approval(&self, tool: &str, mode: PermissionMode) -> bool {
        // Dangerous tools always need approval
        if self.dangerous.contains(tool) {
            return true;
        }

        !self.is_allowed(tool, mode)
    }

    /// Get a description of what the permission mode means for the system prompt
    pub fn mode_guidance(&self, mode: PermissionMode) -> &'static str {
        match mode {
            PermissionMode::Ask => {
                "Permission Mode: ask - all tool calls require approval"
            }
            PermissionMode::Plan => {
                "Permission Mode: plan - read-only mode, you must not modify files or run shell commands"
            }
            PermissionMode::DontAsk => {
                "Permission Mode: dont-ask - only use auto-approved tools (read operations and safe file edits)"
            }
            PermissionMode::AcceptEdits => {
                "Permission Mode: accept-edits - file edits auto-approved, shell commands and dangerous operations need approval"
            }
            PermissionMode::BypassPermissions => {
                "Permission Mode: bypass - all tools available but use caution"
            }
        }
    }

    /// Get the set of tools allowed in Plan mode
    pub fn plan_tools(&self) -> &HashSet<&'static str> {
        &self.plan_allowed
    }

    /// Get the set of tools allowed in DontAsk mode
    pub fn dont_ask_tools(&self) -> &HashSet<&'static str> {
        &self.dont_ask_allowed
    }

    /// Get the set of dangerous tools that always need approval
    pub fn dangerous_tools(&self) -> &HashSet<&'static str> {
        &self.dangerous
    }
}

/// Check if a pattern matches using wildcard rules
///
/// Supports:
/// - Exact match: "read_file"
/// - Prefix wildcard: "execute_bash:*"
/// - Substring wildcard: "*:git *"
/// - Full wildcard: "*"
pub fn wildcard_matches(pattern: &str, text: &str) -> bool {
    // Simple exact match
    if !pattern.contains('*') {
        return text.contains(pattern);
    }

    // Split pattern into parts
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        if i == 0 {
            // First segment: text must start with it
            if !text.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if i == parts.len() - 1 {
            // Last segment: text must end with it
            if !text[pos..].ends_with(part) {
                return false;
            }
        } else {
            // Interior segment: must appear after current position
            match text[pos..].find(part) {
                Some(idx) => pos += idx + part.len(),
                None => return false,
            }
        }
    }

    true
}

/// Approval policy configuration for tool execution control
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalPolicy {
    /// Current permission mode
    mode: PermissionMode,
    /// Tool-specific allow patterns (supports wildcards)
    allow_list: Vec<String>,
    /// Tools that require explicit denial
    deny_list: Vec<String>,
    /// Session-scoped allow patterns (from user approvals)
    session_allow: Vec<String>,
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            mode: PermissionMode::Ask,
            allow_list: Vec::new(),
            deny_list: Vec::new(),
            session_allow: Vec::new(),
        }
    }
}

impl ApprovalPolicy {
    /// Create a new approval policy with the given mode
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            mode,
            allow_list: Vec::new(),
            deny_list: Vec::new(),
            session_allow: Vec::new(),
        }
    }

    /// Create with a custom permission catalog (for testing or advanced use)
    #[allow(dead_code)]
    pub fn with_catalog(mode: PermissionMode) -> Self {
        Self::new(mode)
    }

    /// Get the current permission mode
    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    /// Set the permission mode
    pub fn set_mode(&mut self, mode: PermissionMode) {
        self.mode = mode;
    }

    /// Add an allow pattern (supports wildcards: "execute_bash:git *")
    pub fn add_allow(&mut self, pattern: &str) {
        if !self.allow_list.contains(&pattern.to_string()) {
            self.allow_list.push(pattern.to_string());
        }
    }

    /// Add a deny pattern
    pub fn add_deny(&mut self, pattern: &str) {
        if !self.deny_list.contains(&pattern.to_string()) {
            self.deny_list.push(pattern.to_string());
        }
    }

    /// Add a session-scoped allow pattern (e.g., from user "always allow" choice)
    pub fn add_session_allow(&mut self, pattern: String) {
        if !self.session_allow.contains(&pattern) {
            self.session_allow.push(pattern);
        }
    }

    /// Get all allow patterns (config + session)
    fn all_allow_patterns(&self) -> impl Iterator<Item = &str> {
        self.allow_list
            .iter()
            .chain(self.session_allow.iter())
            .map(|s| s.as_str())
    }

    /// Check if a tool call is allowed, needs approval, or is denied
    pub fn check(&self, tool_name: &str, tool_input: &str) -> PermissionTier {
        // BypassPermissions mode allows everything
        if self.mode == PermissionMode::BypassPermissions {
            return PermissionTier::Allowed;
        }

        let full_key = format!("{}:{}", tool_name, tool_input);

        // Check deny list first - highest priority
        for pattern in &self.deny_list {
            if wildcard_matches(pattern, &full_key) {
                return PermissionTier::Denied;
            }
        }

        // Check allow lists (config + session)
        for pattern in self.all_allow_patterns() {
            if wildcard_matches(pattern, &full_key) {
                return PermissionTier::Allowed;
            }
        }

        // Use catalog-based mode checks
        let catalog = PermissionCatalog::new();

        // Check if tool is in dangerous set - always needs approval
        if catalog.dangerous_tools().contains(tool_name) {
            return PermissionTier::Ask;
        }

        // Check mode-specific catalog
        match self.mode {
            PermissionMode::Ask => {
                // Ask mode - only auto-approve if explicitly allowed in catalog
                if catalog.is_allowed(tool_name, self.mode) {
                    PermissionTier::Allowed
                } else {
                    PermissionTier::Ask
                }
            }
            PermissionMode::Plan => {
                if catalog.plan_tools().contains(tool_name) {
                    PermissionTier::Allowed
                } else {
                    PermissionTier::Denied
                }
            }
            PermissionMode::DontAsk => {
                if catalog.dont_ask_tools().contains(tool_name) {
                    PermissionTier::Allowed
                } else {
                    PermissionTier::Denied
                }
            }
            PermissionMode::AcceptEdits => {
                if catalog.is_allowed(tool_name, self.mode) {
                    PermissionTier::Allowed
                } else if catalog.edits_approval_needed.contains(tool_name) {
                    PermissionTier::Ask
                } else {
                    PermissionTier::Ask
                }
            }
            PermissionMode::BypassPermissions => PermissionTier::Allowed,
        }
    }

    /// Suggest an allow pattern based on tool call
    pub fn suggest_allow_pattern(tool_name: &str, tool_input: &str) -> String {
        // Extract command from input if it looks like a shell command
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(tool_input) {
            if let Some(cmd) = json.get("command").and_then(|v| v.as_str()) {
                let cmd_parts: Vec<&str> = cmd.split_whitespace().collect();
                if cmd_parts.is_empty() {
                    return format!("{}:*", tool_name);
                }
                let cmd_base = cmd_parts[0];
                if cmd_parts.len() > 1 {
                    return format!("{}:{} *", tool_name, cmd_base);
                } else {
                    return format!("{}:{}*", tool_name, cmd_base);
                }
            }
            // Check for path field
            if let Some(path) = json.get("path").and_then(|v| v.as_str()) {
                let path_parts: Vec<&str> = path.split('/').collect();
                if path_parts.len() > 1 {
                    let pattern = format!("{}:{}/*", tool_name, path_parts[0]);
                    return pattern;
                }
                return format!("{}:{}*", tool_name, path);
            }
        }
        // Fallback: if input is short/simple, suggest general pattern
        if tool_input.len() < 50 && !tool_input.contains('\n') {
            format!("{}:*", tool_name)
        } else {
            format!(
                "{}:{}",
                tool_name,
                tool_input.chars().take(30).collect::<String>()
            )
        }
    }

    /// Get the permission catalog for this policy
    pub fn catalog(&self) -> PermissionCatalog {
        PermissionCatalog::new()
    }

    /// Check if we're in a read-only mode
    pub fn is_read_only(&self) -> bool {
        matches!(self.mode, PermissionMode::Plan)
    }

    /// Check if we should fail when a tool needs approval but we're in non-interactive mode
    pub fn should_fail_on_ask(&self) -> bool {
        matches!(
            self.mode,
            PermissionMode::Ask | PermissionMode::Plan | PermissionMode::DontAsk
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_mode_default() {
        let mode = PermissionMode::default();
        assert_eq!(mode, PermissionMode::Ask);
    }

    #[test]
    fn test_wildcard_pattern_parse() {
        let pattern = WildcardPattern::parse("execute_bash:git *").unwrap();
        assert_eq!(pattern.tool, "execute_bash");
        assert_eq!(pattern.args, "git *");
    }

    #[test]
    fn test_wildcard_pattern_parse_no_args() {
        let pattern = WildcardPattern::parse("execute_bash").unwrap();
        assert_eq!(pattern.tool, "execute_bash");
        assert_eq!(pattern.args, "*");
    }

    #[test]
    fn test_wildcard_pattern_matches() {
        let pattern = WildcardPattern::parse("execute_bash:git *").unwrap();
        assert!(pattern.matches("execute_bash", "git status"));
        assert!(pattern.matches("execute_bash", "git push origin main"));
        assert!(!pattern.matches("execute_bash", "npm install"));
        assert!(!pattern.matches("execute_other", "git status"));
    }

    #[test]
    fn test_wildcard_pattern_star_tool() {
        let pattern = WildcardPattern::parse("*:git status").unwrap();
        assert!(pattern.matches("execute_bash", "git status"));
        assert!(pattern.matches("run_shell_command", "git status"));
        assert!(!pattern.matches("execute_bash", "npm install"));
    }

    #[test]
    fn test_exact_match() {
        assert!(wildcard_matches("read_file", "read_file"));
        assert!(!wildcard_matches("read_file", "write_file"));
    }

    #[test]
    fn test_wildcard_suffix() {
        assert!(wildcard_matches(
            "execute_bash:*",
            "execute_bash:git status"
        ));
        assert!(wildcard_matches("execute_bash:*", "execute_bash:ls -la"));
        assert!(!wildcard_matches("execute_bash:*", "execute_other:ls"));
    }

    #[test]
    fn test_wildcard_prefix() {
        assert!(wildcard_matches("*:git status", "execute_bash:git status"));
        assert!(wildcard_matches("*:git *", "execute_bash:git status"));
    }

    #[test]
    fn test_wildcard_both() {
        assert!(wildcard_matches("*:git *", "execute_bash:git push"));
    }

    #[test]
    fn test_wildcard_star_only() {
        assert!(wildcard_matches("*", "anything at all"));
    }

    #[test]
    fn test_permission_catalog_plan_mode() {
        let catalog = PermissionCatalog::new();
        assert!(catalog.is_allowed("read_file", PermissionMode::Plan));
        assert!(catalog.is_allowed("list_directory", PermissionMode::Plan));
        assert!(!catalog.is_allowed("write_file", PermissionMode::Plan));
        assert!(!catalog.is_allowed("execute_bash", PermissionMode::Plan));
    }

    #[test]
    fn test_permission_catalog_dont_ask_mode() {
        let catalog = PermissionCatalog::new();
        assert!(catalog.is_allowed("read_file", PermissionMode::DontAsk));
        assert!(catalog.is_allowed("write_file", PermissionMode::DontAsk));
        assert!(catalog.is_allowed("edit_file", PermissionMode::DontAsk));
        assert!(!catalog.is_allowed("execute_bash", PermissionMode::DontAsk));
        assert!(!catalog.is_allowed("delete_path", PermissionMode::DontAsk));
    }

    #[test]
    fn test_permission_catalog_accept_edits_mode() {
        let catalog = PermissionCatalog::new();
        assert!(catalog.is_allowed("write_file", PermissionMode::AcceptEdits));
        assert!(!catalog.is_allowed("execute_bash", PermissionMode::AcceptEdits));
        assert!(!catalog.is_allowed("delete_path", PermissionMode::AcceptEdits));
    }

    #[test]
    fn test_permission_catalog_dangerous_tools() {
        let catalog = PermissionCatalog::new();
        assert!(catalog.needs_approval("execute_bash", PermissionMode::AcceptEdits));
        assert!(catalog.needs_approval("delete_path", PermissionMode::AcceptEdits));
        assert!(catalog.needs_approval("spawn_subagent", PermissionMode::AcceptEdits));
    }

    #[test]
    fn test_approval_policy_default_mode() {
        let policy = ApprovalPolicy::new(PermissionMode::Ask);
        // In Ask mode, nothing is auto-approved
        assert_eq!(policy.check("read_file", "{}"), PermissionTier::Ask);
    }

    #[test]
    fn test_approval_policy_plan_mode() {
        let policy = ApprovalPolicy::new(PermissionMode::Plan);
        assert_eq!(policy.check("read_file", "{}"), PermissionTier::Allowed);
        assert_eq!(policy.check("write_file", "{}"), PermissionTier::Denied);
        assert_eq!(policy.check("execute_bash", "{}"), PermissionTier::Denied);
    }

    #[test]
    fn test_approval_policy_dont_ask_mode() {
        let policy = ApprovalPolicy::new(PermissionMode::DontAsk);
        assert_eq!(policy.check("read_file", "{}"), PermissionTier::Allowed);
        assert_eq!(policy.check("write_file", "{}"), PermissionTier::Allowed);
        assert_eq!(policy.check("execute_bash", "{}"), PermissionTier::Denied);
    }

    #[test]
    fn test_approval_policy_bypass_mode() {
        let policy = ApprovalPolicy::new(PermissionMode::BypassPermissions);
        assert_eq!(policy.check("read_file", "{}"), PermissionTier::Allowed);
        assert_eq!(
            policy.check("run_shell_command", "rm -rf /"),
            PermissionTier::Allowed
        );
        assert_eq!(policy.check("delete_path", "/"), PermissionTier::Allowed);
    }

    #[test]
    fn test_approval_policy_deny_list() {
        let mut policy = ApprovalPolicy::new(PermissionMode::BypassPermissions);
        policy.add_deny("run_shell_command:*rm*");
        assert_eq!(
            policy.check("run_shell_command", "rm -rf /"),
            PermissionTier::Denied
        );
        assert_eq!(
            policy.check("run_shell_command", "ls -la"),
            PermissionTier::Allowed
        );
    }

    #[test]
    fn test_approval_policy_allow_list() {
        let mut policy = ApprovalPolicy::new(PermissionMode::Ask);
        policy.add_allow("read_file:*");
        assert_eq!(
            policy.check("read_file", "Cargo.toml"),
            PermissionTier::Allowed
        );
    }

    #[test]
    fn test_approval_policy_session_allow() {
        let mut policy = ApprovalPolicy::new(PermissionMode::Ask);
        policy.add_session_allow("execute_bash:git *".to_string());
        assert_eq!(
            policy.check("execute_bash", "git status"),
            PermissionTier::Allowed
        );
        assert_eq!(
            policy.check("execute_bash", "npm install"),
            PermissionTier::Ask
        );
    }

    #[test]
    fn test_approval_policy_session_allow_deduplication() {
        let mut policy = ApprovalPolicy::new(PermissionMode::Ask);
        policy.add_session_allow("execute_bash:git *".to_string());
        policy.add_session_allow("execute_bash:git *".to_string());
        // Should only have one entry - verify no panic on check
        assert_eq!(
            policy.check("execute_bash", "git status"),
            PermissionTier::Allowed
        );
    }

    #[test]
    fn test_suggest_allow_pattern_bash_git() {
        let pattern =
            ApprovalPolicy::suggest_allow_pattern("execute_bash", r#"{"command": "git status"}"#);
        assert!(pattern.contains("git"));
    }

    #[test]
    fn test_suggest_allow_pattern_bash_npm() {
        let pattern = ApprovalPolicy::suggest_allow_pattern(
            "execute_bash",
            r#"{"command": "npm install express"}"#,
        );
        assert!(pattern.contains("npm"));
        assert!(pattern.contains('*'));
    }

    #[test]
    fn test_suggest_allow_pattern_path() {
        let pattern = ApprovalPolicy::suggest_allow_pattern(
            "write_file",
            r#"{"path": "src/main.rs", "content": "fn main() {}"}"#,
        );
        assert!(pattern.contains("src"));
    }

    #[test]
    fn test_mode_guidance() {
        let catalog = PermissionCatalog::new();
        assert!(catalog.mode_guidance(PermissionMode::Ask).contains("ask"));
        assert!(catalog.mode_guidance(PermissionMode::Plan).contains("plan"));
        assert!(catalog
            .mode_guidance(PermissionMode::DontAsk)
            .contains("dont-ask"));
        assert!(catalog
            .mode_guidance(PermissionMode::AcceptEdits)
            .contains("accept-edits"));
        assert!(catalog
            .mode_guidance(PermissionMode::BypassPermissions)
            .contains("bypass"));
    }

    #[test]
    fn test_is_read_only() {
        let plan_policy = ApprovalPolicy::new(PermissionMode::Plan);
        assert!(plan_policy.is_read_only());

        let bypass_policy = ApprovalPolicy::new(PermissionMode::BypassPermissions);
        assert!(!bypass_policy.is_read_only());
    }

    #[test]
    fn test_should_fail_on_ask() {
        let ask_policy = ApprovalPolicy::new(PermissionMode::Ask);
        assert!(ask_policy.should_fail_on_ask());

        let bypass_policy = ApprovalPolicy::new(PermissionMode::BypassPermissions);
        assert!(!bypass_policy.should_fail_on_ask());
    }
}
