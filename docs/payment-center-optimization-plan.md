# 支付模块与支付中心优化方案

> 适用范围：KeyCompute Web、Server、Client API、数据库与支付 provider crate  
> 目标版本：Dioxus 0.7.x  
> 方案结论：保留支付宝，完整接入微信支付 API v3；支付方式由服务端按真实运行状态判定，前端只展示当前可用渠道。

## 1. 目标与边界

本次改造需要同时解决三个问题：

1. 支付模块从“支付宝单实现”升级为可承载支付宝、微信支付的统一支付域。
2. 完成微信支付 Native 扫码链路：下单、展示二维码、回调验签与解密、查单、关单、幂等入账、超时处理和对账。
3. 重构用户充值页与管理员支付中心，使其与当前控制台的 Linear 风格保持一致；不可用支付方式不占位、不置灰，直接不展示。

本期不包含退款、分账、代金券、JSAPI/小程序支付。接口设计为后续扩展保留能力，但首期微信只开放 Native 支付；支付宝继续支持 Page/WAP/QR。

## 2. 现状审计

### 2.1 已有基础

- 已有 `payment_orders`、`user_balances` 与余额流水，可复用现有“订单更新 + 余额充值”的事务逻辑。
- 支付宝实现已覆盖 Page、WAP、QR、异步通知和主动查单。
- 系统设置已有 `alipay_enabled`、`wechatpay_enabled` 开关，公开设置也会返回这两个字段。
- 用户端已有余额、充值记录、用量明细和充值交互；管理员已有支付订单列表。

### 2.2 必须修复的问题

| 位置 | 当前问题 | 影响 |
| --- | --- | --- |
| `keycompute-wechatpay` | 仍是 `add(2, 2)` 模板 | 微信支付完全未实现 |
| `AppState.payment` | 类型固定为支付宝 `PaymentService` | 服务端无法同时装载两个渠道 |
| 创建订单接口 | 请求只有 `payment_type`，没有 `payment_method` | 前端选择微信后仍会进入支付宝服务 |
| 订单落库 | `PaymentOrder::create` 固定写入 `alipay` | 渠道记录错误，后续查单与回调无法路由 |
| 可用性判断 | 仅检查数据库启用开关 | 开关已开但密钥缺失时仍可能向用户展示 |
| 公开设置 | 直接暴露管理员开关 | “启用”被错误等同为“可用” |
| 同步查单 | 只按商户订单号查单，未校验当前用户归属 | 存在越权触发他人订单同步的风险 |
| 回调模型 | 交易号字段和注释绑定支付宝 | 无法准确保存微信 `transaction_id` |
| 用户端 | 两种方式被硬编码同时显示 | 不可用渠道也出现；无渠道时没有空状态 |
| 管理端 | 只有订单表，没有渠道健康、回调和异常视图 | 运维无法判断“为何不可用”和“是否正常入账” |

## 3. 总体架构

将支付实现拆成四层：

```text
充值页 / 管理员支付中心
          │
          ▼
统一 Payment API（鉴权、金额规则、订单归属）
          │
          ▼
PaymentRegistry（渠道发现、可用性、路由）
       ┌──┴──────────────┐
       ▼                 ▼
AlipayProvider     WechatPayProvider
       └──┬──────────────┘
          ▼
订单状态机 + 幂等通知 + 余额入账事务 + 对账任务
```

服务端定义统一 provider 接口，避免 handler 继续依赖具体厂商：

```rust
#[async_trait]
pub trait PaymentProvider: Send + Sync {
    fn method(&self) -> PaymentMethod;
    fn capabilities(&self) -> &'static [PaymentScene];
    fn health(&self) -> ProviderHealth;

    async fn create_order(&self, order: ProviderOrderRequest)
        -> Result<ProviderOrderResult, PaymentError>;
    async fn query_order(&self, out_trade_no: &str)
        -> Result<ProviderTradeState, PaymentError>;
    async fn close_order(&self, out_trade_no: &str)
        -> Result<(), PaymentError>;

    fn verify_and_decode_notify(
        &self,
        request: &PaymentNotifyRequest,
    ) -> Result<VerifiedPaymentNotify, PaymentError>;
}
```

`PaymentNotifyRequest` 是支付域自己的 DTO，包含规范化请求头集合和未经修改的原始 body 字节，不直接依赖 Axum 的 `HeaderMap`，避免 provider crate 与 Web 框架耦合。

回调验签和解密由各 provider 自己处理，但“校验本地订单、状态迁移、幂等入账”必须进入统一领域服务，不能在两套 provider 中复制。

Registry 必须区分新订单准入与存量订单处理，不能用同一个 `available` 判断覆盖两种语义：

```rust
// 新订单：同时检查运营开关、验证状态、运行健康和支持场景
registry.for_new_order(method, scene)?;

// 存量订单：忽略运营开关和新单熔断，只要求 provider 与处理该订单所需的密钥仍可用
registry.for_existing_order(method)?;
```

关闭渠道只改变 `for_new_order` 的结果。回调、查单、关单和补偿任务必须根据订单中已经固化的 `payment_method` 走 `for_existing_order`。

## 4. 支付方式可用性判定

### 4.1 核心规则

支付方式只有同时满足以下条件才允许创建新订单并对用户可见：

```text
accepts_new_orders = 管理员启用
                  && 编译包含该 provider
                  && configured
                  && verified
                  && provider 初始化成功
                  && 运行状态为 available 或 degraded
```

仅有 `*_enabled = true` 不代表可用。管理员开关表达“运营意愿”，配置、验证和运行状态表达“技术上可提供服务”，用户侧只认四者的交集。存量订单处理不受管理员开关或新单熔断影响。

### 4.2 状态模型

每个渠道分别维护三组正交状态，避免管理员开关、配置状态和运行健康互相覆盖：

- 运营状态：`enabled` / `disabled`。
- 准备状态：`misconfigured` / `configured_unverified` / `verified`。
- 运行状态：`available` / `degraded` / `unavailable`。

其中：

- `misconfigured`：必填项缺失、URL 不合法、密钥或证书无法解析、商户私钥自签自验失败。
- `configured_unverified`：本地检查通过，但尚未完成真实商户身份、App ID 绑定和产品权限验证。
- `verified`：当前配置指纹下至少一次受控验证订单或真实下单成功，确认当前商户配置有对应产品权限。
- `available`：近期没有确定性故障。
- `degraded`：近期请求有临时失败，但仍允许创建订单；管理端告警，用户端仍可见。
- `unavailable`：认证失败、商户权限失败、连续确定性故障达到阈值或人工熔断；停止新单，用户端隐藏。

建议熔断规则：明确的认证/商户权限失败首次发生即进入 `unavailable`；其他确定性业务错误在 5 分钟内连续 3 次后进入 `unavailable`；网络超时只进入 `degraded`，不因一次网络抖动隐藏渠道。60 秒后允许后台任务半开探测，创建或查询订单成功可恢复 `available`，但不得改变运营开关。管理员可手动“重新检测”。

### 4.3 启动检查与运行检查

支付宝启动检查：App ID、商户私钥、支付宝公钥、通知 URL 必填；RSA2 密钥可解析；用商户私钥签名并做本地结构校验；通知 URL 必须是生产环境 HTTPS 地址。

微信启动检查：`appid`、`mchid`、商户证书序列号、商户 API 私钥、API v3 密钥、微信支付公钥 ID/公钥、通知 URL 必填；API v3 密钥必须为 32 字节；RSA 私钥与微信支付公钥可解析；生产通知 URL 必须是 HTTPS。启动检查通过只进入 `configured_unverified`，不能直接宣称渠道真实可用。

微信 `appid` 与 `mchid` 的真实绑定和 Native 产品权限无法仅靠本地密码学检查证明。生产开放前必须由管理员执行一次受控验证订单：成功取得 `code_url` 后立即关单，或由专用白名单验证流程真实下单成功，把状态提升为 `verified`；该流程不通过普通用户方法列表进入，普通用户不得承担首单探测。支付宝采用相同的 `configured_unverified → verified` 规则；迁移时只有使用当前配置成功查询历史订单，才能把支付宝标记为 `verified`。

验证状态必须绑定 `verified_config_fingerprint`，指纹覆盖 App ID、商户号、证书/公钥 ID、通知 URL 和密钥版本的不可逆摘要。上述任一配置变化都立即清除原验证状态，回到 `configured_unverified`，不得沿用旧凭证的验证结果。

不建议为了展示页面而每次实时调用第三方接口。`GET /payments/methods` 读取健康快照，避免页面加载速度受第三方网络影响。数据库保存单调递增的 `payment_config_version`、配置指纹、验证状态和集群级熔断状态，Redis Pub/Sub 只负责加速失效通知；各实例同时保留不超过 30 秒的数据库版本轮询兜底，Redis 不可用不能产生第二套事实来源。

每个实例针对当前配置版本上报短期 readiness 心跳。只有所有仍处于服务发现中的活跃实例都完成当前版本 provider 初始化，集群准入状态才允许对用户返回该渠道；未完成初始化的实例必须从支付 API 流量中摘除或使集群准入关闭。创建订单仍需在本机执行最终准入检查，不能信任页面早先获得的结果。这样避免方法查询命中实例 A、创建请求命中未就绪实例 B 时出现状态分裂。

### 4.4 对外返回

新增认证接口：

```http
GET /api/v1/payments/methods
```

```json
{
  "methods": [
    {
      "code": "wechatpay",
      "display_name": "微信支付",
      "scenes": ["native"],
      "recommended_scene": "native",
      "sort_order": 10,
      "is_default": true
    }
  ],
  "min_amount": "1.00",
  "max_amount": "100000.00",
  "currency": "CNY"
}
```

接口只返回 `accepts_new_orders = true` 的渠道，不向普通用户返回错误原因、密钥状态或内部健康细节。返回顺序必须按服务端稳定的 `sort_order` 排序，并显式返回 `is_default`，前端不得依赖 HashMap 或注册顺序。若列表为空，充值入口隐藏；直接访问充值路由时显示“当前暂无可用支付方式”，并保留返回支付与账单页的操作。

## 5. 微信支付完整实现

### 5.1 配置与密钥

在 `keycompute-config` 增加支付配置。非敏感项可以写入 TOML；敏感项只允许来自 `KC__` 分层环境变量或密钥管理服务，不写入 `system_settings`，不通过管理 API 回传明文。项目当前配置加载器不支持在 TOML 字符串中展开 `${ENV_NAME}`，因此方案不使用这种占位符：

```toml
[payment.wechatpay]
notify_url = "https://example.com/api/v1/payments/notify/wechatpay"
timeout_minutes = 15
```

部署环境注入：

```text
KC__PAYMENT__WECHATPAY__APPID=...
KC__PAYMENT__WECHATPAY__MCHID=...
KC__PAYMENT__WECHATPAY__MERCHANT_SERIAL_NO=...
KC__PAYMENT__WECHATPAY__MERCHANT_PRIVATE_KEY=...
KC__PAYMENT__WECHATPAY__API_V3_KEY=...
KC__PAYMENT__WECHATPAY__WECHATPAY_PUBLIC_KEY_ID=...
KC__PAYMENT__WECHATPAY__WECHATPAY_PUBLIC_KEY=...
```

优先采用微信支付公钥模式；它比平台证书模式少一套证书自动轮换逻辑。日志、错误响应和 Debug 输出必须对私钥、API v3 密钥、签名、完整回调密文脱敏。

### 5.2 Native 下单

实现 `POST /v3/pay/transactions/native`：

1. 服务端生成 6–32 位、全系统唯一的 `out_trade_no`。全局唯一强于微信要求的商户号内唯一，可以保证回调、运维搜索和兼容期查询都不产生歧义。
2. 本地先创建 `pending` 订单，金额使用 `Decimal`；转微信请求时严格转为分，禁止经过 `f64`。
3. 组装 `appid`、`mchid`、`description`、`out_trade_no`、`notify_url`、`time_expire` 和 `amount.total`。
4. 使用商户 API 私钥生成 `WECHATPAY2-SHA256-RSA2048` Authorization。
5. 校验微信响应签名，读取 `code_url`，写回订单的支付载荷。
6. 前端根据 `code_url` 在本地生成二维码；不再依赖第三方二维码图片 URL，避免泄漏支付链接。
7. 第三方下单失败时记录规范化错误码；确定失败标记 `failed`，结果未知保留 `pending` 并进入主动查单。

### 5.3 支付回调

新增：

```http
POST /api/v1/payments/notify/wechatpay
```

处理顺序固定为：读取原始请求体和 `Wechatpay-*` 请求头 → 校验时间戳窗口与签名 → 使用 API v3 密钥 AES-256-GCM 解密 `resource` → 校验 `appid`、`mchid`、`out_trade_no`、币种和金额 → 开启数据库事务 → 写入/锁定通知去重记录 → 锁定并更新订单 → 写余额流水并更新余额 → 将通知标记为 `processed` → 提交事务 → 返回 200/204。

任何验签、商户身份或金额校验失败都不得进入业务入账事务。验签失败、商户身份不符、金额不一致等拒绝结果写入单独的 `payment_security_events` 安全事件表或安全日志，只保存渠道、时间、请求 ID、拒绝分类、来源信息和原始 body 哈希，不保存可重放的敏感明文；该数据设置限流和短期清理策略。数据库临时错误回滚全部业务变更，并在事务外 best-effort 写入不参与去重判断的 `payment_processing_attempts`，随后返回非 2xx 以触发微信重试。

重复通知不能仅凭唯一键冲突直接返回成功：必须读取通知处理状态和订单状态。仅当通知为 `processed` 且订单已经 `paid` 时返回成功；若存在非终态历史记录，或通知存在但订单未支付，必须重新进入幂等入账流程。处理失败不得提交占用去重键的终态通知记录。

### 5.4 查单、关单与状态映射

- 主动查单：`GET /v3/pay/transactions/out-trade-no/{out_trade_no}?mchid=...`。
- 关闭订单：`POST /v3/pay/transactions/out-trade-no/{out_trade_no}/close`。
- 本地 `pending` 超过有效期后先向微信查单；若仍为 `NOTPAY`，调用关单后再置为 `closed`，不能只改本地状态。
- 微信 `SUCCESS` → `paid`，`NOTPAY/USERPAYING` → `pending`，`CLOSED/REVOKED` → `closed`，`PAYERROR` → `failed`。未知状态不擅自映射，保留本地状态并告警。
- 回调是主路径，前端轮询和主动查单是补偿路径；两条路径必须调用同一个幂等入账函数。

### 5.5 入账幂等与一致性

在单个数据库事务中执行：

1. 插入通知记录；若唯一键冲突，则 `SELECT ... FOR UPDATE` 读取现有通知处理状态。
2. `SELECT ... FOR UPDATE` 锁定订单。
3. 若通知已经 `processed` 且订单已经 `paid`，提交空操作并返回成功。
4. 校验订单当前状态允许迁移到 `paid`，并再次核对渠道、金额和商户身份。
5. 保存渠道交易号、回调摘要与支付时间。
6. 新增余额流水，增加可用余额和累计充值。
7. 将通知状态更新为 `processed`。
8. 提交事务后再发送日志/指标事件。

数据库增加双保险：`out_trade_no` 全局唯一；`(payment_method, provider_trade_no)` 唯一；充值流水对 `order_id + transaction_type='recharge'` 建立部分唯一索引；通知表对 `(payment_method, provider_event_id)` 唯一。主动查单补偿没有第三方通知 ID，应生成确定性的内部事件键 `sync:{payment_method}:{provider_trade_no}`，并与回调路径调用同一个入账事务。即使回调和查单并发，也只能入账一次。

## 6. 数据模型与 API 调整

### 6.1 数据库

建议新增迁移，不直接修改 `001_init.sql`：

- 将语义为支付宝的 `trade_no` 演进为 `provider_trade_no`；兼容期可先新增列并回填。
- `payment_method` 改为创建订单时必填，不再由模型默认支付宝。
- 增加 `payment_scene`、`provider_payload JSONB`、`last_error_code`、`last_error_message`、`last_synced_at`。
- 新增 `payment_notifications`：provider 事件 ID、渠道、订单号、`processing_status`（`received/processed`）、失败原因、时间戳；该表只接收已经通过渠道验签和身份校验的业务通知。`received` 仅是事务内中间态，正常提交后必须为 `processed`；处理失败时与入账事务一起回滚，不留下会阻止重试的去重记录。
- 新增 `payment_security_events` 或接入结构化安全日志，记录验签失败、时间戳过期、无法解密、商户身份不符和金额不一致等拒绝事件；只保存不可逆 body 哈希及必要元数据，设置写入限流和较短保留期。
- 新增 `payment_processing_attempts` 或使用结构化运维事件，best-effort 记录数据库临时故障、provider 超时等处理失败；该记录不参与幂等判断，写入失败也不能改变给支付平台的重试响应。
- 增加上述交易号、余额流水和通知幂等唯一索引。
- 管理员订单查询增加 `payment_method`、状态、时间区间、订单号/用户搜索索引。
- 增加支付渠道状态记录：`payment_config_version`、`config_fingerprint`、`verified_config_fingerprint`、验证时间、集群级熔断状态；运营开关更新事务内单调递增版本，用于多实例 Registry 缓存失效。部署环境密钥变化在启动时计算出新指纹并清除旧验证状态。
- 多实例部署增加短期 provider readiness 心跳（数据库表或现有服务发现后端），按 `instance_id + payment_config_version + payment_method` 记录；过期实例不参与集群准入计算。

### 6.2 创建订单协议

改为明确传递渠道与场景：

```json
{
  "payment_method": "wechatpay",
  "payment_scene": "native",
  "amount": "100.00"
}
```

`subject`、`body` 由服务端根据站点和充值订单生成，不信任客户端提供的商品描述。服务端再次调用 Registry 检查渠道可用性，前端隐藏不能替代后端校验。

首期充值币种固定为 `CNY`。后台现有“默认币种”不能直接改变支付订单币种；如果默认币种不是 CNY，支付方式发现接口仍返回渠道实际支持的 `CNY`，金额展示和订单落库均以该币种为准。未来支持多币种时，将限额与币种改为 provider/scene 维度，而不是一个全局值。

响应使用统一展示模型：

```json
{
  "order_id": "...",
  "out_trade_no": "...",
  "payment_method": "wechatpay",
  "expires_at": "...",
  "presentation": {
    "type": "qr_code",
    "content": "weixin://wxpay/..."
  }
}
```

支付宝网页支付返回 `presentation.type = redirect`。前端只按展示类型渲染，不写“微信一定是 QR、支付宝一定是 Page”的分支。

同步接口改为订单 ID：

```http
POST /api/v1/payments/orders/{order_id}/sync
```

服务端必须校验订单属于当前用户/租户；从订单读取并固化 `payment_method`，通过 `for_existing_order` 路由，不能使用当前默认渠道。增加每用户和每订单限流，避免把同步接口变成第三方查单放大器。

### 6.3 支付方式信息来源收口

`GET /api/v1/payments/methods` 是用户端支付方式展示的唯一事实来源。现有公开设置中的 `alipay_enabled`、`wechatpay_enabled` 在兼容期只表达管理员意愿，不得被前端用于决定是否展示渠道，并按以下步骤废弃：

1. `Recharge`、`PaymentsOverview` 和 `PublicSettingsStore` 移除基于两个开关的支付方式判断。
2. 公开设置字段增加 deprecated 标记或改名为仅管理语义的 `*_admin_enabled`；新客户端不再消费。
3. 至少保留一个发布版本的兼容返回，确认没有旧客户端依赖后再从公开响应删除。
4. 管理员使用独立的 `GET /api/v1/admin/payments/providers` 查看所有渠道的运营、配置验证和运行状态；普通方法接口永远不返回内部原因。
5. 管理员通过独立的高权限 `POST /api/v1/admin/payments/providers/{method}/verify` 创建受控验证订单。该接口不依赖用户端方法列表，验证成功后立即关单，并把当前配置指纹标记为 `verified`。

## 7. 用户支付中心与充值页重构

截图中的页面结构可保留，但视觉和状态表达需要收敛到现有后台控制台规范：

- 页面内容使用控制台统一 `page-container`、紧凑标题和 12px 内圆角，去除超宽空白卡片和大面积浅紫选中底色。
- 支付方式卡片由接口循环生成；一项时单列紧凑显示，两项时双列；使用正式品牌图标替代 emoji。
- 不可用方式不渲染，切勿显示禁用卡片或“建设中”。异步加载期间展示骨架，避免先闪现两个渠道再消失。
- 默认选中 `is_default = true` 的渠道；缺失时才选择按 `sort_order` 排序后的第一项。可用列表变化时校正选中值，禁止保留已失效渠道。
- 金额按钮、输入框和主按钮置于最大约 720px 的表单工作区；按钮文案明确为“使用微信支付 ¥100.00”。
- 金额校验使用服务端返回的最小/最大值；输入支持两位小数，提交后锁定，防重复点击。
- 微信待支付页显示二维码、剩余有效时间、金额和订单号；二维码过期后提供“重新下单”，不复用旧 `code_url`。
- 支付成功后自动刷新余额与充值记录；轮询采用 2s、3s、5s、8s 退避，页面隐藏时暂停，达到过期时间后停止。
- 所有错误展示用户可理解的分类信息，不透出第三方原始报错和内部配置。

若没有可用渠道，`/payments` 页面保留余额和账单能力，但隐藏“立即充值”；充值页展示统一空状态，避免空白页。

## 8. 管理员支付中心重构

将当前分散的设置开关和支付订单列表收拢为 `/admin/payments`，采用当前管理员控制台的安静、紧凑、低噪声风格：

### 8.1 页面信息架构

1. **概览**：今日实收、成功订单、待处理订单、失败率、回调积压；每项可点击进入过滤结果。
2. **支付渠道**：支付宝、微信支付状态卡。显示运营开关、运行状态、支持场景、最近成功时间、最近错误摘要和“检测配置”。密钥只显示“已配置/未配置”，永不回显。
3. **订单**：按渠道、状态、时间筛选，支持订单号、渠道交易号、用户邮箱搜索；列出金额、渠道、状态、创建/支付时间。
4. **回调与异常**：验签失败、金额不一致、重复通知、入账失败、长时间 pending；支持按订单重新查单，涉及余额调整仍需独立高权限审计流程。
5. **基础规则**：最小/最大充值额、支付币种（首期固定 CNY）。开关变更后在同一数据库事务内递增 `payment_config_version`、提交后广播缓存失效，并显示各实例可重建的检测结果。

### 8.2 交互规则

- 管理员打开开关但配置不完整时，保存开关可以成功，但渠道状态显示 `misconfigured`，并列出缺失字段；用户端仍不展示。
- “检测配置”只执行本地签名自检和无副作用检查，成功后状态为 `configured_unverified`，不宣传为真实可用；“验证支付能力”是单独的高权限操作，通过受控小额订单或白名单真实订单把渠道提升为 `verified`。
- 关闭渠道只阻止新订单；历史订单仍可查单、收回调和完成入账，避免用户已经付款却无法到账。
- 状态色只用于徽标和细线，不使用大块高饱和背景；表格、筛选器、按钮复用现有 UI 组件。

## 9. 安全、审计与可观测性

- 金额全链路使用 `Decimal`/最小货币单位，移除前端 client API 中 `f64` 构造金额的方式。
- 回调必须保留原始 body 进行验签；在验签前不解析、重排或重新序列化 JSON。
- 微信回调校验时间戳容差，防止重放；通知 ID 和交易号做数据库去重。
- 管理配置接口禁止读取或写入明文密钥；密钥轮换通过部署环境完成。轮换时必须保留处理存量订单所需的旧密钥/公钥验证材料直到最长订单与回调重试窗口结束。
- 管理员的开关、检测、手工查单操作写审计日志，包含操作者、渠道、结果和时间。
- 指标：按渠道统计下单成功率/耗时、支付成功率、回调验签失败、回调处理耗时、重复通知、主动查单、入账失败和 pending 年龄。
- 告警：渠道进入 `unavailable`、验签失败突增、金额不一致、入账事务失败、超过阈值的 pending 订单。
- 定时对账任务扫描“第三方成功但本地未 paid”和“本地 paid 但缺少渠道交易号”的异常；后续可接入日账单下载。

## 10. 测试方案

### 10.1 单元测试

- 微信 Authorization 签名固定向量、响应验签、AES-256-GCM 回调解密。
- 元转分边界：`0.01`、两位小数、超限、拒绝三位小数和溢出。
- 两个 provider 的状态映射、配置校验、可用性状态机和熔断恢复。
- 回调金额、商户号、App ID、签名、时间戳异常均不得入账。
- `configured_unverified` 不得出现在用户方法列表；只有管理员受控验证或白名单真实下单成功后才能进入 `verified`。

### 10.2 集成测试

- 使用现有 `HttpTransport` mock 覆盖微信下单、查单、关单和网络结果未知。
- 回调重复 10 次只产生一笔余额流水。
- 回调与主动查单并发仍只入账一次。
- 管理员关闭渠道后不能创建新订单，但旧订单回调仍能入账。
- 管理员关闭或渠道新单熔断后，旧订单仍可通过 `for_existing_order` 查单、关单和处理回调。
- 同步他人订单返回 404/403；普通用户看不到不可用原因和配置内容。
- 方法发现接口覆盖：仅支付宝、仅微信、两者、无可用渠道四种组合。
- 多实例下更新运营开关后，配置版本、readiness 心跳和失效广播能让所有活跃实例在约定时间内收敛；任一支付 API 实例未完成当前版本初始化时，集群不开放该渠道。即使读取旧页面快照，创建接口也会拒绝已经关闭的渠道。
- 公开设置中的旧支付开关不能影响任何用户端渠道展示。
- 配置测试使用项目实际支持的 `KC__PAYMENT__...` 环境变量，确认 TOML 中不存在未解析的 `${...}` 字面量。

### 10.3 前端测试

- 只渲染接口返回渠道；加载时无闪烁；无渠道为空状态。
- 选择渠道后请求体的 `payment_method` 与 `payment_scene` 正确。
- 二维码过期、轮询恢复、重复提交、移动端布局和暗色模式。
- 管理中心的状态、筛选、空表和错误态符合现有设计 token。

生产上线前优先使用商户侧确实提供的受控验证能力；标准 Native 支付没有可用沙箱时，使用白名单用户和满足平台及本站最低金额的小额真实订单完成端到端验收。必须验证反向代理不会修改回调 body，公网 HTTPS 回调可达。

## 11. 实施顺序与工期建议

| 阶段 | 内容 | 建议工期 |
| --- | --- | --- |
| P0 | 最终数据库结构、统一 provider trait、Registry 双路由、集群配置状态、订单归属修复、方法发现接口 | 3–4 天 |
| P1 | 微信 API v3 配置、签名验签、Native 下单、查单、关单、受控验证 | 4–5 天 |
| P2 | 微信回调解密、统一幂等入账、通知表、并发测试 | 3–4 天 |
| P3 | 用户支付中心/充值页重构与动态渠道展示 | 2–3 天 |
| P4 | 管理员支付中心、健康状态、异常视图与审计 | 4–5 天 |
| P5 | 端到端联调、可观测性、灰度与故障演练 | 2–3 天 |

总计约 18–24 人日。支付安全与入账一致性应先于界面改造合入；不建议先把微信入口上线再补回调和幂等。

## 12. 上线与回滚

1. 重建数据库并由服务初始化当前版本的完整 schema；系统不执行旧库升级或历史数据迁移。保持微信开关关闭。
2. 部署 Registry 与支付宝适配，确认支付宝链路无回归后再开始微信渠道验证。
3. 配置微信密钥，本地检测通过后状态为 `configured_unverified`；通过管理员受控验证或白名单真实订单提升为 `verified`。
4. 使用满足微信平台和本站 `min_recharge_amount` 的小额订单验证下单、回调、主动查单、重复通知和余额流水。
5. 逐步放量，重点观察回调失败、pending 年龄和入账事务指标。
6. 回滚时关闭微信运营开关以停止新单，但服务端保留 provider 和回调路由，继续处理存量订单；禁止通过下线回调接口来回滚。

## 13. 验收标准

- 微信 Native 支付从创建订单到余额到账完整可用，回调与主动查单均可补偿。
- 支付宝功能无回归，且两种渠道可以同时启用、单独启用或全部关闭。
- 用户端只显示 `enabled + configured + verified` 且可接受新订单的渠道；配置缺失、未验证、初始化失败或新单熔断渠道不显示。
- 任何重复/并发通知不会重复入账，任何签名或金额异常不会入账。
- 用户不能同步、查询其他用户或其他租户的订单。
- 管理员能看到渠道真实状态、错误摘要、订单与回调异常，但无法读取任何明文密钥。
- 关闭渠道不影响已有订单完成回调与入账。
- 支付方式发现、管理员开关和创建订单在多实例环境中按配置版本最终收敛，创建接口始终执行最终准入校验。
- 充值页和管理员支付中心在桌面、移动端、亮色和暗色主题下与现有控制台风格一致。

## 14. 官方协议依据

- 微信支付 API v3 使用 SHA256-RSA 非对称签名，回调敏感内容使用 AES-256-GCM；新接入优先采用微信支付公钥模式。
- Native 下单接口为 `POST /v3/pay/transactions/native`，成功返回 `code_url`，由商户前端生成二维码。
- 支付成功回调需要先验证 `Wechatpay-*` 请求头签名，再使用 API v3 密钥解密资源；重复通知必须按幂等处理。
- 未支付订单在超时或取消时应调用微信关单接口；订单状态不确定时通过商户订单号主动查单。

参考：

- https://pay.wechatpay.cn/doc/v3/merchant/4012081606
- https://pay.wechatpay.cn/doc/v3/merchant/4012791877
- https://pay.wechatpay.cn/doc/v3/merchant/4013070368
- https://pay.wechatpay.cn/doc/v3/merchant/4013070356
- https://pay.wechatpay.cn/doc/v3/merchant/4012526915
