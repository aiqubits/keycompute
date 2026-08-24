# Contributing Guide

Thank you for your interest in KeyCompute. Contributions are welcome across code, documentation, bug reports, feature requests, and project feedback.

This guide is written for the current repository layout and workflow. If a command here conflicts with the codebase, follow the codebase and open a documentation fix.

## Ways to Contribute

- Fix bugs or improve existing behavior
- Add tests for uncovered scenarios
- Improve documentation and examples
- Add or refine provider integrations
- Report bugs, edge cases, or usability issues
- Propose new features or architectural improvements

## Before You Start

- Read [README.md](README.md) for the product overview and local development commands.
- Search existing issues and pull requests before starting duplicate work.
- Keep changes focused. Small, reviewable pull requests are much easier to merge.

## Development Environment

### Stack

- Backend: Rust, Axum, Tokio
- Frontend: Dioxus 0.7
- Database: PostgreSQL 16+
- Cache / rate limiting: Redis 7+

### Prerequisites

- Rust stable toolchain
- Docker and Docker Compose for local services
- `dioxus-cli` for frontend development

Install the Dioxus CLI if you plan to work on the web frontend:

```bash
curl -sSL http://dioxus.dev/install.sh | sh
```

## Local Setup

### Option 1: Run the full Docker Compose stack

This is the easiest way to get the project running end to end.

```bash
git clone https://github.com/keycompute/keycompute.git
cd keycompute

cp .env.example .env
# Edit .env and replace all placeholder secrets before real deployments

docker compose up -d
```

### Option 2: Run backend and frontend locally

Use this setup when you want a faster edit / run loop.

1. Copy the local config template:

```bash
cp config.example.toml config.toml
```

2. Start the local dependencies:

```bash
docker compose up -d postgres redis
```

3. Start the backend:

```bash
cargo run -p keycompute-server
```

4. Start the web frontend in another terminal:

```bash
dx serve --package web --platform web --hot-reload true --addr 0.0.0.0
```

Notes:

- The backend initializes the embedded schema on startup. Database upgrades are intentionally unsupported: recreate the database after schema changes.
- `config.toml` is intended for local development. Environment variables override values from `config.toml`.
- If you work on password reset emails or public invite links, set `APP_BASE_URL` explicitly.

## Project Structure

```text
keycompute/
├── crates/
│   ├── keycompute-server/          # Axum HTTP service entrypoint
│   ├── keycompute-db/              # Database access and new-database schema initialization
│   ├── keycompute-auth/            # Authentication and authorization
│   ├── keycompute-routing/         # Model and account routing
│   ├── keycompute-billing/         # Billing and settlement
│   ├── keycompute-distribution/    # Referral distribution
│   ├── keycompute-runtime/         # Runtime state and store backends
│   ├── keycompute-config/          # Config loading and validation
│   ├── keycompute-observability/   # Logging and metrics
│   ├── keycompute-emailserver/     # Email delivery
│   ├── llm-gateway/                # Provider execution gateway
│   └── llm-protocol/               # Protocol adapters (openai / anthropic)
├── packages/
│   ├── web/                        # Dioxus web app
│   ├── ui/                         # Shared UI components
│   ├── client-api/                 # Client API package and tests
│   ├── desktop/                    # Dioxus desktop app
│   └── mobile/                     # Dioxus mobile app
├── nginx/                          # Reverse proxy config
├── docker-compose.yml
└── .github/workflows/              # CI checks
```

## Code Quality Checks

Please run the same core checks used by CI before opening a pull request.

### Formatting

```bash
cargo fmt --all --check
```

### Linting

```bash
cargo clippy --workspace --exclude desktop --exclude mobile --all-targets --all-features --future-incompat-report -- -D warnings
```

### Tests

```bash
cargo test --lib --workspace --exclude desktop --exclude mobile --verbose
cargo test --package client-api --tests --verbose
cargo test --package integration-tests --tests --verbose
```

### Optional build check

```bash
cargo build --workspace --exclude desktop --exclude mobile --verbose
```

If your change touches `desktop` or `mobile`, run the relevant package commands in addition to the shared workspace checks.

## Contribution Expectations

### Rust and backend changes

- Follow the existing crate boundaries and dependency direction.
- Prefer small, composable changes over broad refactors.
- Add or update tests when behavior changes.
- Keep logging and error messages actionable.

### Example security configuration policy

- `.env.example`, `config.example.toml`, and the fallback values in the Compose
  files are development examples. Operators must override them for production.
- Production startup rejects placeholder JWT and node-registration secrets,
  default or weak administrator passwords, and an omitted Provider API-key
  encryption key. Keep code, templates, and tests aligned with this
  fail-closed policy.
- New example credentials must be clearly marked, remain overridable through
  documented configuration, and have tests that keep code, templates, and
  Compose fallbacks aligned.

### Frontend changes

- This repository uses Dioxus 0.7. Do not introduce older Dioxus APIs.
- Keep shared UI logic in `packages/ui` when it is platform-agnostic.
- Keep web-specific dependencies and behavior in `packages/web`.

### Database changes

- Update `crates/keycompute-db/migrations/V0001__baseline.sql`; it is the complete
  schema and the only SQL source of truth for fresh deployments.
- Update the relevant data models and query code in `crates/keycompute-db/src/models/`.
- Recreate the database after changing the baseline; incremental upgrades,
  compatibility migrations, and old-data backfills are not supported.
- Verify the server can initialize an empty database and boot cleanly.

### Adding a new provider

Most vendors (DeepSeek, Ollama, vLLM, Gemini OpenAI-compat, etc.) do NOT need
new code: create a channel account with the matching protocol (`openai` or
`anthropic`) plus the vendor's base URL and API key.

Only when integrating a genuinely new wire protocol:

1. Create a protocol crate under `crates/llm-protocol/`.
2. Implement the `ProviderAdapter` trait from `llm-protocol-provider`.
3. Add the protocol to `ProtocolType` and register it in `keycompute-server`
   (`providers.rs`) and any required routing wiring.
4. Add tests for request mapping, stream parsing, error handling, and any
   protocol-specific behavior.

## Commits

- Use clear commit messages that explain what changed and why.
- Keep one logical change per commit whenever practical.
- Prefer English commit messages for consistency across the repository.

Example:

```text
feat: add DeepSeek provider streaming support

- implement provider client
- normalize streaming chunks
- add tests and update docs
```

## Pull Requests

### Submission flow

1. Fork the repository and create a branch from `main`.
2. Make your changes and run the relevant checks.
3. Push your branch.
4. Open a pull request with a clear description.

### PR checklist

- [ ] The code is formatted with `cargo fmt`
- [ ] `cargo clippy` passes for the affected scope
- [ ] Relevant tests were added or updated
- [ ] Relevant test suites pass locally
- [ ] Documentation was updated when behavior or setup changed

### PR description tips

Include:

- What changed
- Why it changed
- How it was tested
- Any follow-up work or known limitations

Screenshots or API examples are helpful for UI and behavior changes.

## Reporting Bugs

Please include:

- A clear description of the problem
- Steps to reproduce
- Expected behavior and actual behavior
- Environment details such as OS, Rust version, and how you started the app
- Relevant logs, traces, screenshots, or error messages

## Suggesting Features

Please describe:

- The use case
- The expected behavior
- Why the feature would be valuable
- Any proposed implementation direction if you already have one

## Community and Communication

- Use GitHub Issues for bug reports and feature discussions
- Use pull requests for concrete code and documentation changes
- If you find inaccurate docs, documentation-only pull requests are welcome

## License

By contributing to this repository, you agree that your contributions will be licensed under the same [MIT License](LICENSE) that covers the project.
