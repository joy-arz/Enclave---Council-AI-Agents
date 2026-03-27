//! Tool call parser - extracts tool calls from model responses

use super::ToolCall;
use std::collections::HashSet;

/// Parse tool calls from model response text
/// Supports:
/// 1. JSON code blocks: ```json {"name": "tool", "arguments": {...}} ```
/// 2. Plain function calls: list_directory({"path": "."})
/// 3. Natural language: "use list_directory to see files" or "I'll use read_file"
pub fn parse_tool_calls(response: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();

    // Try JSON format first
    calls.extend(parse_json_blocks(response));

    // Try plain function call format
    calls.extend(parse_plain_calls(response));

    // Try to detect natural language tool usage
    calls.extend(parse_natural_language(response));

    // Deduplicate
    let mut seen = HashSet::new();
    calls.retain(|call| {
        let key = format!("{}:{}", call.name, call.arguments.to_string());
        seen.insert(key)
    });

    calls
}

/// Detect natural language mentions of tool usage
fn parse_natural_language(response: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    let tool_names = ["list_directory", "read_file", "write_file", "run_shell_command", "grep"];

    for tool_name in &tool_names {
        // Pattern variations that indicate tool use:
        // "use list_directory", "use the list_directory", "use list_directory tool"
        // "I'll use list_directory", "I will use list_directory"
        let patterns = [
            format!("use {}(", tool_name),
            format!("use the {}(", tool_name),
            format!("use the {} tool", tool_name),
            format!("I'll use {}(", tool_name),
            format!("I will use {}(", tool_name),
            format!("I need to use {}(", tool_name),
            format!("I should use {}(", tool_name),
            format!("use {} to", tool_name),
            format!("use {} tool", tool_name),
        ];

        for pattern in &patterns {
            if response.contains(pattern) || response.contains(&format!("{} tool", tool_name)) {
                // Try to extract arguments from context
                let default_args = match *tool_name {
                    "list_directory" => serde_json::json!({"path": "."}),
                    "read_file" => serde_json::json!({"path": "."}),
                    "write_file" => serde_json::json!({"path": "", "content": ""}),
                    "run_shell_command" => serde_json::json!({"command": "ls"}),
                    "grep" => serde_json::json!({"pattern": "", "path": "."}),
                    _ => serde_json::json!({}),
                };
                calls.push(ToolCall {
                    name: tool_name.to_string(),
                    arguments: default_args,
                });
                break;
            }
        }
    }

    calls
}

fn parse_json_blocks(response: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();

    // Find all JSON code blocks
    let mut search_start = 0;
    while let Some(json_start) = response[search_start..].find("```json") {
        let block_start = search_start + json_start + 6;
        if let Some(block_end) = response[block_start..].find("```") {
            let json_content = &response[block_start..block_start + block_end];
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_content) {
                if let Some(call) = extract_tool_from_json(&parsed) {
                    calls.push(call);
                }
            }
            search_start = block_start + block_end;
        } else {
            break;
        }
    }

    // Also look for raw JSON objects with name and arguments
    let raw_regex = regex::Regex::new(r#"\{[^{}]*"name"\s*:\s*"([^"]+)"[^{}]*"arguments"\s*:\s*(\{[^{}]*(?:\{[^{}]*\}[^{}]*)*\})[^{}]*\}"#).unwrap();
    for cap in raw_regex.captures_iter(response) {
        if let (Some(name_m), Some(args_m)) = (cap.get(1), cap.get(2)) {
            let name = name_m.as_str().to_string();
            let args_str = format!("{{{}}}", args_m.as_str());
            if let Ok(args) = serde_json::from_str(&args_str) {
                calls.push(ToolCall { name, arguments: args });
            }
        }
    }

    calls
}

fn parse_plain_calls(response: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    let tool_names = ["read_file", "write_file", "run_shell_command", "list_directory", "grep"];

    for tool_name in &tool_names {
        let pattern = format!("{}(", tool_name);
        let mut search_start = 0;

        while let Some(call_start) = response[search_start..].find(&pattern) {
            let call_start = search_start + call_start;
            // Find the opening brace
            if let Some(arg_start) = response[call_start..].find('{') {
                let arg_start = call_start + arg_start;
                // Find matching closing brace
                let mut depth = 0;
                let mut arg_end = arg_start;
                for (i, c) in response[arg_start..].chars().enumerate() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                arg_end = arg_start + i;
                                break;
                            }
                        }
                        _ => {}
                    }
                }

                let json_str = &response[arg_start..=arg_end];
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
                    if let Some(call) = extract_tool_from_json(&parsed) {
                        calls.push(call);
                    }
                }

                search_start = arg_end;
            } else {
                break;
            }
        }
    }

    calls
}

fn extract_tool_from_json(value: &serde_json::Value) -> Option<ToolCall> {
    // Check for direct name/arguments
    if let Some(name) = value.get("name").or_else(|| value.get("tool")).and_then(|v| v.as_str()) {
        let arguments = value.get("arguments")
            .or_else(|| value.get("args"))
            .or_else(|| value.get("parameters"))
            .cloned()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        return Some(ToolCall { name: name.to_string(), arguments });
    }

    // Check for tool_calls array
    if let Some(tool_calls) = value.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tool_calls {
            if let Some(call) = extract_tool_from_json(tc) {
                return Some(call);
            }
        }
    }

    None
}

/// Extract text content, removing tool call blocks
#[allow(dead_code)]
pub fn extract_text_content(response: &str) -> String {
    let mut result = response.to_string();

    // Remove JSON code blocks
    while let Some(start) = result.find("```json") {
        if let Some(end) = result[start..].find("```") {
            result = format!("{}{}", &result[..start], &result[start + end + 3..]);
        } else {
            break;
        }
    }

    // Remove XML tool call tags
    let xml_regex = regex::Regex::new(r"<tool_call>\s*\{[^}]+\}\s*</tool_call>").unwrap();
    result = xml_regex.replace_all(&result, "").to_string();

    // Remove plain function call patterns
    let tool_names = ["read_file", "write_file", "run_shell_command", "list_directory", "grep"];
    for name in &tool_names {
        let pattern = format!("{}({})", name, r#"(\{[^{}]*(?:\{[^{}]*\}[^{}]*)*\})"#);
        if let Ok(re) = regex::Regex::new(&pattern) {
            result = re.replace_all(&result, "").to_string();
        }
    }

    // Clean up extra whitespace
    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }

    result.trim().to_string()
}
