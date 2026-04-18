use crate::core::tools::ToolResult;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAX_POOL_SIZE: usize = 5;
const POOL_TTL: Duration = Duration::from_secs(60);

type Pool = HashMap<String, (McpClient, Instant)>;

static MCP_POOL: Lazy<Mutex<Pool>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub enabled: bool,
}

/// MCP tool schema from server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolSchema {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "inputSchema", alias = "input_schema")]
    pub input_schema: McpInputSchema,
}

/// Input schema for MCP tool
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct McpInputSchema {
    #[serde(default)]
    pub properties: Option<serde_json::Map<String, Value>>,
    #[serde(default)]
    pub required: Option<Vec<String>>,
}

/// MCP client for communicating with MCP servers via JSON-RPC over stdio
pub struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    /// Spawn an MCP server as a child process
    pub fn spawn(workspace_root: &Path, server_config: &McpServerConfig) -> Result<Self, String> {
        let mut command = Command::new(&server_config.command);
        command
            .args(&server_config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let working_directory = server_config
            .cwd
            .clone()
            .unwrap_or_else(|| workspace_root.to_path_buf());
        command.current_dir(working_directory);

        for (key, value) in &server_config.env {
            command.env(key, value);
        }

        let mut child = command
            .spawn()
            .map_err(|err| format!("failed to start MCP server `{}`: {err}", server_config.name))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("missing stdin for MCP server `{}`", server_config.name))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("missing stdout for MCP server `{}`", server_config.name))?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    /// Send initialize request to MCP server
    pub fn initialize(&mut self) -> Result<(), String> {
        self.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "council-agent",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }),
        )?;

        self.notify(
            "notifications/initialized",
            Value::Object(Default::default()),
        )?;

        Ok(())
    }

    /// List available tools from the MCP server
    pub fn list_tools(&mut self) -> Result<Vec<McpToolSchema>, String> {
        let result = self.request("tools/list", serde_json::json!({}))?;

        serde_json::from_value(
            result
                .get("tools")
                .cloned()
                .unwrap_or_else(|| Value::Array(vec![])),
        )
        .map_err(|err| format!("failed to decode MCP tool list: {err}"))
    }

    /// Call an MCP tool with the given name and arguments
    pub fn call_tool(&mut self, tool_name: &str, input: Value) -> Result<Value, String> {
        self.request(
            "tools/call",
            serde_json::json!({
                "name": tool_name,
                "arguments": input,
            }),
        )
    }

    /// Gracefully shutdown the MCP server
    pub fn shutdown(&mut self) {
        let _ = self.request("shutdown", Value::Null);
        let _ = self.notify("exit", Value::Null);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Send a notification (no response expected)
    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let value = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_message(&value)
    }

    /// Send a request and wait for response
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;

        let value = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        self.write_message(&value)?;

        loop {
            let response = self.read_message()?;
            if response.get("id") == Some(&Value::from(id)) {
                if let Some(error) = response.get("error") {
                    return Err(format!("MCP error: {}", error));
                }
                return Ok(response
                    .get("result")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default())));
            }
        }
    }

    fn write_message(&mut self, value: &Value) -> Result<(), String> {
        let line = serde_json::to_string(value).map_err(|err| err.to_string())?;
        writeln!(self.stdin, "{line}").map_err(|err| err.to_string())?;
        self.stdin.flush().map_err(|err| err.to_string())
    }

    fn read_message(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .map_err(|err| err.to_string())?;
        serde_json::from_str(&line).map_err(|err| err.to_string())
    }
}

struct PooledClient {
    key: String,
    client: Option<McpClient>,
}

impl PooledClient {
    fn new(key: String, client: McpClient) -> Self {
        Self {
            key,
            client: Some(client),
        }
    }

    fn return_to_pool(&mut self) {
        if let Some(client) = self.client.take() {
            let mut pool = MCP_POOL.lock().unwrap();
            if pool.len() < MAX_POOL_SIZE {
                pool.insert(self.key.clone(), (client, Instant::now()));
            }
        }
    }
}

impl Drop for PooledClient {
    fn drop(&mut self) {
        if let Some(mut client) = self.client.take() {
            let mut pool = MCP_POOL.lock().unwrap();
            if pool.len() < MAX_POOL_SIZE {
                pool.insert(self.key.clone(), (client, Instant::now()));
            } else {
                client.shutdown();
            }
        }
    }
}

fn get_or_spawn(
    workspace_root: &Path,
    server_config: &McpServerConfig,
) -> Result<PooledClient, String> {
    let key = server_config.name.clone();
    let mut pool = MCP_POOL.lock().unwrap();

    if let Some((_, (old_client, last_used))) = pool.remove_entry(&key) {
        if Instant::now().duration_since(last_used) < POOL_TTL {
            return Ok(PooledClient::new(key, old_client));
        }
    }

    drop(pool);
    let client = McpClient::spawn(workspace_root, server_config)?;
    Ok(PooledClient::new(server_config.name.clone(), client))
}

/// Execute an MCP tool call and return the result
pub fn execute_mcp_tool(
    workspace_root: &Path,
    server_config: &McpServerConfig,
    tool_name: &str,
    input: Value,
) -> ToolResult {
    let mut client = match get_or_spawn(workspace_root, server_config) {
        Ok(client) => client,
        Err(err) => {
            return ToolResult {
                name: tool_name.to_string(),
                success: false,
                output: String::new(),
                error: Some(format!("Failed to spawn MCP server: {}", err)),
            };
        }
    };

    if let Err(err) = client.client.as_mut().unwrap().initialize() {
        return ToolResult {
            name: tool_name.to_string(),
            success: false,
            output: String::new(),
            error: Some(format!("Failed to initialize MCP server: {}", err)),
        };
    }

    match client.client.as_mut().unwrap().call_tool(tool_name, input) {
        Ok(result) => {
            client.return_to_pool();
            let output = serde_json::to_string(&result).unwrap_or_else(|_| String::new());
            ToolResult {
                name: tool_name.to_string(),
                success: true,
                output: output.to_string(),
                error: None,
            }
        }
        Err(err) => {
            if let Some(mut c) = client.client.take() {
                c.shutdown();
            }
            ToolResult {
                name: tool_name.to_string(),
                success: false,
                output: String::new(),
                error: Some(format!("MCP tool call failed: {}", err)),
            }
        }
    }
}

/// Parse an MCP tool name to extract server and tool names
/// Format: mcp__server__tool
/// Returns None if the name does not match the MCP pattern
fn parse_mcp_tool_name(full_name: &str) -> Option<(String, String)> {
    let prefix = "mcp__";
    if !full_name.starts_with(prefix) {
        return None;
    }

    let remainder = &full_name[prefix.len()..];
    let parts: Vec<&str> = remainder.split("__").collect();

    if parts.len() != 2 {
        return None;
    }

    Some((parts[0].to_string(), parts[1].to_string()))
}

/// Get the prefixed name for an MCP tool
fn mcp_tool_name(server_name: &str, tool_name: &str) -> String {
    format!("mcp__{}__{}", server_name, tool_name)
}

/// Get MCP servers from environment variables
/// Uses MCP_CONFIG JSON or MCP_SERVERS format
fn get_mcp_servers() -> Vec<McpServerConfig> {
    let mut servers = Vec::new();

    if let Ok(config_json) = std::env::var("MCP_CONFIG") {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&config_json) {
            if let Some(server_list) = parsed.get("servers").and_then(|s| s.as_array()) {
                for server in server_list {
                    if let Some(config) = parse_mcp_server_config(server) {
                        servers.push(config);
                    }
                }
            }
        }
    }

    if let Ok(mcp_servers) = std::env::var("MCP_SERVERS") {
        for server_str in mcp_servers.split('|') {
            if let Some(config) = parse_mcp_server_from_string(server_str) {
                servers.push(config);
            }
        }
    }

    servers
}

/// Parse MCP server config from JSON value
fn parse_mcp_server_config(value: &serde_json::Value) -> Option<McpServerConfig> {
    let name = value.get("name")?.as_str()?.to_string();
    let command = value.get("command")?.as_str()?.to_string();
    let args = value
        .get("args")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let env = value
        .get("env")
        .and_then(|e| e.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let cwd = value.get("cwd").and_then(|c| c.as_str()).map(PathBuf::from);
    let enabled = value
        .get("enabled")
        .map(|e| e.as_bool().unwrap_or(true))
        .unwrap_or(true);

    Some(McpServerConfig {
        name,
        command,
        args,
        env,
        cwd,
        enabled,
    })
}

/// Parse MCP server config from string format:
/// name:command:arg1,arg2:env_key=val
fn parse_mcp_server_from_string(s: &str) -> Option<McpServerConfig> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() < 2 {
        return None;
    }

    let name = parts[0].to_string();
    let command = parts[1].to_string();

    let args: Vec<String> = if parts.len() > 2 {
        parts[2]
            .split(',')
            .filter(|s| !s.is_empty() && !s.contains('='))
            .map(String::from)
            .collect()
    } else {
        Vec::new()
    };

    let mut env = HashMap::new();
    if parts.len() > 2 {
        for part in parts[2].split(',') {
            if let Some((key, val)) = part.split_once('=') {
                if !key.is_empty() {
                    env.insert(key.to_string(), val.to_string());
                }
            }
        }
    }
    if parts.len() > 3 {
        for part in parts[3].split(',') {
            if let Some((key, val)) = part.split_once('=') {
                if !key.is_empty() {
                    env.insert(key.to_string(), val.to_string());
                }
            }
        }
    }

    Some(McpServerConfig {
        name,
        command,
        args,
        env,
        cwd: None,
        enabled: true,
    })
}

/// Try to execute a tool as an MCP tool
/// Returns Some(ToolResult) if the tool name matches MCP naming pattern and execution succeeds/fails
/// Returns None if the tool name does not match MCP pattern
pub fn execute_mcp_tool_matching(
    tool_name: &str,
    args: &serde_json::Value,
    workspace_dir: &Path,
) -> Option<ToolResult> {
    let (server_name, tool_name_only) = parse_mcp_tool_name(tool_name)?;

    let servers = get_mcp_servers();

    let server_config = servers
        .iter()
        .find(|s| s.name == server_name && s.enabled)?;

    Some(execute_mcp_tool(
        workspace_dir,
        server_config,
        &tool_name_only,
        args.clone(),
    ))
}

/// Load all MCP tools from configured servers and return as tool definitions
/// This function connects to each MCP server, lists available tools,
/// and returns them in the format expected by the agent system
pub fn get_mcp_tool_definitions() -> Vec<crate::core::tools::ToolDefinition> {
    let servers = get_mcp_servers();
    let mut tools = Vec::new();

    for server in servers.iter().filter(|s| s.enabled) {
        let mut client = match get_or_spawn(std::path::Path::new("/tmp"), server) {
            Ok(client) => client,
            Err(e) => {
                tracing::warn!("Failed to spawn MCP server '{}': {}", server.name, e);
                continue;
            }
        };

        let client_mut = client.client.as_mut().unwrap();
        if client_mut.initialize().is_err() {
            continue;
        }

        match client_mut.list_tools() {
            Ok(mcp_tools) => {
                for mcp_tool in mcp_tools {
                    let full_name = mcp_tool_name(&server.name, &mcp_tool.name);
                    let description = mcp_tool.description.unwrap_or_else(|| {
                        format!(
                            "MCP tool '{}' from server '{}'",
                            mcp_tool.name, server.name
                        )
                    });

                    let mut parameters = Vec::new();
                    if let Some(props) = mcp_tool.input_schema.properties {
                        for (name, prop) in props {
                            let param_type = prop
                                .as_object()
                                .and_then(|o| o.get("type"))
                                .and_then(|t| t.as_str())
                                .unwrap_or("string")
                                .to_string();

                            let description = prop
                                .as_object()
                                .and_then(|o| o.get("description"))
                                .and_then(|d| d.as_str())
                                .unwrap_or("")
                                .to_string();

                            let required = mcp_tool
                                .input_schema
                                .required
                                .as_ref()
                                .map(|r| r.contains(&name))
                                .unwrap_or(false);

                            parameters.push(crate::core::tools::ToolParam {
                                name,
                                description,
                                param_type,
                                required,
                            });
                        }
                    }

                    tools.push(crate::core::tools::ToolDefinition {
                        name: full_name,
                        description,
                        parameters,
                    });
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to list tools from MCP server '{}': {}",
                    server.name,
                    e
                );
            }
        }

        client.return_to_pool();
    }

    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mcp_tool_name() {
        assert_eq!(
            parse_mcp_tool_name("mcp__filesystem__read_file"),
            Some(("filesystem".to_string(), "read_file".to_string()))
        );

        assert_eq!(
            parse_mcp_tool_name("mcp__server__tool"),
            Some(("server".to_string(), "tool".to_string()))
        );

        assert_eq!(parse_mcp_tool_name("read_file"), None);
        assert_eq!(parse_mcp_tool_name("mcp__invalid"), None);
        assert_eq!(parse_mcp_tool_name("mcp__"), None);
    }

    #[test]
    fn test_mcp_tool_name() {
        assert_eq!(mcp_tool_name("server", "tool"), "mcp__server__tool");
    }
}
