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
  <a href="https://github.com/keycompute/keycompute/issues"><img src="https://img.shields.io/github/issues/aiqubits/keycompute" alt="GitHub Issues" /></a>
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

- **Web admin dashboard**: Dioxus WASM SPA, 9 management modules
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
git clone https://github.com/your-org/keycompute.git
cd keycompute

# Copy and edit environment variables
cp .env.example .env
# Edit .env and fill in real configuration values

# Start all services
docker compose up -d

# Check service status
docker compose ps
```

After deployment, visit `http://localhost:8080` to get started.

Default account: `admin@keycompute.local`, password: `change-me-admin-password`

> Change the default administrator password immediately in production.

### Option 2: Local development

> ⚠️ **Security Warning**: The default values shown below (`change-me-*`) are for demonstration only.
> **Never use these in production!** Generate strong random passwords using:
> ```bash
> openssl rand -base64 32
> ```

```bash
# Create the network
docker network create keycompute-internal

# PostgreSQL (using the password from .env)
docker run -d \
  --name keycompute-postgres \
  --network keycompute-internal \
  -e POSTGRES_DB=keycompute \
  -e POSTGRES_USER=keycompute \
  -e POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-change-me-strong-password}" \
  -p 5432:5432 \
  -v keycompute_postgres_data:/var/lib/postgresql/data \
  --restart unless-stopped \
  postgres:16-alpine

# Redis (optional, for distributed rate limiting and node queue)
docker run -d \
  --name keycompute-redis \
  --network keycompute-internal \
  -p 6379:6379 \
  -v keycompute_redis_data:/data \
  --restart unless-stopped \
  redis:7-alpine \
  redis-server \
  --requirepass "${REDIS_PASSWORD:-change-me-redis-password}" \
  --maxmemory 256mb \
  --maxmemory-policy allkeys-lru

# Install dioxus-cli
curl -sSL http://dioxus.dev/install.sh | sh

# Load environment variables (recommended to use .env file)
cp .env.example .env
# Edit .env with your actual configuration values
set -a && source .env && set +a

# Start the backend
cargo run -p keycompute-server

# Start the frontend development server (in another terminal)
dx serve --package web --platform web --hot-reload true --addr 0.0.0.0
```

---

## Project Structure

```text
keycompute/
├── crates/                          # Backend core modules (Rust)
│   ├── keycompute-server/            # Axum HTTP service (integrates all modules)
│   ├── keycompute-types/             # Shared types and macros
│   ├── keycompute-db/                # Database ORM (23 tables)
│   ├── keycompute-auth/              # Auth & authorization (JWT + API Key + Password)
│   ├── keycompute-ratelimit/         # Rate limiting engine (In-memory/Redis dual backend)
│   ├── keycompute-pricing/           # Pricing engine (Three-tier + LRU cache)
│   ├── keycompute-routing/           # Two-layer smart routing engine
│   ├── keycompute-runtime/           # Runtime (AES-256-GCM encryption + storage abstraction)
│   ├── keycompute-billing/           # Billing & settlement (Post-stream precise settlement)
│   ├── keycompute-distribution/      # Referral distribution system
│   ├── keycompute-observability/     # Observability three pillars
│   ├── keycompute-config/            # Configuration management (Env vars + TOML)
│   ├── keycompute-emailserver/       # SMTP email service
│   ├── keycompute-payment/           # Payment integration
│   │   ├── keycompute-alipay/        # Alipay payment
│   │   └── keycompute-wechatpay/     # WeChat Pay
│   ├── llm-gateway/                  # LLM execution gateway (single upstream layer)
│   ├── llm-provider/                 # Provider adapters
│   │   ├── keycompute-openai/        # OpenAI
│   │   ├── keycompute-claude/        # Anthropic Claude
│   │   ├── keycompute-gemini/        # Google Gemini
│   │   ├── keycompute-deepseek/      # DeepSeek
│   │   ├── keycompute-ollama/        # Ollama local models
│   │   └── keycompute-vllm/          # vLLM self-hosted
│   ├── node-gateway/                 # Node gateway (registration/heartbeat/task management)
│   └── integration-tests/           # End-to-end integration tests (30+ scenarios)
├── packages/                         # Frontend (Dioxus 0.7)
│   ├── web/                          # Web admin dashboard (9 management modules)
│   ├── ui/                           # Shared UI component library
│   ├── desktop/                      # Desktop native application
│   ├── mobile/                       # Mobile cross-platform application
│   └── client-api/                   # API client wrapper (17 modules)
├── nginx/                            # Nginx reverse proxy configuration
├── Dockerfile.server                 # Backend container image
├── Dockerfile.web                    # Frontend container image
└── docker-compose.yml                # Container orchestration
```

---

## Configuration

### Environment variables

| Variable | Description | Required |
|:---|:---|:---:|
| `KC__DATABASE__URL` | PostgreSQL connection string | ✅ |
| `KC__AUTH__JWT_SECRET` | JWT signing secret | ✅ |
| `KC__CRYPTO__SECRET_KEY` | API Key AES-256-GCM encryption key (cannot be changed after writing) | ✅ |
| `KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET` | HMAC signing secret; issues one-time node registration tokens | ✅ |
| `KC__REDIS__URL` | Redis connection string (optional; without it: rate limiter falls back to in-memory, cache no-ops, node gateway unavailable) | ⚪ |
| `KC__EMAIL__SMTP_HOST` | SMTP host (optional) | ⚪ |
| `KC__EMAIL__SMTP_PORT` | SMTP port (optional) | ⚪ |
| `KC__EMAIL__SMTP_USERNAME` | SMTP username (optional) | ⚪ |
| `KC__EMAIL__SMTP_PASSWORD` | SMTP password (optional) | ⚪ |
| `KC__EMAIL__FROM_ADDRESS` | Sender email address (optional) | ⚪ |
| `KC__EMAIL__FROM_NAME` | Sender display name (optional) | ⚪ |
| `KC__EMAIL__REQUIREMENT_RECIPIENT` | Requirement collection recipient email (optional; required to receive homepage submissions) | ⚪ |
| `APP_BASE_URL` | Public frontend base URL (required for password reset/invite links) | ⚪ |
| `KC__DEFAULT_ADMIN_EMAIL` | Default administrator email (optional) | ⚪ |
| `KC__DEFAULT_ADMIN_PASSWORD` | Default administrator password (optional) | ⚪ |

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

- 🐛 [Report bugs](https://github.com/aiqubits/keycompute/issues/new?template=bug_report.yml)
- 💡 [Feature requests](https://github.com/aiqubits/keycompute/issues/new?template=feature_request.yml)
- 🔧 [Submit code](CONTRIBUTING.md)

---

## License

This project is open sourced under the [MIT](LICENSE) License.

---

<div align="center">

### 💖 Thanks for using KeyCompute

If this project helps you, feel free to give it a ⭐️ star.

**[Quick Start](#quick-start)** • **[Report Issues](https://github.com/aiqubits/keycompute/issues)** • **[Latest Releases](https://github.com/aiqubits/keycompute/releases)**

</div>
