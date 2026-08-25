<div align="center">

<img src="./logo.jpg" alt="KeyCompute logo" width="160" style="border-radius: 20px;" />

# KeyCompute

<p align="center">
  <a href="./README.zh-CN.md">简体中文</a> |
  <a href="./README.md">English</a> |
  <a href="./README.zh-TW.md">繁體中文</a> |
  <a href="./README.es.md">Español</a> |
  <a href="./README.ar.md">العربية</a>
</p>

**Next-generation high-performance AI token compute service platform**

<p align="center">
  <a href="https://github.com/keycompute/keycompute/stargazers"><img src="https://img.shields.io/github/stars/keycompute/keycompute?style=social" alt="GitHub Stars" /></a>
  <a href="https://github.com/keycompute/keycompute/issues"><img src="https://img.shields.io/github/issues/keycompute/keycompute" alt="GitHub Issues" /></a>
  <a href="./LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="MIT License" /></a>
  <a href="./CONTRIBUTING.md"><img src="https://img.shields.io/badge/PRs-welcome-brightgreen" alt="PRs Welcome" /></a>
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/Rust-1.92%2B-orange?logo=rust" alt="Rust Version" /></a>
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#configuration">Configuration</a> •
  <a href="#project-structure">Project Structure</a> •
  <a href="#api">API</a> •
  <a href="#development-guide">Development</a>
</p>

</div>

---

## Overview

KeyCompute is a **high-performance**, **extensible**, and **out-of-the-box** AI token compute service platform, providing enterprise-grade capabilities including unified LLM access, smart routing, metering and billing, compute node leasing, multi-level distribution, and observability.

> **Pure Rust Full Stack**: Backend (Axum) + Frontend (Dioxus WASM) + CLI client, sharing types and logic, ultimate performance and security.

> **Note**: This project is for personal learning only. You must use it in compliance with OpenAI [Terms of Use](https://openai.com/policies) and applicable laws and regulations. Do not use it for illegal purposes. In accordance with the Interim Measures for the Administration of Generative Artificial Intelligence Services, do not provide any unregistered generative AI services to the public in China.

---

## Features

### Compute Node Leasing
Compute nodes connect via **pull-based polling** without requiring a **public IP**. They run hosted models on local hardware and earn rewards based on contributions.

- **One-click connection**: Run the standalone CLI binary to auto-register → heartbeat → poll tasks → local execution → submit results
- **Node routing**: Use `node:<model_name>` to explicitly route requests to the node pool
- **Automatic failover**: Failed nodes are excluded from scheduling, tasks are automatically requeued
- **Session persistence**: Local sessions prevent duplicate registration; graceful shutdown ensures task integrity
- **Tip mechanism**: Node owners can earn and withdraw tips

### Unified Multi-model Gateway
Seamlessly switch between all major models with the standard **OpenAI API** — just one line of code:

| Provider | Model Families | Implementation |
|:---|:---|:---:|
| 🟢 OpenAI | GPT-4o / GPT-4 / GPT-3.5 etc. | ✅ |
| 🟣 Anthropic | Claude 3.5 Sonnet / Opus / Haiku etc. | ✅ |
| 🔵 Google | Gemini 1.5 / 2.0 Flash / Pro etc. | ✅ |
| 🔴 DeepSeek | DeepSeek-V3 / R1 / Chat etc. | ✅ |
| 🟤 Ollama | Local models (Llama / Qwen / GLM / MiniMax etc.) | ✅ |
| 🟡 vLLM | Self-hosted models | ✅ |

> GLM (Zhipu) and MiniMax can be deployed locally via the Ollama adapter, not as standalone Provider implementations.

### Smart Routing Engine
**Two-layer routing architecture** with multi-factor weighted scoring for optimal selection:

```text
score = 0.30 × Cost Factor + 0.25 × Latency Factor + 0.25 × Success Rate + 0.20 × Health Status
```

- **Model-level routing** → **Account pool routing**: Automatically distributes across providers and accounts
- **Fallback chain**: Automatically switches to backup targets when primary target fails
- **Exponential backoff retry**: Up to 3 retries, initial 100ms, max 10s
- **Request-level proxy**: Supports provider-level / account-level / wildcard HTTP proxies

### Billing & Payment System

- **Post-stream settlement**: Precise calculation after request completion, no pre-deduction, no impact on results
- **Three-tier pricing**: Tenant-specific pricing → Database default → Hardcoded fallback (LRU cache)
- **Precise usage**: Priority to provider-precise usage, falls back to tiktoken estimation
- **Online top-up**: Alipay/WeChat Pay + balance management
- **Usage analytics**: Detailed token consumption breakdowns with visualization

### Referral Distribution System

- **Referral commissions**: Default 3% for first level + 2% for second level, auto-calculated
- **Invite links**: Generate exclusive invite links with one click
- **Flexible configuration**: Admins configure distribution ratios via API
- **Revenue analytics**: View referral earnings and referral list in real time

### Authentication & Permissions

- **Dual authentication**: JWT (user sessions) + API Key (`sk-...`, API access)
- **Permission separation**: API Key with admin role cannot access management interface
- **Complete user management**: Registration → Email verification → Login → Password reset → Role management
- **Group-based rate limiting**: User-level / tenant-level / API Key-level throttling (in-memory / Redis dual backend)

### Observability

- **Prometheus metrics**: Request volume, latency, error rate, provider health
- **Distributed tracing**: Provider Span / Request Span / Stream Span
- **Structured logging**: JSON format, development/production tiered output
- **Host monitoring**: CPU / Memory / Disk / Network real-time metrics
- **Health check**: `/health` endpoint for one-click service status monitoring

### Cross-platform Frontend

- **Web admin dashboard**: Dioxus WASM SPA
- **Desktop**: Dioxus Desktop native application
- **Mobile**: Dioxus Mobile cross-platform support
- **Route-level permission control**: Admin role verification, secure and manageable

---

## Architecture

```text
[Client: Web / Desktop / Mobile (Dioxus)]
                ↕ HTTP/SSE
[API Layer: keycompute-server (Axum)]
       ├── Authentication (JWT + API Key)
       ├── Rate Limiting (In-memory/Redis)
       ├── Routing (Two-layer engine)
       └── Gateway (Single upstream execution layer)
                ↕
[Provider Adapter Layer]
  ├── OpenAI / Anthropic / Google
  ├── DeepSeek
  ├── Ollama (Local models)
  └── vLLM (Self-hosted)

[Compute Node Network]
  node-token (CLI) ↔ node-gateway ↔ Redis task queue ↔ Local inference
```

---

## Quick Start

### Requirements

| Component | Version Requirement |
|:---|:---|
| Rust | ≥ 1.92 |
| Axum | ≥ 0.8.0 |
| Dioxus | ≥ 0.7.1 (frontend development) |
| PostgreSQL | ≥ 16 |
| Redis | ≥ 7 (optional, for distributed rate limiting/node queue) |
| Docker | Latest (container deployment) |

### Option 1: Docker Compose deployment (recommended)

```bash
# Clone the project
git clone https://github.com/keycompute/keycompute.git
cd keycompute

# Copy and edit environment variables
cp .env.example .env
# Before the first production startup, replace every operational credential.
# The application will refuse to start unless at least these values satisfy:
# - KC__AUTH__JWT_SECRET: non-default, non-blank, at least 32 bytes
# - KC__CRYPTO__SECRET_KEY: Base64 encoding of exactly 32 bytes
# - KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET: non-default, at least 16 bytes
#   (required here because this Compose stack enables Redis/Node Gateway)
# - KC__DEFAULT_ADMIN_PASSWORD: non-default, non-blank, at least 12 characters
#   (required only while creating the first system administrator)

# Start all services
docker compose up -d

# Check service status
docker compose ps
```

After deployment, visit `http://localhost` to get started (or use the port set by `WEB_PORT`).

Unless overridden, the bootstrap administrator email is `admin@keycompute.local`.
Its password is the value you set in `KC__DEFAULT_ADMIN_PASSWORD`; production
never accepts `change-me-admin-password` for a fresh database. After the first
`system` administrator has been created, remove this one-time password from the
environment; later restarts do not require it.

### Option 2: Local development

> ⚠️ **Security Warning**: The default values shown below (`change-me-*`) are for demonstration only.
> **Never use these in production!** Generate strong random passwords using:
> ```bash
> openssl rand -base64 32
> ```
> An ordinary `cargo run` builds the debug executable, so the startup method
> itself selects development mode. It reads only `config.toml`, may use public
> local credentials, and may listen on `0.0.0.0`. `.env.example`, `KC__*`, and
> `APP_BASE_URL` cannot override that file. Never reuse it in production.

```bash
# Create the cargo-run configuration
cp config.example.toml config.toml

# Start PostgreSQL and Redis with host-local ports matching config.toml
docker compose --env-file .env.example -f docker-compose.yml -f docker-compose.dev.yml up -d postgres redis

# Install dioxus-cli
curl -sSL http://dioxus.dev/install.sh | sh

# Start the backend
cargo run -p keycompute-server

# Start the frontend development server (in another terminal)
dx serve --package web --platform web --hot-reload true --addr 0.0.0.0
```

`cargo run -p keycompute-server --release` and the Docker image select
production mode. The release executable reads environment variables and ignores
`config.toml`; no configuration parameter or environment variable can change
the runtime mode.

---

## Project Structure

```text
keycompute/
├── crates/                          # Backend core modules (Rust)
│   ├── keycompute-server/            # Axum HTTP service (integrates all modules)
│   ├── keycompute-types/             # Shared types and macros
│   ├── keycompute-db/                # Database ORM and migrations
│   ├── keycompute-auth/              # Auth & authorization (JWT + API Key + Password)
│   ├── keycompute-cache/             # Cache abstraction and Redis support
│   ├── keycompute-ratelimit/         # Rate limiting engine (In-memory/Redis dual backend)
│   ├── keycompute-pricing/           # Pricing engine (Three-tier + LRU cache)
│   ├── keycompute-routing/           # Two-layer smart routing engine
│   ├── keycompute-runtime/           # Runtime (AES-256-GCM encryption + storage abstraction)
│   ├── keycompute-billing/           # Billing & settlement (Post-stream precise settlement)
│   ├── keycompute-distribution/      # Referral distribution system
│   ├── keycompute-observability/     # Observability three pillars
│   ├── keycompute-config/            # Separate debug-TOML and release-environment loaders
│   ├── keycompute-emailserver/       # SMTP email service
│   ├── keycompute-payment/           # Payment integration
│   │   ├── keycompute-alipay/        # Alipay payment
│   │   └── keycompute-wechatpay/     # WeChat Pay
│   ├── llm-gateway/                  # LLM execution gateway (single upstream layer)
│   ├── llm-protocol/                 # Protocol translation and provider definitions
│   │   ├── openai/                   # OpenAI-compatible protocol
│   │   ├── anthropic/                # Anthropic-compatible protocol
│   │   └── provider/                 # Shared provider protocol types
│   ├── node-gateway/                 # Node gateway (registration/heartbeat/task management)
│   └── integration-tests/           # End-to-end integration tests
├── packages/                         # Frontend (Dioxus 0.7)
│   ├── web/                          # Web admin dashboard
│   ├── ui/                           # Shared UI component library
│   ├── desktop/                      # Desktop native application
│   ├── mobile/                       # Mobile cross-platform application
│   └── client-api/                   # API client wrapper
├── nginx/                            # Nginx reverse proxy configuration
├── Dockerfile.server                 # Backend container image
├── Dockerfile.web                    # Frontend container image
├── docker-compose.yml                # Production container orchestration
├── docker-compose.dev.yml            # Host-local dependency port overrides
└── docker-compose.replicas.yml       # Production read-replica orchestration
```

---

## Configuration

### Environment variables

This table applies to release/Docker production startup. A debug `cargo run`
uses `config.toml` instead.

| Variable | Description | Required |
|:---|:---|:---:|
| `KC__DATABASE__URL` | PostgreSQL connection string | ✅ |
| `KC__AUTH__JWT_SECRET` | Production: non-default/non-blank JWT signing secret, at least 32 bytes | ✅ |
| `KC__CRYPTO__SECRET_KEY` | Production: Base64 encoding of exactly 32 bytes; cannot be changed after Provider API keys are written | ✅ |
| `KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET` | Production with Redis: non-default HMAC signing secret of at least 16 bytes; issues one-time node registration tokens | Conditional |
| `KC__REDIS__URL` | Redis connection string (optional; without it: rate limiter falls back to in-memory, cache no-ops, node gateway unavailable) | ⚪ |
| `KC__EMAIL__SMTP_HOST` | SMTP host (optional) | ⚪ |
| `KC__EMAIL__SMTP_PORT` | SMTP port (optional) | ⚪ |
| `KC__EMAIL__SMTP_USERNAME` | SMTP username (optional) | ⚪ |
| `KC__EMAIL__SMTP_PASSWORD` | SMTP password (optional) | ⚪ |
| `KC__EMAIL__FROM_ADDRESS` | Sender email address (optional) | ⚪ |
| `KC__EMAIL__FROM_NAME` | Sender display name (optional) | ⚪ |
| `KC__EMAIL__REQUIREMENT_RECIPIENT` | Requirement collection recipient email (optional; required to receive homepage submissions) | ⚪ |
| `APP_BASE_URL` | Current deployment's public frontend URL; required when SMTP is enabled and before enabling public invite links | Conditional |
| `KC__DEFAULT_ADMIN_EMAIL` | Default administrator email (optional) | ⚪ |
| `KC__DEFAULT_ADMIN_PASSWORD` | One-time bootstrap password: required only when production creates the first `system` administrator; non-default/non-blank and at least 12 characters | Conditional |

---

## API

### OpenAI-compatible API

```bash
# Chat Completions (streaming + non-streaming)
curl http://localhost:3000/v1/chat/completions \
  -H "Authorization: Bearer sk-xxx" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "deepseek-chat",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": true
  }'

# List available models (defaults to the openai ingress protocol)
# Pass ?protocol=anthropic to list anthropic-protocol models instead
curl http://localhost:3000/v1/models \
  -H "Authorization: Bearer sk-xxx"
```

### Admin API Overview

| Category | Endpoint | Description |
|:---|:---|:---|
| Auth | `POST /api/v1/auth/register` | User registration |
| | `POST /api/v1/auth/login` | User login |
| | `POST /api/v1/auth/forgot-password` | Forgot password |
| User | `GET /api/v1/me` | Current user info |
| | `GET/POST /api/v1/keys` | API Key management |
| Billing | `GET /api/v1/usage` | Usage statistics |
| | `GET /api/v1/billing/records` | Billing records |
| Payment | `POST /api/v1/payments/orders` | Create payment order |
| | `GET /api/v1/payments/balance` | Balance inquiry |
| Distribution | `GET /api/v1/me/distribution/earnings` | Distribution earnings |
| Node | `GET /api/v1/me/node-gateway/token` | Node token |
| | `GET /api/v1/me/tips` | Tip summary |
| Admin | `GET/POST /api/v1/accounts` | Upstream account management |
| | `GET/POST /api/v1/settings` | System settings |
| | `GET/POST /api/v1/pricing` | Pricing management |
| | `GET /api/v1/admin/monitoring/overview` | Monitoring overview |

> For complete API documentation, refer to the route definitions in the project source code.

---

## Development Guide

```bash
# Build (exclude desktop and mobile)
cargo build --workspace --exclude desktop --exclude mobile --verbose

# Run unit tests
cargo test --lib --workspace --exclude desktop --exclude mobile --verbose

# Run integration tests
cargo test --package integration-tests --tests --verbose

# Run frontend API client tests
cargo test --package client-api --tests --verbose

# Clippy code checks
cargo clippy --workspace --exclude desktop --exclude mobile --all-targets --all-features --future-incompat-report -- -D warnings

# Code formatting check
cargo fmt --all --check

# Release build
cargo build -p keycompute-server --release
```

---

## Contributing

We welcome contributions of all kinds. Please read [CONTRIBUTING.md](CONTRIBUTING.md) to learn how to get involved.

- 🐛 [Report bugs](https://github.com/keycompute/keycompute/issues/new?template=bug_report.yml)
- 💡 [Feature requests](https://github.com/keycompute/keycompute/issues/new?template=feature_request.yml)
- 🔧 [Submit code](CONTRIBUTING.md)

---

## License

This project is open sourced under the [MIT](LICENSE) License.

---

<div align="center">

### 💖 Thanks for using KeyCompute

If this project helps you, feel free to give it a ⭐️ star.

**[Quick Start](#quick-start)** • **[Report Issues](https://github.com/keycompute/keycompute/issues)** • **[Latest Releases](https://github.com/keycompute/keycompute/releases)**

</div>
