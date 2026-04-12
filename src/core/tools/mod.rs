//! Tool system for agent execution
//! Implements tools for workspace interaction including MCP support

pub mod mcp_client;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use tokio::process::Command;

/// Represents a tool call from the model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Result of executing a tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub name: String,
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

/// Tool definitions available to agents
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Vec<ToolParam>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ToolParam {
    pub name: String,
    pub description: String,
    pub param_type: String,
    pub required: bool,
}

/// Get all available tool definitions
#[allow(dead_code)]
pub fn get_tool_definitions() -> Vec<ToolDefinition> {
    let mut tools = vec![
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read the contents of a file from the workspace".to_string(),
            parameters: vec![
                ToolParam {
                    name: "path".to_string(),
                    description: "Relative path to the file from workspace root".to_string(),
                    param_type: "string".to_string(),
                    required: true,
                },
                ToolParam {
                    name: "limit".to_string(),
                    description: "Maximum number of lines to read (default: 100)".to_string(),
                    param_type: "number".to_string(),
                    required: false,
                },
                ToolParam {
                    name: "offset".to_string(),
                    description: "Line number to start reading from (default: 0)".to_string(),
                    param_type: "number".to_string(),
                    required: false,
                },
            ],
        },
        ToolDefinition {
            name: "write_file".to_string(),
            description: "Create or overwrite a file in the workspace".to_string(),
            parameters: vec![
                ToolParam {
                    name: "path".to_string(),
                    description: "Relative path to the file from workspace root".to_string(),
                    param_type: "string".to_string(),
                    required: true,
                },
                ToolParam {
                    name: "content".to_string(),
                    description: "Content to write to the file".to_string(),
                    param_type: "string".to_string(),
                    required: true,
                },
            ],
        },
        ToolDefinition {
            name: "run_shell_command".to_string(),
            description: "Execute a shell command in the workspace directory".to_string(),
            parameters: vec![
                ToolParam {
                    name: "command".to_string(),
                    description: "Shell command to execute (e.g., 'ls -la', 'cargo build')"
                        .to_string(),
                    param_type: "string".to_string(),
                    required: true,
                },
                ToolParam {
                    name: "timeout".to_string(),
                    description: "Timeout in seconds (default: 30)".to_string(),
                    param_type: "number".to_string(),
                    required: false,
                },
            ],
        },
        ToolDefinition {
            name: "list_directory".to_string(),
            description: "List contents of a directory".to_string(),
            parameters: vec![ToolParam {
                name: "path".to_string(),
                description: "Relative path to directory (default: '.')".to_string(),
                param_type: "string".to_string(),
                required: false,
            }],
        },
        ToolDefinition {
            name: "grep".to_string(),
            description: "Search for a pattern in files".to_string(),
            parameters: vec![
                ToolParam {
                    name: "pattern".to_string(),
                    description: "Text pattern or regex to search for".to_string(),
                    param_type: "string".to_string(),
                    required: true,
                },
                ToolParam {
                    name: "path".to_string(),
                    description: "Directory to search in (default: '.')".to_string(),
                    param_type: "string".to_string(),
                    required: false,
                },
                ToolParam {
                    name: "file_pattern".to_string(),
                    description: "File pattern to match (e.g., '*.rs', '*.js')".to_string(),
                    param_type: "string".to_string(),
                    required: false,
                },
            ],
        },
    ];

    // Add MCP tools from configured servers
    let mcp_tools = mcp_client::get_mcp_tool_definitions();
    tools.extend(mcp_tools);

    tools
}

/// Convert tool definitions to JSON for system prompt
/// Note: MiniMax uses "input_schema" instead of "parameters" for Anthropic compatibility
#[allow(dead_code)]
pub fn get_tools_json() -> String {
    let tools = get_tool_definitions();
    let mut json = String::from("[\n");
    for (i, tool) in tools.iter().enumerate() {
        if i > 0 {
            json.push_str(",\n");
        }
        json.push_str(&format!(
            "{{ \"name\": \"{}\", \"description\": \"{}\", \"input_schema\": {{",
            tool.name, tool.description
        ));

        let params: Vec<String> = tool
            .parameters
            .iter()
            .map(|param| {
                format!(
                    "\"{}\": {{ \"type\": \"{}\", \"description\": \"{}\", \"required\": {} }}",
                    param.name, param.param_type, param.description, param.required
                )
            })
            .collect();

        json.push_str(&params.join(", "));
        json.push_str("}}");
        json.push('}');
    }
    json.push_str("\n]");
    json
}

/// Execute a tool call with optional approval policy
/// If policy is Some and tool requires approval, returns ToolResult with error "PENDING_APPROVAL"
pub async fn execute_tool(
    tool_call: &ToolCall,
    workspace_dir: &PathBuf,
    policy: Option<&crate::core::approval::ApprovalPolicy>,
) -> ToolResult {
    let name = &tool_call.name;
    let args = &tool_call.arguments;

    // Security: validate argument size to prevent memory exhaustion
    let args_size = args.to_string().len();
    if args_size > crate::utils::constants::MAX_TOOL_ARGUMENT_SIZE {
        return ToolResult {
            name: name.clone(),
            success: false,
            output: String::new(),
            error: Some(format!(
                "Tool argument too large ({} bytes, max {}). Refusing to execute.",
                args_size,
                crate::utils::constants::MAX_TOOL_ARGUMENT_SIZE
            )),
        };
    }

    // Check approval policy if provided
    if let Some(p) = policy {
        let tool_input = args.to_string();
        let tier = p.check(name, &tool_input);
        match tier {
            crate::core::approval::PermissionTier::Denied => {
                return ToolResult {
                    name: name.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("Tool '{}' denied by approval policy", name)),
                };
            }
            crate::core::approval::PermissionTier::Ask => {
                return ToolResult {
                    name: name.clone(),
                    success: false,
                    output: String::new(),
                    error: Some("PENDING_APPROVAL".to_string()),
                };
            }
            crate::core::approval::PermissionTier::Allowed => {
                // Proceed with execution
            }
        }
    }

    match name.as_str() {
        "read_file" => execute_read_file(args, workspace_dir).await,
        "write_file" => execute_write_file(args, workspace_dir).await,
        "run_shell_command" => execute_shell_command(args, workspace_dir).await,
        "list_directory" => execute_list_directory(args, workspace_dir).await,
        "grep" => execute_grep(args, workspace_dir).await,
        _ => {
            // Check if this is an MCP tool call (format: mcp__server__tool)
            if let Some(mcp_result) =
                mcp_client::execute_mcp_tool_matching(name, args, workspace_dir)
            {
                return mcp_result;
            }
            ToolResult {
                name: name.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("Unknown tool: {}", name)),
            }
        }
    }
}

async fn execute_read_file(
    args: &serde_json::Value,
    workspace_dir: &std::path::Path,
) -> ToolResult {
    // Accept both "path" and "absolute_path"
    let path = args
        .get("path")
        .or_else(|| args.get("absolute_path"))
        .and_then(|v| v.as_str());

    let path = match path {
        Some(p) => p,
        None => {
            return ToolResult {
                name: "read_file".to_string(),
                success: false,
                output: String::new(),
                error: Some("Missing required parameter: path".to_string()),
            };
        }
    };

    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

    // Security: reject absolute paths and path traversal attempts
    let path_obj = std::path::Path::new(path);
    if path_obj.is_absolute() {
        return ToolResult {
            name: "read_file".to_string(),
            success: false,
            output: String::new(),
            error: Some("Security violation: absolute paths not allowed".to_string()),
        };
    }

    // Check for path traversal attempts
    if path_obj.components().any(|c| c.as_os_str() == "..") {
        return ToolResult {
            name: "read_file".to_string(),
            success: false,
            output: String::new(),
            error: Some("Security violation: path traversal not allowed".to_string()),
        };
    }

    let full_path = workspace_dir.join(path);

    // Canonicalize and verify path stays within workspace
    let resolved_path = match full_path.canonicalize() {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return ToolResult {
                name: "read_file".to_string(),
                success: false,
                output: String::new(),
                error: Some(format!("File not found: {}", path)),
            };
        }
        Err(e) => {
            return ToolResult {
                name: "read_file".to_string(),
                success: false,
                output: String::new(),
                error: Some(format!("Failed to resolve path: {}", e)),
            };
        }
    };

    let workspace_resolved = match workspace_dir.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return ToolResult {
                name: "read_file".to_string(),
                success: false,
                output: String::new(),
                error: Some(format!("Workspace resolution error: {}", e)),
            };
        }
    };

    if !resolved_path.starts_with(&workspace_resolved) {
        return ToolResult {
            name: "read_file".to_string(),
            success: false,
            output: String::new(),
            error: Some("Security violation: path escapes workspace".to_string()),
        };
    }

    match fs::read_to_string(&resolved_path).await {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let total_lines = lines.len();
            let start = offset.min(lines.len());
            let end = (offset + limit).min(lines.len());
            let selected: Vec<String> = lines[start..end].iter().map(|s| s.to_string()).collect();
            let output = if total_lines > limit {
                format!(
                    "[Showing lines {}-{} of {}]\n\n{}",
                    start + 1,
                    end,
                    total_lines,
                    selected.join("\n")
                )
            } else {
                selected.join("\n")
            };
            ToolResult {
                name: "read_file".to_string(),
                success: true,
                output,
                error: None,
            }
        }
        Err(e) => ToolResult {
            name: "read_file".to_string(),
            success: false,
            output: String::new(),
            error: Some(format!("Failed to read file: {}", e)),
        },
    }
}

async fn execute_write_file(
    args: &serde_json::Value,
    workspace_dir: &std::path::Path,
) -> ToolResult {
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => {
            return ToolResult {
                name: "write_file".to_string(),
                success: false,
                output: String::new(),
                error: Some("Missing required parameter: path".to_string()),
            };
        }
    };

    let content_raw = match args.get("content").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return ToolResult {
                name: "write_file".to_string(),
                success: false,
                output: String::new(),
                error: Some("Missing required parameter: content".to_string()),
            };
        }
    };

    // Unescape common escape sequences that may come from LLM output
    // e.g., "\n" -> actual newline, "\\n" -> "\n", etc.
    let content_unescaped = content_raw
        .replace("\\n", "\n")
        .replace("\\t", "\t")
        .replace("\\r", "\r")
        .replace("\\\"", "\"")
        .replace("\\\\", "\\");

    let content_for_write = content_unescaped.as_str();

    // Security: reject absolute paths and path traversal attempts
    let path_obj = std::path::Path::new(path);
    if path_obj.is_absolute() {
        return ToolResult {
            name: "write_file".to_string(),
            success: false,
            output: String::new(),
            error: Some("Security violation: absolute paths not allowed".to_string()),
        };
    }

    if path_obj.components().any(|c| c.as_os_str() == "..") {
        return ToolResult {
            name: "write_file".to_string(),
            success: false,
            output: String::new(),
            error: Some("Security violation: path traversal not allowed".to_string()),
        };
    }

    let full_path = workspace_dir.join(path);

    // Create parent directories if needed
    if let Some(parent) = full_path.parent() {
        // Verify parent stays within workspace before creating
        let parent_resolved = if parent.exists() {
            match parent.canonicalize() {
                Ok(p) => p,
                Err(e) => {
                    return ToolResult {
                        name: "write_file".to_string(),
                        success: false,
                        output: String::new(),
                        error: Some(format!("Failed to resolve parent directory: {}", e)),
                    };
                }
            }
        } else {
            // Parent doesn't exist yet - verify the path components are safe
            // by checking the workspace prefix
            workspace_dir.join(parent)
        };

        let workspace_resolved = match workspace_dir.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    name: "write_file".to_string(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("Workspace resolution error: {}", e)),
                };
            }
        };

        if !parent_resolved.starts_with(&workspace_resolved) {
            return ToolResult {
                name: "write_file".to_string(),
                success: false,
                output: String::new(),
                error: Some("Security violation: path escapes workspace".to_string()),
            };
        }

        if !parent.exists() {
            if let Err(e) = fs::create_dir_all(parent).await {
                return ToolResult {
                    name: "write_file".to_string(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to create directory: {}", e)),
                };
            }
        }
    }

    match fs::write(&full_path, content_for_write).await {
        Ok(_) => ToolResult {
            name: "write_file".to_string(),
            success: true,
            output: format!(
                "Successfully wrote {} bytes to {}",
                content_for_write.len(),
                path
            ),
            error: None,
        },
        Err(e) => ToolResult {
            name: "write_file".to_string(),
            success: false,
            output: String::new(),
            error: Some(format!("Failed to write file: {}", e)),
        },
    }
}

/// Dangerous command patterns to block
const DANGEROUS_COMMAND_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    "mkfs",
    "dd if=/dev/zero",
    ":(){:|:&};:",
    "> /dev/",
    "curl * | *sh",
    "wget * | *sh",
    "chmod -R 777 /",
    "chown -R",
    "sudo rm",
    "shutdown",
    "reboot",
    "init 0",
    "telinit 6",
    "echo * > /etc/",
    "echo * > /proc/",
    "echo * > /sys/",
    "fork() {",
    "mv /* /dev/null",
    "cp -rf / /*",
];

/// Check if command contains dangerous patterns
fn is_command_dangerous(command: &str) -> Option<&'static str> {
    let cmd_lower = command.to_lowercase();
    for pattern in DANGEROUS_COMMAND_PATTERNS {
        if cmd_lower.contains(pattern) {
            return Some(*pattern);
        }
    }
    // Also check for attempts to access sensitive paths
    if command.contains("/etc/passwd")
        || command.contains("/etc/shadow")
        || command.contains("~/.ssh")
        || command.contains("/root/")
    {
        return Some("sensitive path access");
    }
    None
}

async fn execute_shell_command(args: &serde_json::Value, workspace_dir: &PathBuf) -> ToolResult {
    let command = match args.get("command").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return ToolResult {
                name: "run_shell_command".to_string(),
                success: false,
                output: String::new(),
                error: Some("Missing required parameter: command".to_string()),
            };
        }
    };

    // Security: Block dangerous commands
    if let Some(_pattern) = is_command_dangerous(command) {
        return ToolResult {
            name: "run_shell_command".to_string(),
            success: false,
            output: String::new(),
            error: Some("Security violation: command blocked".to_string()),
        };
    }

    // Use configurable timeout, capped at max
    let max_timeout = crate::utils::constants::SHELL_TIMEOUT_SECS;
    let timeout_secs = args
        .get("timeout")
        .and_then(|v| v.as_u64())
        .map(|t| t.min(max_timeout))
        .unwrap_or(max_timeout);

    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command).current_dir(workspace_dir);

    // Execute with timeout
    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        cmd.output(),
    )
    .await
    {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            return ToolResult {
                name: "run_shell_command".to_string(),
                success: false,
                output: String::new(),
                error: Some(format!("Failed to execute command: {}", e)),
            };
        }
        Err(_) => {
            return ToolResult {
                name: "run_shell_command".to_string(),
                success: false,
                output: String::new(),
                error: Some(format!("Command timed out after {} seconds", timeout_secs)),
            };
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let status = output.status;

    let out_str = if !stdout.is_empty() {
        format!("=== STDOUT ===\n{}\n", stdout)
    } else {
        String::new()
    };

    let out_str = if !stderr.is_empty() {
        format!("{}=== STDERR ===\n{}\n", out_str, stderr)
    } else {
        out_str
    };

    let out_str = format!(
        "{}{} exited with code {}",
        out_str,
        if status.success() {
            "Command succeeded"
        } else {
            "Command failed"
        },
        status
            .code()
            .map(|c: i32| c.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    );

    ToolResult {
        name: "run_shell_command".to_string(),
        success: status.success(),
        output: out_str,
        error: None,
    }
}

async fn execute_list_directory(args: &serde_json::Value, workspace_dir: &PathBuf) -> ToolResult {
    // Accept both "path" and "absolute_path"
    let path = args
        .get("path")
        .or_else(|| args.get("absolute_path"))
        .and_then(|v| v.as_str())
        .unwrap_or(".");

    // SECURITY: Validate path stays within workspace (prevent traversal)
    let full_path = if PathBuf::from(path).is_absolute() {
        PathBuf::from(path)
    } else {
        workspace_dir.join(path)
    };

    // Check that resolved path is within workspace
    let resolved = match full_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return ToolResult {
                name: "list_directory".to_string(),
                success: false,
                output: String::new(),
                error: Some(format!("Directory not found or inaccessible: {}", e)),
            };
        }
    };

    let workspace_resolved = match workspace_dir.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return ToolResult {
                name: "list_directory".to_string(),
                success: false,
                output: String::new(),
                error: Some(format!("Workspace error: {}", e)),
            };
        }
    };

    if !resolved.starts_with(&workspace_resolved) {
        return ToolResult {
            name: "list_directory".to_string(),
            success: false,
            output: String::new(),
            error: Some("Security violation: path escapes workspace".to_string()),
        };
    }

    // Use -- to prevent option injection (path can't start with -)
    let mut cmd = Command::new("ls");
    cmd.arg("-la");
    cmd.arg("--"); // Separator - everything after is a path, not an option
    cmd.arg(path); // Use original path since we validated above
    cmd.current_dir(workspace_dir);

    let output = cmd.output().await;

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            ToolResult {
                name: "list_directory".to_string(),
                success: true,
                output: if stdout.is_empty() {
                    "(empty directory)".to_string()
                } else {
                    stdout.to_string()
                },
                error: None,
            }
        }
        Err(e) => ToolResult {
            name: "list_directory".to_string(),
            success: false,
            output: String::new(),
            error: Some(format!("Failed to list directory: {}", e)),
        },
    }
}

async fn execute_grep(args: &serde_json::Value, workspace_dir: &PathBuf) -> ToolResult {
    let pattern = match args.get("pattern").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => {
            return ToolResult {
                name: "grep".to_string(),
                success: false,
                output: String::new(),
                error: Some("Missing required parameter: pattern".to_string()),
            };
        }
    };

    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let file_pattern = args
        .get("file_pattern")
        .and_then(|v| v.as_str())
        .unwrap_or("*");

    let mut cmd = Command::new("grep");
    cmd.arg("-rn");
    cmd.arg("--include=".to_string() + file_pattern);
    cmd.arg(pattern);
    cmd.arg(path);
    cmd.current_dir(workspace_dir);

    let output = cmd.output().await;

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            ToolResult {
                name: "grep".to_string(),
                success: true,
                output: if stdout.is_empty() {
                    "No matches found".to_string()
                } else {
                    stdout.to_string()
                },
                error: None,
            }
        }
        Err(e) => ToolResult {
            name: "grep".to_string(),
            success: false,
            output: String::new(),
            error: Some(format!("Failed to grep: {}", e)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definitions_exist() {
        let tools = get_tool_definitions();
        assert!(!tools.is_empty());
        assert!(tools.iter().any(|t| t.name == "read_file"));
        assert!(tools.iter().any(|t| t.name == "write_file"));
        assert!(tools.iter().any(|t| t.name == "run_shell_command"));
    }
}
