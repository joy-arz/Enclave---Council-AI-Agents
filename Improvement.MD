# Enclave - Code Analysis & Improvement Recommendations

## System Overview

Enclave is a Rust-powered multi-agent council system where AI agents (Architect, Critic, Refactorer, Maintainer, Judge) debate and modify code collaboratively. The system supports multiple LLM providers (OpenAI, Anthropic, MiniMax, OpenRouter) and CLI binaries, with tool-based file operations, MCP server integration, session persistence, and git worktree isolation.

**Intended Usage:** Local-first application. User runs Enclave on their own machine with local workspace access. Security model assumes trusted local environment.

---

## Verified Critical Issues (Subagent-Confirmed)

### 1. Agent Naming Inconsistency

**Files affected:** `src/agents/roles.rs:11-14`, `src/core/orchestrator_mod.rs:139`

**Status:** CONFIRMED - Medium severity (functional bug, just confusing)

The role functions create agents with hardcoded names that don't match their function names:
- `strategist()` creates agent named `"architect"` (line 11)
- `critic()` creates agent named `"reviewer"` (line 28)
- `optimizer()` creates agent named `"refactorer"` (line 45)
- `contrarian()` creates agent named `"maintainer"` (line 62)

Logs print "architect" when the code references "strategist".

---

### 2. Judge Verdict Ignores Terminal Output

**File affected:** `src/agents/judge.rs:25-38`

**Status:** INCORRECT - Issue is wrong. The `call_model` method **does** return terminal output (stdout + stderr combined via `full_terminal` in providers_mod.rs:204-221). The orchestrator **does** use the `terminal` value in `agent_response.terminal_output`. No fix needed.

---

### 3. Synchronous Filesystem in Async Context

**File affected:** `src/core/worktree_mod.rs:29-31`

**Status:** CONFIRMED - High priority

```rust
pub fn is_git_repo(&self) -> bool {
    self.workspace_dir.join(".git").exists()
}
```
Uses blocking `std::fs::exists()` in async codebase. Should use `tokio::fs::metadata` or `spawn_blocking`.

---

### 4. Hardcoded Rate Limit Client IP

**File affected:** `src/api/routes.rs:137`

**Status:** CONFIRMED - Critical for multi-user setups, Low for single-user local use

```rust
let client_ip = "127.0.0.1";  // Always localhost!
```

**Note for Local Usage:** Rate limiting is less critical for single-user local deployments. However, if the API is exposed or used by multiple clients, this is a real issue. Fix when exposing the API externally.

---

### 5. CLI Timeout Ignores max_tokens

**File affected:** `src/core/providers_mod.rs:198`

**Status:** CONFIRMED - High priority

Fixed 600-second timeout regardless of `max_tokens`. A simple request with `max_tokens=100` could wait 10 minutes if CLI hangs.

---

## Verified Security Concerns (Local-Use Adjusted)

**Context:** Since Enclave is designed for local use with trusted workspace access, many security concerns are less severe. The attacker would already need local access or user cooperation.

### 6. Path Traversal via Symlinks

**File affected:** `src/core/tools/mod.rs:310-350`

**Status:** NOT AN ISSUE - Subagent found canonicalization correctly mitigates symlink attacks

The `canonicalize()` method follows symlinks and the `starts_with` check on the resolved path is correct. Symlink-based path traversal is properly prevented.

---

### 7. Dangerous Command Detection is Incomplete

**File affected:** `src/core/tools/mod.rs:522-544`

**Status:** CONFIRMED - Medium priority (mitigated by confinement)

**Local-Use Note:** Commands run in `workspace_dir` via `current_dir(workspace_dir)`, not as root. Shell timeout is 30 seconds.

**Missing patterns:** `sudo -s`, `chmod 777 *`, `dd of=/dev/sda`, etc.

**Recommended Fix:** Accept pattern-matching limitations. Focus on workspace confinement and timeout rather than exhaustive pattern lists.

---

### 8. Workspace Directory Not Validated

**File affected:** `src/utils/config_mod.rs:77`

**Status:** CONFIRMED - Low priority for local use

```rust
fn default_workspace() -> PathBuf { std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")) }
```

If current directory doesn't exist, application proceeds with invalid state. For local use, this is acceptable - user will notice.

---

### 9. API Keys in Memory Are Not Protected

**File affected:** `src/utils/config_mod.rs:21-27`

**Status:** CONFIRMED - Low priority (standard limitation for local apps)

**Local-Use Note:** API keys stored as `Option<String>`. Log redaction is implemented. For local use on trusted machine, this is standard and acceptable.

---

### 10. No Audit Trail for Autonomous Actions

**File affected:** `src/agents/base.rs:445-471`, `src/utils/logger_mod.rs:56-77`

**Status:** CONFIRMED - Medium priority for accountability

When `autonomous_mode=true`, tools execute without user approval. There's no structured audit event for tool executions - only embedded in agent_message content.

**Local-Use Note:** Less critical for personal use, more important if delegating to automated agents.

---

## Verified Memory & Performance Issues

### 11. Rough Token Estimation

**File affected:** `src/core/memory.rs:33-39`

**Status:** CONFIRMED - Low priority (only used for warnings)

The estimation uses 4-char/token heuristic. It's only used for logging warnings, not actual circuit-breaking. Acceptable for local use.

---

### 12. No Actual Context Compaction

**File affected:** `src/core/orchestrator_mod.rs:260-268`

**Status:** MISLEADING - Already handled by sliding window

The `shared_memory` sliding window IS the compaction mechanism. When messages exceed `max_messages` (50), oldest are dropped. This is working as designed.

---

### 13. Session Store Grows Unbounded

**File affected:** `src/api/sessions_mod.rs:56-60`

**Status:** CONFIRMED - Moderate priority

Sessions accumulate in memory. `delete_session()` exists but no automatic cleanup. Large histories loaded on startup.

**For Local Use:** Manageable unless user creates thousands of sessions. Consider adding archival.

---

### 14. MCP Server Spawn Per Tool Call

**File affected:** `src/core/tools/mcp_client.rs:201-249`

**Status:** CONFIRMED - **HIGH PRIORITY - Quick Win**

Every MCP tool call spawns a new process, initializes, calls tool, shuts down:
- Process spawn: ~50-200ms
- MCP initialization: ~100-300ms
- Shutdown: ~50-100ms
- **Total overhead: 200-600ms per call**

**Fix:** Implement connection pooling or client caching. This is the biggest performance gain available.

---

### 15. Spawn Subagent Tool is Incomplete

**File affected:** `src/core/tools/spawn_subagent.rs`

**Status:** CONFIRMED - Feature not implemented by design

The `spawn_tx` channel is never connected. Tool falls back to simulated responses.

**To Complete:**
1. Build IPC spawner service
2. Connect to orchestrator
3. Integrate with WorktreeManager for isolation

---

### 16. Synchronous Logging in Hot Path

**File affected:** `src/utils/logger_mod.rs:155-159`

**Status:** CONFIRMED - Moderate-High priority

`log()` does disk I/O with `sync_all()` on every call. Under high load, becomes bottleneck.

**Quick Fix:** Remove `sync_all()` call at line 149. Use buffered writes.

---

## Configuration Issues

### 17. Confusing Binary/Provider Naming

**File affected:** `src/utils/config_mod.rs:44-53`, `src/core/providers_mod.rs:1690-1699`

Default binaries like `gemini-cli`, `qwen-cli` are used to detect provider type. Config value "minimax" could mean CLI binary or API provider.

---

### 18. No Validation of Provider Configuration

The application starts even if no API keys and no CLI binaries. Only fails when agents try to make requests.

---

### 19. Autonomous Mode Flag Duplication

**File affected:** `src/core/providers_mod.rs:73`, `src/agents/base.rs:233`

Both `cli_provider.is_autonomous` and `agent.is_autonomous` control similar behavior. Set independently, can get out of sync.

---

## Code Quality Issues

### 20. Overuse of `#[allow(non_camel_case_types)]`

**Confirmed:** 39 instances across codebase. Types like `agent_response`, `tool_call_result`, `shared_memory` should be `AgentResponse`, `ToolCallResult`, `SharedMemory`.

**Fix Effort:** Large - requires mass renaming across 10+ files with serialization risk.

---

### 21. Inconsistent Error Handling

**Confirmed:** Three error patterns used:
- `Result<T, anyhow::Error>` (idiomatic)
- `Result<T, String>` (used in tools)
- `anyhow::bail!` (scattered)

**Fix Effort:** Medium - standardize on `anyhow::Error` throughout.

---

### 22. Magic Numbers Not in Constants

**File:** `src/core/orchestrator_mod.rs:246-250`

Inline calculation `messages.len() + pinned_messages.len()` duplicated.

**Fix Effort:** Small - extract to method.

---

### 23. No Integration Tests

**Confirmed:** Only unit tests exist. No integration tests for:
- Full council flow
- API endpoints
- Session persistence
- MCP integration

**Fix Effort:** Medium - needs test infrastructure setup.

---

### 24. Error Messages Silently Dropped

**File:** `src/api/sessions_mod.rs:86-94`

Uses `eprintln!` instead of structured logging (`tracing::warn!`).

**Fix Effort:** Small - one-line fix per location.

---

## Specific Code Problems

### 25. Race Condition in Worktree Cleanup

**File:** `src/core/worktree_mod.rs:117-140`

Exponential backoff retry still races with git's internal cleanup.

**Fix Effort:** Small - could use `git worktree remove --force`.

---

### 26. Streaming SSE Character-by-Character

**File:** `src/core/providers_mod.rs:364-367`

Non-streaming CLI responses chunked character-by-character instead of batching.

**Fix Effort:** Small - batch output or send as single message.

---

### 27. Tool Call Regex is Brittle

**File:** `src/agents/base.rs:49-135`

Complex nested regex and state machine for parsing LLM pseudo-JSON output. Misses valid formats.

**Fix Effort:** Medium - would need parser redesign.

---

### 28. `wildcard_matches` Doesn't Handle Escaped Characters

**File:** `src/core/approval.rs:144-186`

No escape handling for `\*` in patterns. Could cause false positives.

**Fix Effort:** Small - add escape character support.

---

## Missing Features

### 29. No Request Correlation IDs

No way to trace a request through the system.

---

### 30. No API Documentation

No OpenAPI/Swagger spec.

---

### 31. No Metrics/Observability

No Prometheus metrics or structured monitoring for production use.

---

### 32. No graceful degradation

If judge fails, entire session fails. No fallback.

---

### 33. Linux not supported for workspace browser

**File:** `src/api/routes.rs:41-79`

Only macOS and Windows folder picker dialogs.

---

## NEW: Web-Based Environment Configuration

### 34. Environment Configuration via Web UI

**Priority:** High (User-Requested Feature)

**Description:**
Add the ability to configure API keys and application settings via the web UI instead of environment variables or config files.

**Scope:**
- API key management (MiniMax, OpenAI, Anthropic, OpenRouter)
- Provider selection (API vs CLI binaries)
- Default model and provider preferences
- Autonomous mode toggle
- Session preferences (max rounds, timeout values)
- Workspace directory selection

**Implementation Approach:**

1. **Add Config API Endpoints** (in `src/api/routes.rs`):
   ```
   GET  /api/config          - View current configuration (masked keys)
   PUT  /api/config          - Update configuration
   POST /api/config/validate - Validate configuration before saving
   ```

2. **Create Configuration State** (in `src/utils/config_mod.rs`):
   ```rust
   pub struct app_config {
       pub debug: bool,
       pub minimax_api_key: Option<String>,
       pub openai_api_key: Option<String>,
       pub default_provider: String,
       pub autonomous_mode: bool,
       // ... existing fields
   }

   impl app_config {
       pub fn save_to_file(&self, path: &Path) -> Result<()>
       pub fn load_from_file(path: &Path) -> Result<Self>
       pub fn mask(&self) -> Self  // Replace keys with "***"
   }
   ```

3. **Add Web UI Components** (in `src/api/`):
   - Settings panel with form inputs
   - API key input fields (password type, show/hide toggle)
   - Provider dropdown selectors
   - Save/Apply/Cancel buttons
   - Validation feedback

4. **Persist Configuration:**
   - Save to `.enclave_config.json` in workspace
   - Load on startup
   - Keys stored as plain text (local use assumption)
   - Option to export/import config

5. **Security Considerations (Local-Use Adjusted):**
   - No encryption needed for local use
   - Warn users if config file has weak permissions
   - Allow clearing keys from UI

**UI Layout Suggestion:**
```
┌─────────────────────────────────────────┐
│  Settings                               │
├─────────────────────────────────────────┤
│  API Keys                               │
│  ├─ MiniMax API Key    [••••••••••••]   │
│  ├─ OpenAI API Key     [••••••••••••]   │
│  ├─ Anthropic API Key  [••••••••••••]   │
│  └─ OpenRouter API Key [••••••••••••]   │
│                                         │
│  Default Provider  [v]                  │
│  Default Model     [gpt-4o]             │
│                                         │
│  Autonomous Mode   [toggle]             │
│  Max Rounds        [7]                  │
│                                         │
│         [Save]  [Cancel]               │
└─────────────────────────────────────────┘
```

---

## Recommendations Summary

### High Priority (Quick Wins)

1. **Fix rate limiting IP** - Use `axum::extract::ConnectInfo` (when exposing API)
2. **MCP connection pooling** - Biggest performance gain (200-600ms savings per call)
3. **Remove `sync_all()` from logging** - One-line change, significant impact
4. **Fix character-by-char SSE** - Batch CLI output
5. **Web-based config UI** - User-requested feature for convenience

### Medium Priority

6. Fix sync filesystem in `is_git_repo()`
7. Add structured `tool_executed` audit events
8. Standardize error handling on `anyhow::Error`
9. Add integration tests
10. Implement sub-agent spawner runtime

### Lower Priority

11. Rename types to PascalCase (large effort, low return)
12. Add OpenAPI documentation
13. Add Linux folder picker support
14. Add graceful degradation for judge failures
15. Create session archival/cleanup mechanism

---

## Architecture Notes

The system has good separation of concerns with:
- Providers handling different LLM interfaces
- Tool system abstracted from agent logic
- Session management decoupled from orchestration

However, the tight coupling between `base_agent` and specific tool execution makes it hard to:
- Test agents in isolation
- Swap tool implementations
- Add new agent types without modifying core logic

The worktree isolation feature is innovative but adds complexity that could be optional for non-git workflows.

---

## Recommended Skills to Integrate

### 1. UI/UX Design - getdesign.md

**Source:** https://getdesign.md/

A collection of DESIGN.md files for AI coding agents containing design system inspirations from popular websites (Vercel, Stripe, Figma, Notion, Linear, Airbnb, Spotify, Apple, Uber, etc.).

**Integration Benefits:**
- Enables agents to generate UI that matches established design systems
- Provides 68+ pre-built design specifications for popular platforms
- Helps maintain consistent visual identity across generated code

**How to Use:**
Add a `DESIGN.md` file to the project that agents can reference when building UI components. The collection includes:
- Color palettes with hex codes
- Typography specifications (Google Fonts)
- Spacing and layout conventions
- Component styling patterns

### 2. UI/UX Pro Max Skill

**Source:** https://github.com/MattYudha/ui-ux-pro-max-skill (forked from nextlevelbuilder/ui-ux-pro-max-skill)

An AI skill providing professional UI/UX design intelligence with:
- **67 UI Styles** - Glassmorphism, Claymorphism, Minimalism, Brutalism, Neumorphism, Bento Grid, Dark Mode, AI-Native UI, and more
- **161 Industry-Specific Reasoning Rules** - Tech/SaaS, Finance, Healthcare, E-commerce, Services, Creative, Lifestyle, Emerging Tech
- **57 Font Pairings** - Curated typography combinations
- **25 Chart Types** - Dashboard recommendations
- **99 UX Guidelines** - Best practices and anti-patterns
- **Design System Generator** - AI-powered reasoning engine that analyzes requirements and generates complete, tailored design systems

**Integration Benefits:**
- Automatic design system generation based on project requirements
- Industry-specific UI patterns and anti-patterns to avoid
- Pre-delivery checklist validation (accessibility, responsiveness, hover states)
- Multi-platform support (React, Next.js, Vue, Svelte, SwiftUI, React Native, Flutter, HTML+Tailwind)

**CLI Installation:**
```bash
npm install -g uipro-cli
uipro init --ai claude
```

### 3. Spec Kit - GitHub's Spec-Driven Development Toolkit

**Source:** https://github.com/github/spec-kit

An open source toolkit for **Spec-Driven Development** - specifications become executable, directly generating working implementations.

**Core Philosophy:**
Instead of coding first and treating specs as disposable scaffolding, Spec-Driven Development makes specifications the source of truth that directly generates implementations.

**Key Features:**
- **Constitution** - Project governing principles and development guidelines
- **Specify** - Define what to build (requirements and user stories focus on what and why, not tech stack)
- **Plan** - Technical implementation plans with architecture choices
- **Tasks** - Actionable task lists generated from implementation plans
- **Implement** - Execute all tasks according to the plan

**Integration Benefits:**
- Structured workflow that complements Enclave's council debate process
- Agent-agnostic - works with 30+ AI coding agents (Claude Code, Cursor, Copilot, etc.)
- Extensions ecosystem (Jira integration, Azure DevOps, Confluence, security review, QA testing)
- Presets for customizing workflow terminology and templates
- Brownfield support for extending existing codebases

**Key Commands:**
```
/speckit.constitution  # Create project governing principles
/speckit.specify      # Define what to build
/speckit.plan         # Create technical implementation plan
/speckit.tasks        # Generate actionable task lists
/speckit.implement    # Execute all tasks
```

**CLI Installation:**
```bash
uv tool install specify-cli --from git+https://github.com/github/spec-kit.git
```

### Recommended Integration Approach

1. **For UI Development:** Integrate UI/UX Pro Max skill to provide design intelligence when agents need to build interfaces. The design system generator can be invoked to create project-specific design specifications.

2. **For Complex Features:** Use Spec Kit's spec-driven workflow as an outer loop that Enclave executes within. The constitution and specify phases can set context before Enclave's council debate begins.

3. **For Design Consistency:** Add a `DESIGN.md` file (from getdesign.md collection) to the project workspace so agents reference the same visual standards when generating UI.

4. **Considerations:**
   - Spec Kit and UI/UX Pro Max can work together - use spec-kit for planning and UI/UX Pro Max for visual design generation
   - Both tools support multiple AI platforms (Claude, Cursor, Copilot, etc.)
   - The design system generator in UI/UX Pro Max uses industry-specific reasoning that complements spec-kit's product-focused specifications