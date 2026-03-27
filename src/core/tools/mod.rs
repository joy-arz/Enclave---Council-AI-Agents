//! Tool system for agent execution
//! Implements MCP-style tools for workspace interaction

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use tokio::process::Command;

pub mod parser;

pub use parser::parse_tool_calls;

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
    vec![
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
                    description: "Shell command to execute (e.g., 'ls -la', 'cargo build')".to_string(),
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
            parameters: vec![
                ToolParam {
                    name: "path".to_string(),
                    description: "Relative path to directory (default: '.')".to_string(),
                    param_type: "string".to_string(),
                    required: false,
                },
            ],
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
    ]
}

/// Convert tool definitions to JSON for system prompt
#[allow(dead_code)]
pub fn get_tools_json() -> String {
    let tools = get_tool_definitions();
    let mut json = String::from("[\n");
    for (i, tool) in tools.iter().enumerate() {
        if i > 0 {
            json.push_str(",\n");
        }
        json.push_str(&format!("{{ \"name\": \"{}\", \"description\": \"{}\", \"parameters\": {{", tool.name, tool.description));
        let mut params = Vec::new();
        for param in &tool.parameters {
            params.push(format!(
                "\"{}\": {{ \"type\": \"{}\", \"description\": \"{}\", \"required\": {} }}",
                param.name, param.param_type, param.description, param.required
            ));
        }
        json.push_str(&params.join(", "));
        json.push_str("}}");
        json.push('}');
    }
    json.push_str("\n]");
    json
}

/// Execute a tool call
pub async fn execute_tool(
    tool_call: &ToolCall,
    workspace_dir: &PathBuf,
) -> ToolResult {
    let name = &tool_call.name;
    let args = &tool_call.arguments;

    match name.as_str() {
        "read_file" => execute_read_file(args, workspace_dir).await,
        "write_file" => execute_write_file(args, workspace_dir).await,
        "run_shell_command" => execute_shell_command(args, workspace_dir).await,
        "list_directory" => execute_list_directory(args, workspace_dir).await,
        "grep" => execute_grep(args, workspace_dir).await,
        _ => ToolResult {
            name: name.clone(),
            success: false,
            output: String::new(),
            error: Some(format!("Unknown tool: {}", name)),
        },
    }
}

async fn execute_read_file(args: &serde_json::Value, workspace_dir: &PathBuf) -> ToolResult {
    // Accept both "path" and "absolute_path"
    let path = args.get("path")
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

    let full_path = workspace_dir.join(path);
    if !full_path.exists() {
        return ToolResult {
            name: "read_file".to_string(),
            success: false,
            output: String::new(),
            error: Some(format!("File not found: {}", path)),
        };
    }

    match fs::read_to_string(&full_path).await {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let total_lines = lines.len();
            let start = offset.min(lines.len());
            let end = (offset + limit).min(lines.len());
            let selected: Vec<String> = lines[start..end].iter().map(|s| s.to_string()).collect();
            let output = if total_lines > limit {
                format!("[Showing lines {}-{} of {}]\n\n{}",
                    start + 1, end, total_lines, selected.join("\n"))
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

async fn execute_write_file(args: &serde_json::Value, workspace_dir: &PathBuf) -> ToolResult {
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

    let content = match args.get("content").and_then(|v| v.as_str()) {
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

    let full_path = workspace_dir.join(path);

    // Create parent directories if needed
    if let Some(parent) = full_path.parent() {
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

    match fs::write(&full_path, content).await {
        Ok(_) => ToolResult {
            name: "write_file".to_string(),
            success: true,
            output: format!("Successfully wrote {} bytes to {}", content.len(), path),
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

    let _timeout_secs = args.get("timeout").and_then(|v| v.as_u64()).unwrap_or(30);

    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(workspace_dir)
        .output()
        .await;

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let status = out.status;

            let output = if !stdout.is_empty() {
                format!("=== STDOUT ===\n{}\n", stdout)
            } else {
                String::new()
            }.to_string();

            let output = if !stderr.is_empty() {
                format!("{}=== STDERR ===\n{}\n", output, stderr)
            } else {
                output
            };

            let output = format!("{}{} exited with code {}",
                output,
                if status.success() { "Command succeeded" } else { "Command failed" },
                status.code().map(|c| c.to_string()).unwrap_or_else(|| "unknown".to_string())
            );

            ToolResult {
                name: "run_shell_command".to_string(),
                success: status.success(),
                output,
                error: None,
            }
        }
        Err(e) => ToolResult {
            name: "run_shell_command".to_string(),
            success: false,
            output: String::new(),
            error: Some(format!("Failed to execute command: {}", e)),
        },
    }
}

async fn execute_list_directory(args: &serde_json::Value, workspace_dir: &PathBuf) -> ToolResult {
    // Accept both "path" and "absolute_path"
    let path = args.get("path")
        .or_else(|| args.get("absolute_path"))
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let full_path = if PathBuf::from(path).is_absolute() {
        PathBuf::from(path)
    } else {
        workspace_dir.join(path)
    };

    if !full_path.exists() {
        return ToolResult {
            name: "list_directory".to_string(),
            success: false,
            output: String::new(),
            error: Some(format!("Directory not found: {}", path)),
        };
    }

    let mut cmd = Command::new("ls");
    cmd.arg("-la");
    cmd.arg(path);
    cmd.current_dir(workspace_dir);

    let output = cmd.output().await;

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            ToolResult {
                name: "list_directory".to_string(),
                success: true,
                output: if stdout.is_empty() { "(empty directory)".to_string() } else { stdout.to_string() },
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
    let file_pattern = args.get("file_pattern").and_then(|v| v.as_str()).unwrap_or("*");

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
