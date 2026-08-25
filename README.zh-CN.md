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

**新一代高性能 AI Token 算力服务平台**

<p align="center">
  <a href="https://github.com/keycompute/keycompute/stargazers"><img src="https://img.shields.io/github/stars/keycompute/keycompute?style=social" alt="GitHub Stars" /></a>
  <a href="https://github.com/keycompute/keycompute/issues"><img src="https://img.shields.io/github/issues/keycompute/keycompute" alt="GitHub Issues" /></a>
  <a href="./LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="MIT License" /></a>
  <a href="./CONTRIBUTING.md"><img src="https://img.shields.io/badge/PRs-welcome-brightgreen" alt="PRs Welcome" /></a>
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/Rust-1.92%2B-orange?logo=rust" alt="Rust Version" /></a>
</p>

<p align="center">
  <a href="#核心特性">核心特性</a> •
  <a href="#架构总览">架构总览</a> •
  <a href="#快速开始">快速开始</a> •
  <a href="#配置说明">配置说明</a> •
  <a href="#项目结构">项目结构</a> •
  <a href="#api-接口">API 接口</a> •
  <a href="#开发指南">开发指南</a>
</p>

</div>

---

## 项目简介

KeyCompute 是一个**高性能**、**易扩展**、**开箱即用**的 AI Token 算力服务平台，提供统一的大模型接入、智能路由、计量计费、算力节点租赁、多级分销和可观测性等企业级能力。

> **纯 Rust 全栈**：后端 (Axum) + 前端 (Dioxus WASM) + CLI 客户端，共享类型与逻辑，极致性能与安全。

> **注意**：本项目仅供个人学习使用，使用者必须在遵循 OpenAI [使用条款](https://openai.com/policies)以及法律法规的情况下使用，不得用于非法用途。根据《生成式人工智能服务管理暂行办法》的要求，请勿对中国地区公众提供一切未经备案的生成式人工智能服务。

---

## 核心特性

### 算力节点租赁
算力节点通过**拉取式轮询**接入成为计算节点，**无需公网 IP**，在本地硬件上运行托管模型，按贡献获取收益。

- **一键接入**：运行独立 CLI 二进制即可自动注册 → 心跳 → 轮询任务 → 本地执行 → 提交结果
- **节点路由**：使用 `node:<模型名>` 显式将请求路由到节点池
- **自动故障转移**：失败节点自动排除，任务重新入队
- **会话持久化**：本地 Session 避免重复注册，优雅关闭保障任务完整性
- **小费机制**：节点所有者可赚取并提现小费

### 统一多模型网关
通过标准 **OpenAI API** 一行代码切换所有大模型：

| Provider | 模型系列 | 实现 |
|:---|:---|:---:|
| 🟢 OpenAI | GPT-4o / GPT-4 / GPT-3.5 等 | ✅ |
| 🟣 Anthropic | Claude 3.5 Sonnet / Opus / Haiku 等 | ✅ |
| 🔵 Google | Gemini 1.5 / 2.0 Flash / Pro 等 | ✅ |
| 🔴 DeepSeek | DeepSeek-V3 / R1 / Chat 等 | ✅ |
| 🟤 Ollama | 本地部署模型 (Llama / Qwen / GLM / MiniMax 等) | ✅ |
| 🟡 vLLM | 自部署任意模型 | ✅ |

> GLM（智谱）和 MiniMax 等可通过 Ollama 适配器本地部署运行，而非独立 Provider 实现。

### 智能路由引擎
**双层路由架构**，多因子加权评分保障最优选择：

```text
score = 0.30 × 成本因子 + 0.25 × 延迟因子 + 0.25 × 成功率 + 0.20 × 健康状态
```

- **模型级路由** → **账号池路由**：自动在 Provider 和账号间择优分配
- **回退链机制**：主目标失败自动切换备用目标
- **指数退避重试**：最多 3 次重试，初始 100ms，最大 10s
- **请求级代理**：支持 Provider 级/账号级/通配符级 HTTP 代理

### 计费与支付体系

- **流结束结算**：请求完成后精确计算，不预扣余额，不影响执行结果
- **三层定价**：租户特定定价 → 数据库默认 → 硬编码兜底（LRU 缓存）
- **精确用量**：优先 Provider 精确 usage，回退 tiktoken 估算
- **在线充值**：支付宝/微信支付 + 余额管理
- **用量统计**：详细的 Token 消耗明细与可视化

### 二级分销系统

- **推荐返佣**：默认一级 3% + 二级 2% 自动计算
- **邀请链接**：一键生成专属邀请链接
- **灵活配置**：管理员可通过 API 配置分销比例
- **收益统计**：实时查看分销收益与推荐列表

### 认证与权限

- **双认证体系**：JWT（用户会话）+ API Key（`sk-...`，API 访问）
- **权限分离**：API Key 即便有 admin 角色也无法访问管理接口
- **完整用户管理**：注册 → 邮箱验证 → 登录 → 密码重置 → 角色管理
- **分组限流**：用户级/租户级/API Key 级限流（内存 / Redis 双后端）

### 可观测性

- **Prometheus 指标**：请求量、延迟、错误率、Provider 健康度
- **分布式追踪**：Provider Span / Request Span / Stream Span
- **结构化日志**：JSON 格式，开发/生产分层输出
- **主机监控**：CPU / 内存 / 磁盘 / 网络实时指标
- **健康检查**：`/health` 接口一键监控服务状态

### 跨平台前端

- **Web 管理后台**：Dioxus WASM SPA
- **桌面端**：Dioxus Desktop 原生应用
- **移动端**：Dioxus Mobile 跨平台支持
- **路由级权限控制**：Admin 角色验证，安全可控

---

## 架构总览

```text
[客户端: Web / Desktop / Mobile (Dioxus)]
                ↕ HTTP/SSE
[API 层: keycompute-server (Axum)]
       ├── 认证 (JWT + API Key)
       ├── 限流 (内存/Redis)
       ├── 路由 (双层引擎)
       └── Gateway (唯一上游执行层)
                ↕
[Provider 适配层]
  ├── OpenAI / Anthropic / Google
  ├── DeepSeek
  ├── Ollama (本地模型)
  └── vLLM (自部署)

[节点计算网络]
  node-token (CLI) ↔ node-gateway ↔ Redis 任务队列 ↔ 本地推理
```

---

## 快速开始

### 环境要求

| 组件 | 版本要求 |
|:---|:---|
| Rust | ≥ 1.92 |
| Axum | ≥ 0.8.0 |
| Dioxus | ≥ 0.7.1 (前端开发) |
| PostgreSQL | ≥ 16 |
| Redis | ≥ 7 (可选，用于分布式限流/节点队列) |
| Docker | 最新版 (容器部署) |

### 方式一：Docker Compose 部署（推荐）

```bash
# 克隆项目
git clone https://github.com/keycompute/keycompute.git
cd keycompute

# 复制并编辑环境变量
cp .env.example .env
# 首次生产启动前替换所有实际凭据；应用至少会强制检查：
# - KC__AUTH__JWT_SECRET：非默认、非纯空白、至少 32 字节
# - KC__CRYPTO__SECRET_KEY：恰好 32 字节的 Base64 编码
# - KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET：非默认、至少 16 字节
#   （本 Compose 启用 Redis/Node Gateway，因此必填）
# - KC__DEFAULT_ADMIN_PASSWORD：非默认、非纯空白、至少 12 个字符
#   （仅首次创建 system 管理员时必填）

# 启动所有服务
docker compose up -d

# 查看服务状态
docker compose ps
```

部署完成后访问 `http://localhost` 即可使用（或使用 `WEB_PORT` 指定的端口）。

未覆盖时，引导管理员邮箱为 `admin@keycompute.local`。密码是启动前设置的
`KC__DEFAULT_ADMIN_PASSWORD`；全新生产数据库不会接受
`change-me-admin-password`。首个 `system` 管理员创建完成后，应从环境变量中
删除这一一次性引导密码，后续重启不再需要它。

### 方式二：本地开发

> ⚠️ **安全警告**：下方显示的默认值（`change-me-*`）仅用于演示。
> **切勿在生产环境中使用！** 请使用以下命令生成强随机密码：
> ```bash
> openssl rand -base64 32
> ```
> 普通 `cargo run` 构建 debug 可执行文件，启动方式本身即选定开发模式。
> 它仅读取 `config.toml`，可使用公开的本地凭据并监听 `0.0.0.0`。
> `.env.example`、`KC__*` 和 `APP_BASE_URL` 不会覆盖该文件；切勿用于生产。

```bash
# 创建 cargo run 使用的配置
cp config.example.toml config.toml

# 启动 PostgreSQL 与 Redis，并开放与 config.toml 匹配的本机端口
docker compose --env-file .env.example -f docker-compose.yml -f docker-compose.dev.yml up -d postgres redis

# 安装 dioxus-cli
curl -sSL http://dioxus.dev/install.sh | sh

# 启动后端
cargo run -p keycompute-server

# 启动前端（另一个终端）
dx serve --package web --platform web --hot-reload true --addr 0.0.0.0
```

`cargo run -p keycompute-server --release` 与 Docker 镜像选定生产模式。
release 可执行文件仅读取环境变量并忽略 `config.toml`；不存在可以
改变运行模式的配置参数或环境变量。

---

## 项目结构

```text
keycompute/
├── crates/                          # 后端核心模块 (Rust)
│   ├── keycompute-server/            # Axum HTTP 服务（整合所有模块）
│   ├── keycompute-types/             # 全局共享类型与宏
│   ├── keycompute-db/                # 数据库 ORM 与迁移
│   ├── keycompute-auth/              # 认证与鉴权（JWT + API Key + 密码）
│   ├── keycompute-cache/             # 缓存抽象与 Redis 支持
│   ├── keycompute-ratelimit/         # 限流引擎（内存/Redis 双后端）
│   ├── keycompute-pricing/           # 定价引擎（三层兜底 + LRU 缓存）
│   ├── keycompute-routing/           # 双层智能路由引擎
│   ├── keycompute-runtime/           # 运行时（AES-256-GCM 加密 + 存储抽象）
│   ├── keycompute-billing/           # 计费结算（流结束后精确结算）
│   ├── keycompute-distribution/      # 二级分销系统
│   ├── keycompute-observability/     # 可观测性三支柱
│   ├── keycompute-config/            # debug TOML / release 环境变量分离加载
│   ├── keycompute-emailserver/       # SMTP 邮件服务
│   ├── keycompute-payment/           # 支付集成
│   │   ├── keycompute-alipay/        # 支付宝支付
│   │   └── keycompute-wechatpay/     # 微信支付
│   ├── llm-gateway/                  # LLM 执行网关（唯一上游层）
│   ├── llm-protocol/                 # 协议转换与 Provider 定义
│   │   ├── openai/                   # OpenAI 兼容协议
│   │   ├── anthropic/                # Anthropic 兼容协议
│   │   └── provider/                 # 共享 Provider 协议类型
│   ├── node-gateway/                 # 节点网关（注册/心跳/任务管理）
│   └── integration-tests/           # 端到端集成测试
├── packages/                         # 前端 (Dioxus 0.7)
│   ├── web/                          # Web 管理后台
│   ├── ui/                           # 共享 UI 组件库
│   ├── desktop/                      # 桌面端原生应用
│   ├── mobile/                       # 移动端跨平台应用
│   └── client-api/                   # API 客户端封装
├── nginx/                            # Nginx 反向代理配置
├── Dockerfile.server                 # 后端容器镜像
├── Dockerfile.web                    # 前端容器镜像
├── docker-compose.yml                # 生产容器编排
├── docker-compose.dev.yml            # 本地依赖端口覆盖
└── docker-compose.replicas.yml       # 生产读副本编排
```

---

## 配置说明

### 环境变量

下表仅适用于 release/Docker 生产启动；debug `cargo run` 改为读取 `config.toml`。

| 变量名 | 说明 | 必填 |
|:---|:---|:---:|
| `KC__DATABASE__URL` | PostgreSQL 连接串 | ✅ |
| `KC__AUTH__JWT_SECRET` | 生产环境：非默认、非纯空白且至少 32 字节的 JWT 签名密钥 | ✅ |
| `KC__CRYPTO__SECRET_KEY` | 生产环境：恰好 32 字节的 Base64 编码；写入 Provider API Key 后不可更改 | ✅ |
| `KC__NODE_GATEWAY__REGISTRATION_TOKEN_SECRET` | 生产环境配置 Redis 时：非默认且至少 16 字节的 HMAC 密钥；签发一次性节点注册 token | 条件必填 |
| `KC__REDIS__URL` | Redis 连接串（可选；不配置时限流降级为内存、缓存变为 no-op、节点网关不可用） | ⚪ |
| `KC__EMAIL__SMTP_HOST` | SMTP 服务器地址（可选） | ⚪ |
| `KC__EMAIL__SMTP_PORT` | SMTP 服务器端口（可选） | ⚪ |
| `KC__EMAIL__SMTP_USERNAME` | SMTP 用户名（可选） | ⚪ |
| `KC__EMAIL__SMTP_PASSWORD` | SMTP 密码（可选） | ⚪ |
| `KC__EMAIL__FROM_ADDRESS` | 发件邮箱地址（可选） | ⚪ |
| `KC__EMAIL__FROM_NAME` | 发件人显示名称（可选） | ⚪ |
| `KC__EMAIL__REQUIREMENT_RECIPIENT` | 需求收集表单接收邮箱（可选；接收首页提交需求时需要配置） | ⚪ |
| `APP_BASE_URL` | 当前部署的公开前端地址；启用 SMTP 时必填，启用公开邀请链接前也必须配置 | 条件必填 |
| `KC__DEFAULT_ADMIN_EMAIL` | 默认管理员邮箱 | ⚪ |
| `KC__DEFAULT_ADMIN_PASSWORD` | 一次性引导密码：仅生产环境首次创建 `system` 管理员时必填；非默认、非纯空白且至少 12 个字符 | 条件必填 |

---

## API 接口

### OpenAI 兼容 API

```bash
# Chat Completions（流式 + 非流式）
curl http://localhost:3000/v1/chat/completions \
  -H "Authorization: Bearer sk-xxx" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "deepseek-chat",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": true
  }'

# 列出可用模型
curl http://localhost:3000/v1/models \
  -H "Authorization: Bearer sk-xxx"
```

### 管理 API 概览

| 分类 | 接口 | 说明 |
|:---|:---|:---|
| 认证 | `POST /api/v1/auth/register` | 用户注册 |
| | `POST /api/v1/auth/login` | 用户登录 |
| | `POST /api/v1/auth/forgot-password` | 忘记密码 |
| 用户 | `GET /api/v1/me` | 当前用户信息 |
| | `GET/POST /api/v1/keys` | API Key 管理 |
| 计费 | `GET /api/v1/usage` | 用量统计 |
| | `GET /api/v1/billing/records` | 账单记录 |
| 支付 | `POST /api/v1/payments/orders` | 创建支付订单 |
| | `GET /api/v1/payments/balance` | 余额查询 |
| 分销 | `GET /api/v1/me/distribution/earnings` | 分销收益 |
| 节点 | `GET /api/v1/me/node-gateway/token` | 节点令牌 |
| | `GET /api/v1/me/tips` | 小费摘要 |
| 管理 | `GET/POST /api/v1/accounts` | 上游账号管理 |
| | `GET/POST /api/v1/settings` | 系统设置 |
| | `GET/POST /api/v1/pricing` | 定价管理 |
| | `GET /api/v1/admin/monitoring/overview` | 监控概览 |

> 完整的 API 文档请参考项目源码中的路由定义。

---

## 开发指南

```bash
# 编译（排除桌面端和移动端）
cargo build --workspace --exclude desktop --exclude mobile --verbose

# 运行单元测试
cargo test --lib --workspace --exclude desktop --exclude mobile --verbose

# 运行集成测试
cargo test --package integration-tests --tests --verbose

# 运行前端 API 客户端测试
cargo test --package client-api --tests --verbose

# Clippy 代码检查
cargo clippy --workspace --exclude desktop --exclude mobile --all-targets --all-features --future-incompat-report -- -D warnings

# 代码格式化检查
cargo fmt --all --check

# 构建发布版本
cargo build -p keycompute-server --release
```

---

## 如何贡献

我们欢迎各种形式的贡献！请阅读 [CONTRIBUTING.md](CONTRIBUTING.md) 了解如何参与项目开发。

- 🐛 [报告 Bug](https://github.com/keycompute/keycompute/issues/new?template=bug_report.yml)
- 💡 [功能建议](https://github.com/keycompute/keycompute/issues/new?template=feature_request.yml)
- 🔧 [提交代码](CONTRIBUTING.md)

---

## 许可证

本项目采用 [MIT](LICENSE) 许可证开源。

---

<div align="center">

### 💖 感谢使用 KeyCompute

如果这个项目对你有帮助，欢迎给我们一个 ⭐️ Star！

**[快速开始](#快速开始)** • **[问题反馈](https://github.com/keycompute/keycompute/issues)** • **[最新发布](https://github.com/keycompute/keycompute/releases)**

</div>
