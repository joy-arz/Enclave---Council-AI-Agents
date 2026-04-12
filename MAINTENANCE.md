# Maintenance & Technical Debt

This document tracks technical debt, known issues, and maintenance tasks for the Enclave project.

---

## Recent Improvements (March 2026)

### ✅ Fixed Issues

1. **Created `.geminiignore` file** - Eliminates CLI warnings about missing ignore file
2. **Enhanced Context Manager** (`src/core/memory.rs`) - Implemented proper context management with auto-summarize:
   - Tracks token usage accurately (input_tokens, output_tokens per message)
   - Configurable `context_window_target` (default 32,000 tokens)
   - `auto_summarize_threshold` (default 75%)
   - AI-powered summarization when context reaches threshold
   - Preserves system prompt and pinned messages
   - Replaces old messages with summary inserted as special message
   - Maintains key topics, decisions, and critical context in summary
3. **Added ContextConfig** to configuration system for tuning context behavior
4. **Context Management Integration** in orchestrator:
   - Emits `ContextWarning` event at 80% of target
   - Emits `ContextCompaction` events during summarization
   - Automatic fallback pruning if summary generation fails

2. **Added cleanup script** - `scripts/cleanup_worktrees.sh` removes old session directories automatically
3. **Improved error handling** - Replaced critical `unwrap()`/`expect()` calls in provider initialization:
   - `openai_provider::new()` now returns `Result`
   - `minimax_provider::new()` now returns `Result` with proper header parsing errors
   - `openrouter_provider::new()` now returns `Result`
   - Factory function gracefully falls back to CLI providers on API provider failures
4. **Regex Compilation at Runtime (parser.rs)** - Now uses `once_cell::sync::Lazy<Regex>` for static regex patterns
5. **Unwrap in Critical Path (main.rs)** - Signal handler now handles errors gracefully with `match` instead of `expect()`

---

## ⚠️ Remaining Technical Debt

### High Priority

#### 1. Context Manager Configuration
**Location:** `src/utils/config_mod.rs`

The new `ContextConfig` struct allows tuning:
- `context_window_target`: Target context size (default 32,000)
- `max_retained_messages`: Sliding window size (default 50)
- `auto_summarize_threshold`: When to trigger summarize (default 75%)
- `enable_auto_summarize`: Toggle auto-summarize (default true)
- `max_message_chars_for_summary`: Truncation limit (default 10,000)

---

#### 2. Missing Error Propagation (routes.rs)
**Location:** `src/api/routes.rs` (multiple locations)

Several API endpoints use `.unwrap()` on operations that can fail:
- Session store operations
- Workspace path handling
- Provider configuration

**Impact:** API may return 500 errors instead of proper error responses.

**Fix:** Return `Result<Json<T>, ApiError>` with proper error types.

---

### Medium Priority

#### 5. Session Directory Accumulation
**Location:** `.enclave_worktrees/`

**Issue:** Session directories accumulate over time (currently 7 sessions taking ~2.7MB).

**Status:** ✅ Mitigation added - cleanup script created. Consider adding automatic cleanup on server startup.

**Recommendation:** Add to CI/CD or run weekly via cron:
```bash
# Add to crontab: 0 3 * * 0 /path/to/scripts/cleanup_worktrees.sh 7
```

---

#### 6. Regex Tool Call Parsing Fragility
**Location:** `src/core/tools/parser.rs`

**Issue:** Tool call parsing relies on regex patterns that may break with complex JSON or nested structures.

**Impact:** Agents may fail to parse tool calls correctly, especially with complex arguments.

**Recommendation:** Consider using a proper JSON parser with span detection instead of regex.

---

#### 7. Provider API Key Validation
**Location:** `src/core/providers_mod.rs`

**Issue:** API keys are not validated before use. Invalid keys fail at runtime during first API call.

**Recommendation:** Add startup validation for API keys with clear error messages.

---

### Low Priority

#### 8. Hardcoded Timeouts
**Location:** Multiple providers (120s timeout)

**Issue:** Timeout values are hardcoded in provider constructors.

**Recommendation:** Make timeouts configurable via environment variables.

---

#### 9. Missing Integration Tests
**Location:** No `tests/` directory

**Issue:** No integration tests for API endpoints or provider implementations.

**Recommendation:** Add basic integration tests for:
- Provider factory creation
- Tool call parsing
- Session store operations

---

## Architecture Concerns

### Long-term Maintainability

1. **Agent Coupling:** Agents are tightly coupled to the orchestrator. Consider trait-based decoupling for easier testing.

2. **Memory Management:** ✅ Enhanced with auto-summarize capability. The context manager (`src/core/memory.rs`) now supports:
   - Configurable sliding window sizes
   - AI-powered summarization at threshold
   - Token-based compaction triggers
   - Fallback pruning when AI summarization fails

3. **Worktree Manager:** The git worktree manager in `src/core/worktree_mod.rs` should handle edge cases better:
   - Failed git operations
   - Concurrent worktree access
   - Disk space monitoring

4. **Error Reporting:** Centralize error handling with a custom error type instead of `anyhow::Error` throughout. This enables better API error responses.

---

## Cleanup & Hygiene

### Files to Monitor
- `.enclave_history.json` - Can grow large over time
- `.enclave_state.md` - Should be cleaned when starting fresh projects
- `last_session_log.jsonl` - Auto-generated, consider rotation

### Recommended Git Ignore Updates
Already added to `.gitignore` and `.geminiignore`:
- `.enclave_worktrees/`
- `*.jsonl`
- `.qwen/` (debug logs)

---

## Performance Notes

- **Cold Start:** First agent response takes 2-3x longer due to regex compilation
- **Memory Usage:** Sliding window keeps ~20 messages; consider reducing for long sessions
- **Concurrent Agents:** Parallel agent execution is good, but consider rate limiting for API providers

---

## Contributing

When adding new features:
1. Avoid `unwrap()` in production code paths
2. Add tracing logs for debugging
3. Consider error recovery, not just error propagation
4. Update this document if introducing known limitations

---

**Last Updated:** March 28, 2026
