# Contributing to Enclave

Thank you for your interest in contributing! Here's how you can help.

## Ways to Contribute

- **Report bugs** - Open an issue with steps to reproduce and expected behavior
- **Suggest features** - Open an issue describing the feature and its use case
- **Fix bugs** - Submit a PR for any issue you're comfortable addressing
- **Improve documentation** - Help make the docs clearer and more comprehensive

## Development Setup

### Prerequisites

- Rust (latest stable via [rustup](https://rustup.rs/))
- A supported CLI AI agent: qwen, gemini, codex, claude, or opencode
- Node.js (optional, for frontend development)

### Getting Started

1. **Fork the repository** on GitHub

2. **Clone your fork:**
   ```bash
   git clone https://github.com/YOUR_USERNAME/Enclave---Council-AI-Agents.git
   cd Enclave---Council-AI-Agents
   ```

3. **Set up environment:**
   ```bash
   cp .env.example .env
   # Edit .env with your configuration
   ```

4. **Run the project:**
   ```bash
   cargo run -- --server
   ```

5. **Check for issues during development:**
   ```bash
   cargo check
   cargo build
   ```

## Code Style

- Run `cargo fmt` before committing to ensure consistent formatting
- Run `cargo clippy` to catch common mistakes
- Keep functions focused and reasonably sized
- Add comments for non-obvious logic, not for obvious operations

## Pull Request Process

1. **Create a feature branch** from `master`:
   ```bash
   git checkout -b feature/your-feature-name
   ```

2. **Make your changes** and commit with clear messages:
   ```bash
   git commit -m "Add brief description of changes"
   ```

3. **Push to your fork:**
   ```bash
   git push origin feature/your-feature-name
   ```

4. **Open a Pull Request** against `master` with a clear description of your changes

5. **Link any relevant issues** in your PR description

## Project Structure

```
src/
├── main.rs           # Entry point, server & CLI modes
├── cli.rs            # CLI argument parsing
├── api/              # HTTP API endpoints
├── core/             # Orchestration and providers
│   ├── orchestrator_mod.rs  # Main council logic
│   └── providers_mod.rs     # CLI provider abstraction
├── agents/           # Agent definitions
│   ├── base.rs       # Base agent structure
│   ├── roles.rs      # Role definitions
│   └── judge.rs      # Lead engineer agent
└── ui/               # Web dashboard
    ├── index.html    # Dashboard UI
    └── script.js     # Frontend logic
```

## Areas for Contribution

- Additional AI provider support
- Enhanced auto-rounds intelligence
- Better error handling and recovery
- UI/UX improvements
- Testing and documentation

## Questions?

Reach out via:
- **Contact Form:** [https://www.joyarz.space/](https://www.joyarz.space/)
