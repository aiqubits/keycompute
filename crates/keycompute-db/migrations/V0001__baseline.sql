-- KeyCompute 新库完整结构。
-- 仅用于空数据库初始化，不包含旧版本升级、数据回填或兼容迁移逻辑。

-- tenants: 租户/组织表
CREATE TABLE IF NOT EXISTS tenants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(255) NOT NULL,
    slug VARCHAR(100) NOT NULL UNIQUE,
    description TEXT,
    status VARCHAR(50) NOT NULL DEFAULT 'active',
    -- 租户配置
    default_rpm_limit INTEGER NOT NULL DEFAULT 60,
    default_tpm_limit INTEGER NOT NULL DEFAULT 100000,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_tenants_slug ON tenants(slug);
CREATE INDEX IF NOT EXISTS idx_tenants_status ON tenants(status);

-- users: 用户表
CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    email VARCHAR(255) NOT NULL UNIQUE,
    name VARCHAR(255),
    role VARCHAR(50) NOT NULL DEFAULT 'user'
        CONSTRAINT chk_users_role_allowed CHECK (role IN ('system', 'admin', 'user')),
    -- 安全事件或权限变化时递增，使此前签发的 JWT 立即失效
    token_version INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_users_tenant_id ON users(tenant_id);
CREATE INDEX IF NOT EXISTS idx_users_email ON users(email);
CREATE UNIQUE INDEX IF NOT EXISTS uq_users_single_system_role ON users (role) WHERE role = 'system';

CREATE OR REPLACE FUNCTION prevent_system_role_change()
RETURNS TRIGGER AS $$
BEGIN
    IF OLD.role = 'system' AND NEW.role <> 'system' THEN
        RAISE EXCEPTION 'system user role cannot be changed';
    END IF;

    IF OLD.role <> 'system' AND NEW.role = 'system' THEN
        RAISE EXCEPTION 'system role cannot be assigned by update';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_prevent_system_role_change ON users;
CREATE TRIGGER trg_prevent_system_role_change
BEFORE UPDATE OF role ON users
FOR EACH ROW
EXECUTE FUNCTION prevent_system_role_change();

CREATE OR REPLACE FUNCTION prevent_system_user_delete()
RETURNS TRIGGER AS $$
BEGIN
    IF OLD.role = 'system' THEN
        RAISE EXCEPTION 'system user cannot be deleted';
    END IF;

    RETURN OLD;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_prevent_system_user_delete ON users;
CREATE TRIGGER trg_prevent_system_user_delete
BEFORE DELETE ON users
FOR EACH ROW
EXECUTE FUNCTION prevent_system_user_delete();

-- produce_ai_keys: Produce AI Key 表（用户访问系统的 API Key）
CREATE TABLE IF NOT EXISTS produce_ai_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- 删除用户或租户时同步删除密钥，避免认证命中孤儿记录
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(255) NOT NULL,
    produce_ai_key_hash VARCHAR(255) NOT NULL UNIQUE,
    produce_ai_key_preview VARCHAR(20) NOT NULL,
    revoked BOOLEAN NOT NULL DEFAULT FALSE,
    revoked_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_produce_ai_keys_tenant ON produce_ai_keys(tenant_id);
CREATE INDEX IF NOT EXISTS idx_produce_ai_keys_user ON produce_ai_keys(user_id);
CREATE INDEX IF NOT EXISTS idx_produce_ai_keys_hash ON produce_ai_keys(produce_ai_key_hash);
CREATE INDEX IF NOT EXISTS idx_produce_ai_keys_revoked ON produce_ai_keys(revoked) WHERE revoked = FALSE;
-- accounts: 上游 Provider 账号池
CREATE TABLE IF NOT EXISTS accounts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL,
    provider VARCHAR(50) NOT NULL,
    name VARCHAR(255) NOT NULL,
    endpoint VARCHAR(500) NOT NULL,
    upstream_api_key_encrypted TEXT NOT NULL,
    upstream_api_key_preview VARCHAR(20) NOT NULL,
    rpm_limit INTEGER NOT NULL DEFAULT 60,
    tpm_limit INTEGER NOT NULL DEFAULT 100000,
    priority INTEGER NOT NULL DEFAULT 0,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    models_supported TEXT[] NOT NULL DEFAULT '{}',
    visibility VARCHAR(20) NOT NULL DEFAULT 'tenant',
    last_probe_at TIMESTAMPTZ,
    last_probe_latency_ms BIGINT,
    last_probe_status VARCHAR(32),
    last_probe_error_code VARCHAR(128),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT ck_accounts_probe_status
        CHECK (last_probe_status IS NULL OR last_probe_status IN ('succeeded', 'failed'))
);

CREATE INDEX IF NOT EXISTS idx_accounts_tenant_id ON accounts(tenant_id);
CREATE INDEX IF NOT EXISTS idx_accounts_provider ON accounts(provider);
CREATE INDEX IF NOT EXISTS idx_accounts_enabled ON accounts(enabled) WHERE enabled = TRUE;
CREATE INDEX IF NOT EXISTS idx_accounts_visibility ON accounts(visibility) WHERE visibility = 'global';
-- pricing_models: 模型定价表
CREATE TABLE IF NOT EXISTS pricing_models (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID,
    model_name VARCHAR(100) NOT NULL,
    billing_dimension VARCHAR(50) NOT NULL,
    currency VARCHAR(10) NOT NULL DEFAULT 'CNY',
    input_price_per_1k DECIMAL(20, 10) NOT NULL,
    output_price_per_1k DECIMAL(20, 10) NOT NULL,
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    effective_from TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    effective_until TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(tenant_id, model_name, billing_dimension)
);

CREATE INDEX IF NOT EXISTS idx_pricing_models_tenant_id ON pricing_models(tenant_id);
CREATE INDEX IF NOT EXISTS idx_pricing_models_model ON pricing_models(model_name);
CREATE INDEX IF NOT EXISTS idx_pricing_models_billing_dimension ON pricing_models(billing_dimension);
CREATE INDEX IF NOT EXISTS idx_pricing_models_default ON pricing_models(is_default) WHERE is_default = TRUE;

COMMENT ON COLUMN pricing_models.billing_dimension IS '计费维度: node 或 provideraccount';
-- usage_logs: 计费主账本，不可变
CREATE TABLE IF NOT EXISTS usage_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    request_id UUID NOT NULL UNIQUE,
    tenant_id UUID NOT NULL,
    user_id UUID NOT NULL,
    produce_ai_key_id UUID NOT NULL,
    model_name VARCHAR(100) NOT NULL,
    provider_name VARCHAR(50) NOT NULL,
    account_id UUID NOT NULL,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    total_tokens INTEGER NOT NULL,
    input_unit_price_snapshot DECIMAL(20, 10) NOT NULL,
    output_unit_price_snapshot DECIMAL(20, 10) NOT NULL,
    user_amount DECIMAL(20, 10) NOT NULL,
    currency VARCHAR(10) NOT NULL DEFAULT 'CNY',
    usage_source VARCHAR(20) NOT NULL,
    status VARCHAR(20) NOT NULL,
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_usage_logs_tenant ON usage_logs(tenant_id);
CREATE INDEX IF NOT EXISTS idx_usage_logs_user ON usage_logs(user_id);
CREATE INDEX IF NOT EXISTS idx_usage_logs_produce_ai_key ON usage_logs(produce_ai_key_id);
CREATE INDEX IF NOT EXISTS idx_usage_logs_created ON usage_logs(created_at);
CREATE INDEX IF NOT EXISTS idx_usage_logs_request ON usage_logs(request_id);

-- ============================================================================
-- Gateway 请求监控追踪
-- ============================================================================

CREATE TABLE IF NOT EXISTS gateway_requests (
    request_id UUID PRIMARY KEY,
    client_request_id VARCHAR(128),
    tenant_id UUID NOT NULL,
    user_id UUID NOT NULL,
    produce_ai_key_id UUID NOT NULL,
    protocol VARCHAR(32) NOT NULL,
    request_path VARCHAR(255) NOT NULL,
    requested_model VARCHAR(255) NOT NULL,
    is_stream BOOLEAN NOT NULL,
    route_type VARCHAR(32),
    status VARCHAR(32) NOT NULL,
    error_origin VARCHAR(32),
    error_category VARCHAR(32),
    error_code VARCHAR(128),
    received_at TIMESTAMPTZ NOT NULL,
    client_first_content_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    billing_status VARCHAR(32) NOT NULL,
    trace_quality VARCHAR(16) NOT NULL DEFAULT 'actual',
    trace_version INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT ck_gateway_requests_protocol CHECK (protocol IN ('openai', 'anthropic')),
    CONSTRAINT ck_gateway_requests_route_type CHECK (route_type IS NULL OR route_type IN ('provider_account', 'node')),
    CONSTRAINT ck_gateway_requests_status CHECK (status IN ('received', 'routing', 'queued', 'running', 'succeeded', 'failed', 'timed_out', 'cancelled')),
    CONSTRAINT ck_gateway_requests_error_origin CHECK (error_origin IS NULL OR error_origin IN ('client', 'gateway', 'upstream', 'node')),
    CONSTRAINT ck_gateway_requests_error_category CHECK (error_category IS NULL OR error_category IN ('authorization', 'invalid_request', 'balance', 'rate_limit', 'transport', 'timeout', 'upstream_4xx', 'upstream_5xx', 'protocol', 'client_disconnect', 'node_expired', 'node_failed', 'internal')),
    CONSTRAINT ck_gateway_requests_billing_status CHECK (billing_status IN ('pending', 'succeeded', 'failed', 'not_applicable')),
    CONSTRAINT ck_gateway_requests_trace_quality CHECK (trace_quality IN ('actual', 'derived', 'partial')),
    CONSTRAINT ck_gateway_requests_terminal_time CHECK ((status IN ('succeeded', 'failed', 'timed_out', 'cancelled')) = (finished_at IS NOT NULL)),
    CONSTRAINT ck_gateway_requests_first_content_time CHECK (client_first_content_at IS NULL OR client_first_content_at >= received_at),
    CONSTRAINT ck_gateway_requests_finished_time CHECK (finished_at IS NULL OR finished_at >= received_at)
);

CREATE TABLE IF NOT EXISTS gateway_request_attempts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    request_id UUID NOT NULL REFERENCES gateway_requests(request_id) ON DELETE CASCADE,
    attempt_no INTEGER NOT NULL,
    attempt_kind VARCHAR(16) NOT NULL,
    route_type VARCHAR(32) NOT NULL,
    model VARCHAR(255) NOT NULL,
    status VARCHAR(32) NOT NULL,
    is_final BOOLEAN NOT NULL DEFAULT FALSE,
    provider_name VARCHAR(64),
    account_id UUID,
    node_task_id UUID,
    node_id UUID,
    session_id UUID,
    lease_id UUID,
    upstream_request_id VARCHAR(128),
    http_status INTEGER,
    retryable BOOLEAN,
    error_origin VARCHAR(32),
    error_category VARCHAR(32),
    error_code VARCHAR(128),
    error_summary VARCHAR(512),
    started_at TIMESTAMPTZ NOT NULL,
    headers_received_at TIMESTAMPTZ,
    first_content_at TIMESTAMPTZ,
    stream_end_reason VARCHAR(32),
    stream_error_count INTEGER,
    finished_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uk_gateway_request_attempt_no UNIQUE (request_id, attempt_no),
    CONSTRAINT ck_gateway_attempt_no CHECK (attempt_no > 0),
    CONSTRAINT ck_gateway_attempt_kind CHECK (attempt_kind IN ('primary', 'fallback', 'retry', 'reclaim')),
    CONSTRAINT ck_gateway_attempt_route_type CHECK (route_type IN ('provider_account', 'node')),
    CONSTRAINT ck_gateway_attempt_status CHECK (status IN ('running', 'succeeded', 'failed', 'timed_out', 'cancelled', 'expired')),
    CONSTRAINT ck_gateway_attempt_error_origin CHECK (error_origin IS NULL OR error_origin IN ('upstream', 'gateway', 'node')),
    CONSTRAINT ck_gateway_attempt_error_category CHECK (error_category IS NULL OR error_category IN ('authorization', 'invalid_request', 'balance', 'rate_limit', 'transport', 'timeout', 'upstream_4xx', 'upstream_5xx', 'protocol', 'client_disconnect', 'node_expired', 'node_failed', 'internal')),
    CONSTRAINT ck_gateway_attempt_stream_end CHECK (stream_end_reason IS NULL OR stream_end_reason IN ('completed', 'upstream_error', 'protocol_error', 'client_disconnect', 'cancelled', 'timeout', 'truncated')),
    CONSTRAINT ck_gateway_attempt_stream_errors CHECK (stream_error_count IS NULL OR stream_error_count >= 0),
    CONSTRAINT ck_gateway_attempt_headers_time CHECK (headers_received_at IS NULL OR headers_received_at >= started_at),
    CONSTRAINT ck_gateway_attempt_first_time CHECK (first_content_at IS NULL OR first_content_at >= started_at),
    CONSTRAINT ck_gateway_attempt_finished_time CHECK (finished_at IS NULL OR finished_at >= started_at),
    CONSTRAINT ck_gateway_attempt_content_finished CHECK (first_content_at IS NULL OR finished_at IS NULL OR first_content_at <= finished_at),
    CONSTRAINT ck_gateway_attempt_terminal_time CHECK ((status = 'running') = (finished_at IS NULL)),
    CONSTRAINT ck_gateway_attempt_target CHECK (
        (route_type = 'provider_account' AND provider_name IS NOT NULL AND account_id IS NOT NULL AND node_task_id IS NULL AND node_id IS NULL AND session_id IS NULL AND lease_id IS NULL)
        OR
        (route_type = 'node' AND provider_name IS NULL AND account_id IS NULL AND node_task_id IS NOT NULL AND node_id IS NOT NULL AND session_id IS NOT NULL AND lease_id IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS idx_gateway_requests_received ON gateway_requests(received_at DESC, request_id DESC);
CREATE INDEX IF NOT EXISTS idx_gateway_requests_status_received ON gateway_requests(status, received_at DESC);
CREATE INDEX IF NOT EXISTS idx_gateway_requests_route_received ON gateway_requests(route_type, received_at DESC);
CREATE INDEX IF NOT EXISTS idx_gateway_requests_tenant_received ON gateway_requests(tenant_id, received_at DESC);
CREATE INDEX IF NOT EXISTS idx_gateway_requests_user_received ON gateway_requests(user_id, received_at DESC);
CREATE INDEX IF NOT EXISTS idx_gateway_requests_key_received ON gateway_requests(produce_ai_key_id, received_at DESC);
CREATE INDEX IF NOT EXISTS idx_gateway_requests_model_received ON gateway_requests(requested_model, received_at DESC);
CREATE INDEX IF NOT EXISTS idx_gateway_requests_client_id ON gateway_requests(client_request_id) WHERE client_request_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_gateway_requests_pending_billing_finished ON gateway_requests(finished_at)
    WHERE billing_status = 'pending' AND finished_at IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS uk_gateway_request_final_attempt ON gateway_request_attempts(request_id) WHERE is_final;
CREATE UNIQUE INDEX IF NOT EXISTS uk_gateway_node_attempt_lease ON gateway_request_attempts(node_task_id, lease_id) WHERE route_type = 'node';
CREATE INDEX IF NOT EXISTS idx_gateway_attempt_account_started ON gateway_request_attempts(account_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_gateway_attempt_provider_started ON gateway_request_attempts(provider_name, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_gateway_attempt_node_started ON gateway_request_attempts(node_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_gateway_attempt_upstream_id ON gateway_request_attempts(upstream_request_id) WHERE upstream_request_id IS NOT NULL;

-- distribution_records: 二级分销记录
CREATE TABLE IF NOT EXISTS distribution_records (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    usage_log_id UUID NOT NULL REFERENCES usage_logs(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL,
    beneficiary_id UUID NOT NULL,
    share_amount DECIMAL(20, 10) NOT NULL,
    share_ratio DECIMAL(5, 4) NOT NULL,
    -- 分销层级；参与唯一约束以提供写入幂等性
    level VARCHAR(20) NOT NULL DEFAULT 'level1',
    status VARCHAR(20) NOT NULL DEFAULT 'pending',
    settled_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uk_distribution_records_unique
        UNIQUE (usage_log_id, beneficiary_id, level)
);

CREATE INDEX IF NOT EXISTS idx_distribution_records_tenant_id ON distribution_records(tenant_id);
CREATE INDEX IF NOT EXISTS idx_distribution_records_usage_log_id ON distribution_records(usage_log_id);
CREATE INDEX IF NOT EXISTS idx_distribution_records_beneficiary_id ON distribution_records(beneficiary_id);
CREATE INDEX IF NOT EXISTS idx_distribution_records_status ON distribution_records(status);
CREATE INDEX IF NOT EXISTS idx_distribution_records_level ON distribution_records(level);
COMMENT ON CONSTRAINT uk_distribution_records_unique ON distribution_records IS
'幂等性保护：防止同一 usage_log 对同一受益人的重复分销记录';
-- tenant_distribution_rules: 租户分销规则
CREATE TABLE IF NOT EXISTS tenant_distribution_rules (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL,
    beneficiary_id UUID NOT NULL,
    name VARCHAR(255) NOT NULL DEFAULT '默认分销规则',
    description TEXT,
    commission_rate DECIMAL(5, 4) NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    effective_from TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    effective_until TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(tenant_id, beneficiary_id, effective_from)
);

CREATE INDEX IF NOT EXISTS idx_tenant_distribution_rules_tenant ON tenant_distribution_rules(tenant_id);
CREATE INDEX IF NOT EXISTS idx_tenant_distribution_rules_active ON tenant_distribution_rules(is_active) WHERE is_active = TRUE;
-- pending_registrations: 待完成注册表
-- 用于邮箱验证码注册流程，在验证码验证成功前暂存注册占位状态

CREATE TABLE IF NOT EXISTS pending_registrations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email VARCHAR(255) NOT NULL UNIQUE,
    -- 首次触达时锁定的推荐码（可选）
    referral_code UUID REFERENCES users(id) ON DELETE SET NULL,
    -- Argon2 哈希后的 6 位验证码
    verification_code_hash VARCHAR(255) NOT NULL,
    -- 验证码过期时间（默认 10 分钟）
    expires_at TIMESTAMPTZ NOT NULL,
    -- 已尝试验证次数
    verify_attempts INTEGER NOT NULL DEFAULT 0,
    -- 验证码发送次数
    resend_count INTEGER NOT NULL DEFAULT 1,
    -- 最近一次发送时间
    last_sent_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- 发起请求的客户端 IP（可选）
    requested_from_ip TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_pending_registrations_email ON pending_registrations(email);
CREATE INDEX IF NOT EXISTS idx_pending_registrations_expires ON pending_registrations(expires_at);
CREATE INDEX IF NOT EXISTS idx_pending_registrations_referral_code ON pending_registrations(referral_code);
-- user_credentials: 用户密码凭证表
-- 存储用户密码哈希和登录安全相关信息

CREATE TABLE IF NOT EXISTS user_credentials (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 密码哈希 (argon2id)
    password_hash VARCHAR(255) NOT NULL,
    -- 邮箱验证状态
    email_verified BOOLEAN NOT NULL DEFAULT FALSE,
    email_verified_at TIMESTAMPTZ,
    -- 登录失败计数（用于防护暴力破解）
    failed_login_attempts INTEGER NOT NULL DEFAULT 0,
    locked_until TIMESTAMPTZ,
    -- 最后登录信息
    last_login_at TIMESTAMPTZ,
    last_login_ip TEXT,
    -- 时间戳
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- 唯一约束：一个用户只有一个凭证记录
    UNIQUE(user_id)
);

-- 索引
CREATE INDEX IF NOT EXISTS idx_user_credentials_user ON user_credentials(user_id);
CREATE INDEX IF NOT EXISTS idx_user_credentials_locked ON user_credentials(locked_until) 
    WHERE locked_until IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_user_credentials_verified ON user_credentials(email_verified) 
    WHERE email_verified = FALSE;
-- password_resets: 密码重置令牌表
-- 管理用户密码重置流程

CREATE TABLE IF NOT EXISTS password_resets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 重置令牌
    token VARCHAR(255) NOT NULL UNIQUE,
    -- 令牌过期时间（短时效，如 1 小时）
    expires_at TIMESTAMPTZ NOT NULL,
    -- 是否已使用
    used BOOLEAN NOT NULL DEFAULT FALSE,
    used_at TIMESTAMPTZ,
    -- 请求来源 IP
    requested_from_ip INET,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 索引
CREATE INDEX IF NOT EXISTS idx_password_resets_token ON password_resets(token);
CREATE INDEX IF NOT EXISTS idx_password_resets_expires ON password_resets(expires_at) 
    WHERE used = FALSE;
CREATE INDEX IF NOT EXISTS idx_password_resets_user ON password_resets(user_id);
-- user_referrals: 用户推荐关系表
-- 用于存储谁推荐了谁，支持二级分销

CREATE TABLE IF NOT EXISTS user_referrals (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- 被推荐人（新用户）
    user_id UUID NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    -- 一级推荐人
    level1_referrer_id UUID REFERENCES users(id) ON DELETE SET NULL,
    -- 二级推荐人（推荐人的推荐人）
    level2_referrer_id UUID REFERENCES users(id) ON DELETE SET NULL,
    -- 推荐来源（可选，如推荐码、链接等）
    source VARCHAR(255),
    -- 推荐状态: pending, active, expired
    status VARCHAR(50) NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 索引
CREATE INDEX IF NOT EXISTS idx_user_referrals_user ON user_referrals(user_id);
CREATE INDEX IF NOT EXISTS idx_user_referrals_level1 ON user_referrals(level1_referrer_id);
CREATE INDEX IF NOT EXISTS idx_user_referrals_level2 ON user_referrals(level2_referrer_id);
CREATE INDEX IF NOT EXISTS idx_user_referrals_status ON user_referrals(status);
-- 支付订单表
-- 用于存储用户充值订单记录

CREATE TABLE IF NOT EXISTS payment_orders (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- 租户ID
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    -- 用户ID
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 商户订单号（外部订单号）
    out_trade_no VARCHAR(64) NOT NULL UNIQUE,
    -- 通用渠道交易号展示字段
    trade_no VARCHAR(64),
    -- 支付渠道交易号
    provider_trade_no VARCHAR(64),
    -- 订单金额（单位：元）
    amount DECIMAL(12, 2) NOT NULL,
    -- 币种（默认CNY）
    currency VARCHAR(8) NOT NULL DEFAULT 'CNY',
    -- 订单状态: pending/paid/failed/closed
    status VARCHAR(20) NOT NULL DEFAULT 'pending',
    -- 支付方式: alipay/wechatpay
    payment_method VARCHAR(20) NOT NULL DEFAULT 'alipay',
    -- 支付场景: page/wap/qr/native
    payment_scene VARCHAR(20) NOT NULL DEFAULT 'page',
    -- 商品标题
    subject VARCHAR(256) NOT NULL,
    -- 商品描述
    body TEXT,
    -- 支付时间
    paid_at TIMESTAMPTZ,
    -- 关闭时间
    closed_at TIMESTAMPTZ,
    -- 过期时间
    expired_at TIMESTAMPTZ NOT NULL,
    -- 支付URL（用于前端跳转）
    pay_url TEXT,
    -- 回调通知原始数据
    notify_data JSONB,
    -- 支付渠道返回的非敏感展示数据
    provider_payload JSONB,
    -- 最近一次渠道错误码
    last_error_code VARCHAR(64),
    -- 最近一次渠道错误信息
    last_error_message TEXT,
    -- 最近一次主动同步时间
    last_synced_at TIMESTAMPTZ,
    -- 备注信息
    remarks TEXT,
    -- 创建时间
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- 更新时间
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 创建索引
CREATE INDEX IF NOT EXISTS idx_payment_orders_tenant_id ON payment_orders(tenant_id);
CREATE INDEX IF NOT EXISTS idx_payment_orders_user_id ON payment_orders(user_id);
CREATE INDEX IF NOT EXISTS idx_payment_orders_out_trade_no ON payment_orders(out_trade_no);
CREATE INDEX IF NOT EXISTS idx_payment_orders_trade_no ON payment_orders(trade_no);
CREATE INDEX IF NOT EXISTS idx_payment_orders_status ON payment_orders(status);
CREATE INDEX IF NOT EXISTS idx_payment_orders_created_at ON payment_orders(created_at);
CREATE UNIQUE INDEX IF NOT EXISTS uk_payment_orders_provider_trade_no
    ON payment_orders(payment_method, provider_trade_no)
    WHERE provider_trade_no IS NOT NULL;

-- 添加注释
COMMENT ON TABLE payment_orders IS '支付订单表';
COMMENT ON COLUMN payment_orders.id IS '订单ID';
COMMENT ON COLUMN payment_orders.tenant_id IS '租户ID';
COMMENT ON COLUMN payment_orders.user_id IS '用户ID';
COMMENT ON COLUMN payment_orders.out_trade_no IS '商户订单号（外部订单号）';
COMMENT ON COLUMN payment_orders.trade_no IS '通用渠道交易号展示字段';
COMMENT ON COLUMN payment_orders.provider_trade_no IS '支付渠道交易号';
COMMENT ON COLUMN payment_orders.amount IS '订单金额（单位：元）';
COMMENT ON COLUMN payment_orders.currency IS '币种';
COMMENT ON COLUMN payment_orders.status IS '订单状态: pending/paid/failed/closed';
COMMENT ON COLUMN payment_orders.payment_method IS '支付方式';
COMMENT ON COLUMN payment_orders.payment_scene IS '支付场景: page/wap/qr/native';
COMMENT ON COLUMN payment_orders.subject IS '商品标题';
COMMENT ON COLUMN payment_orders.body IS '商品描述';
COMMENT ON COLUMN payment_orders.paid_at IS '支付时间';
COMMENT ON COLUMN payment_orders.closed_at IS '关闭时间';
COMMENT ON COLUMN payment_orders.expired_at IS '过期时间';
COMMENT ON COLUMN payment_orders.pay_url IS '支付URL';
COMMENT ON COLUMN payment_orders.notify_data IS '回调通知原始数据';
COMMENT ON COLUMN payment_orders.provider_payload IS '支付渠道返回的非敏感展示数据';
COMMENT ON COLUMN payment_orders.last_error_code IS '最近一次渠道错误码';
COMMENT ON COLUMN payment_orders.last_error_message IS '最近一次渠道错误信息';
COMMENT ON COLUMN payment_orders.last_synced_at IS '最近一次主动同步时间';
COMMENT ON COLUMN payment_orders.remarks IS '备注信息';

-- 支付通知去重与处理状态
CREATE TABLE IF NOT EXISTS payment_notifications (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    payment_method VARCHAR(20) NOT NULL,
    provider_event_id VARCHAR(128) NOT NULL,
    order_id UUID REFERENCES payment_orders(id) ON DELETE SET NULL,
    out_trade_no VARCHAR(64) NOT NULL,
    processing_status VARCHAR(24) NOT NULL DEFAULT 'received',
    payload_digest VARCHAR(64) NOT NULL,
    failure_reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    processed_at TIMESTAMPTZ,
    UNIQUE(payment_method, provider_event_id)
);

CREATE INDEX IF NOT EXISTS idx_payment_notifications_order
    ON payment_notifications(order_id);
CREATE INDEX IF NOT EXISTS idx_payment_notifications_status
    ON payment_notifications(processing_status, created_at DESC);

-- 支付回调安全拒绝事件
CREATE TABLE IF NOT EXISTS payment_security_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    payment_method VARCHAR(20) NOT NULL,
    event_type VARCHAR(40) NOT NULL,
    request_id VARCHAR(128),
    source_ip VARCHAR(64),
    payload_digest VARCHAR(64) NOT NULL,
    detail TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_payment_security_events_created
    ON payment_security_events(created_at DESC);

-- 支付渠道配置验证与集群运行状态
CREATE TABLE IF NOT EXISTS payment_provider_states (
    payment_method VARCHAR(20) PRIMARY KEY,
    config_version BIGINT NOT NULL DEFAULT 1,
    config_fingerprint VARCHAR(64),
    verified_config_fingerprint VARCHAR(64),
    verified_at TIMESTAMPTZ,
    circuit_state VARCHAR(20) NOT NULL DEFAULT 'available',
    last_error_code VARCHAR(64),
    last_error_message TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT chk_payment_provider_states_circuit
        CHECK (circuit_state IN ('available', 'degraded', 'unavailable'))
);

INSERT INTO payment_provider_states(payment_method)
VALUES ('alipay'), ('wechatpay')
ON CONFLICT (payment_method) DO NOTHING;

-- 用户余额表
CREATE TABLE IF NOT EXISTS user_balances (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- 租户ID
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    -- 用户ID
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE UNIQUE,
    -- 可用余额（单位：元，10 位小数与计费精度对齐）
    available_balance DECIMAL(20, 10) NOT NULL DEFAULT 0,
    -- 冻结余额（单位：元）
    frozen_balance DECIMAL(20, 10) NOT NULL DEFAULT 0,
    -- 累计充值金额
    total_recharged DECIMAL(20, 10) NOT NULL DEFAULT 0,
    -- 累计消费金额
    total_consumed DECIMAL(20, 10) NOT NULL DEFAULT 0,
    -- 创建时间
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- 更新时间
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 创建索引
CREATE INDEX IF NOT EXISTS idx_user_balances_tenant_id ON user_balances(tenant_id);
CREATE INDEX IF NOT EXISTS idx_user_balances_user_id ON user_balances(user_id);

-- 添加注释
COMMENT ON TABLE user_balances IS '用户余额表';
COMMENT ON COLUMN user_balances.available_balance IS '可用余额（单位：元）';
COMMENT ON COLUMN user_balances.frozen_balance IS '冻结余额（单位：元）';
COMMENT ON COLUMN user_balances.total_recharged IS '累计充值金额';
COMMENT ON COLUMN user_balances.total_consumed IS '累计消费金额';

-- 余额变动记录表
CREATE TABLE IF NOT EXISTS balance_transactions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- 租户ID
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    -- 用户ID
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 关联订单ID（可选）
    order_id UUID REFERENCES payment_orders(id),
    -- 关联使用日志ID（可选）
    usage_log_id UUID REFERENCES usage_logs(id),
    -- 交易类型: recharge/consume/freeze/unfreeze
    transaction_type VARCHAR(20) NOT NULL,
    -- 变动金额（正数为增加，负数为减少，10 位小数与计费精度对齐）
    amount DECIMAL(20, 10) NOT NULL,
    -- 变动前余额
    balance_before DECIMAL(20, 10) NOT NULL,
    -- 变动后余额
    balance_after DECIMAL(20, 10) NOT NULL,
    -- 币种
    currency VARCHAR(8) NOT NULL DEFAULT 'CNY',
    -- 备注
    description TEXT,
    -- 创建时间
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 创建索引
CREATE INDEX IF NOT EXISTS idx_balance_transactions_tenant_id ON balance_transactions(tenant_id);
CREATE INDEX IF NOT EXISTS idx_balance_transactions_user_id ON balance_transactions(user_id);
CREATE INDEX IF NOT EXISTS idx_balance_transactions_order_id ON balance_transactions(order_id);
CREATE INDEX IF NOT EXISTS idx_balance_transactions_usage_log_id ON balance_transactions(usage_log_id);
CREATE INDEX IF NOT EXISTS idx_balance_transactions_type ON balance_transactions(transaction_type);
CREATE INDEX IF NOT EXISTS idx_balance_transactions_created_at ON balance_transactions(created_at);
-- 同一支付订单只能产生一笔充值流水
CREATE UNIQUE INDEX IF NOT EXISTS uk_balance_transactions_recharge_order
    ON balance_transactions(order_id)
    WHERE transaction_type = 'recharge' AND order_id IS NOT NULL;

-- 添加注释
COMMENT ON TABLE balance_transactions IS '余额变动记录表';
COMMENT ON COLUMN balance_transactions.transaction_type IS '交易类型: recharge/consume/freeze/unfreeze/tip_credit';
COMMENT ON COLUMN balance_transactions.amount IS '变动金额（正数为增加，负数为减少）';
COMMENT ON COLUMN balance_transactions.balance_before IS '变动前余额';
COMMENT ON COLUMN balance_transactions.balance_after IS '变动后余额';
-- 系统设置表
-- 存储全局系统配置，支持运行时修改

CREATE TABLE IF NOT EXISTS system_settings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- 设置键名（唯一）
    key VARCHAR(100) UNIQUE NOT NULL,
    -- 设置值（以字符串形式存储）
    value TEXT NOT NULL
        CONSTRAINT chk_system_settings_default_user_role
        CHECK (key <> 'default_user_role' OR value = 'user'),
    -- 值类型：string, bool, int, decimal, json
    value_type VARCHAR(20) NOT NULL DEFAULT 'string',
    -- 设置描述
    description VARCHAR(255),
    -- 是否为敏感设置（敏感设置不在日志中显示）
    is_sensitive BOOLEAN DEFAULT FALSE,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

-- 创建索引
-- key 上有 UNIQUE 约束，PG 自动创建唯一索引，无需额外 B-tree 索引

-- 插入默认系统设置
INSERT INTO system_settings (key, value, value_type, description) VALUES
    -- 站点设置
    ('site_name', 'KeyCompute', 'string', '站点名称'),
    ('site_description', 'Next-generation high-performance AI token compute service platform', 'string', '站点描述'),
    ('site_logo_url', '', 'string', '站点 Logo URL'),
    ('site_favicon_url', '', 'string', '站点 Favicon URL'),
    
    -- 注册设置
    ('default_user_quota', '10.00', 'decimal', '新用户默认配额（元）'),
    ('default_user_role', 'user', 'string', '新用户默认角色'),
    
    -- 限流设置
    ('default_rpm_limit', '60', 'int', '默认 RPM 限制'),
    ('default_tpm_limit', '100000', 'int', '默认 TPM 限制'),
    
    -- 系统状态
    ('maintenance_mode', 'false', 'bool', '维护模式（开启后禁止所有 API 访问）'),
    ('maintenance_message', '', 'string', '维护模式提示信息'),
    
    -- 分销设置
    ('distribution_enabled', 'true', 'bool', '是否启用分销系统'),
    ('distribution_level1_default_ratio', '0.03', 'decimal', '一级分销默认分成比例'),
    ('distribution_level2_default_ratio', '0.02', 'decimal', '二级分销默认分成比例'),
    ('distribution_min_withdraw', '10.00', 'decimal', '最低提现金额'),
    
    -- 支付设置
    ('alipay_enabled', 'false', 'bool', '是否启用支付宝支付'),
    ('wechatpay_enabled', 'false', 'bool', '是否启用微信支付'),
    ('min_recharge_amount', '1.00', 'decimal', '最小充值金额'),
    ('max_recharge_amount', '100000.00', 'decimal', '最大充值金额'),
    
    -- 安全设置
    ('login_failed_limit', '5', 'int', '登录失败次数限制'),
    ('login_lockout_minutes', '30', 'int', '登录锁定时长（分钟）'),
    -- 密码策略使用硬编码，参见 keycompute-auth/src/password/validator.rs
    -- ('password_min_length', '8', 'int', '密码最小长度'),
    -- ('password_require_uppercase', 'true', 'bool', '密码是否需要大写字母'),
    -- ('password_require_lowercase', 'true', 'bool', '密码是否需要小写字母'),
    -- ('password_require_number', 'true', 'bool', '密码是否需要数字'),
    -- ('password_require_special', 'false', 'bool', '密码是否需要特殊字符'),
    
    -- 公告设置
    ('system_notice', '', 'string', '系统公告内容'),
    ('system_notice_enabled', 'false', 'bool', '是否显示系统公告'),
    
    -- 节点租赁小费设置
    ('node_tip_ratio', '0.90', 'decimal', '节点租赁小费比例（计费金额 * 比例 = 小费）'),
    
    -- 其他设置
    ('footer_content', '', 'string', '页脚自定义内容'),
    ('about_content', '', 'string', '关于页面内容'),
    ('terms_of_service_url', '', 'string', '服务条款 URL'),
    ('privacy_policy_url', '', 'string', '隐私政策 URL')
ON CONFLICT (key) DO NOTHING;

-- 创建更新时间触发器
CREATE OR REPLACE FUNCTION update_system_settings_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trigger_update_system_settings_updated_at ON system_settings;
CREATE TRIGGER trigger_update_system_settings_updated_at
    BEFORE UPDATE ON system_settings
    FOR EACH ROW
    EXECUTE FUNCTION update_system_settings_updated_at();

-- ============================================================================
-- Node Gateway 节点相关表 (MVP)
-- ============================================================================

-- nodes: 节点注册信息表
CREATE TABLE IF NOT EXISTS nodes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_user_id UUID NOT NULL,
    client_instance_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    status TEXT NOT NULL,
    capabilities_json JSONB NOT NULL,
    consecutive_failure_count INTEGER NOT NULL DEFAULT 0,
    failure_threshold INTEGER NOT NULL DEFAULT 3,
    last_heartbeat_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (owner_user_id, client_instance_id)
);

CREATE INDEX IF NOT EXISTS idx_nodes_status ON nodes(status);
CREATE INDEX IF NOT EXISTS idx_nodes_last_heartbeat_at ON nodes(last_heartbeat_at);

-- node_sessions: 节点会话管理表
CREATE TABLE IF NOT EXISTS node_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    session_token_hash TEXT NOT NULL UNIQUE,
    accepted_models_json JSONB NOT NULL DEFAULT '[]'::jsonb,
    issued_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_node_sessions_node_id_expires_at ON node_sessions(node_id, expires_at);
CREATE INDEX IF NOT EXISTS idx_node_sessions_accepted_models ON node_sessions USING GIN (accepted_models_json);

-- node_tasks: 节点任务生命周期表
CREATE TABLE IF NOT EXISTS node_tasks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    request_id UUID NOT NULL UNIQUE,
    user_id UUID NOT NULL,
    model TEXT NOT NULL,
    payload_json JSONB NOT NULL,
    status TEXT NOT NULL,
    assigned_node_id UUID REFERENCES nodes(id) ON DELETE SET NULL,
    assigned_session_id UUID REFERENCES node_sessions(id) ON DELETE SET NULL,
    lease_id UUID,
    failure_count INTEGER NOT NULL DEFAULT 0,
    failure_threshold INTEGER NOT NULL DEFAULT 3,
    result_json JSONB,
    error_json JSONB,
    queued_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    claimed_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    deadline_at TIMESTAMPTZ NOT NULL,
    complete_grace_until TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_node_tasks_status_model_deadline ON node_tasks(status, model, deadline_at);
CREATE INDEX IF NOT EXISTS idx_node_tasks_assigned_node_status ON node_tasks(assigned_node_id, status);
CREATE INDEX IF NOT EXISTS idx_node_tasks_assigned_session_lease ON node_tasks(assigned_session_id, lease_id);

-- node_task_submissions: 节点任务提交结果表 (幂等控制)
CREATE TABLE IF NOT EXISTS node_task_submissions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    task_id UUID NOT NULL REFERENCES node_tasks(id) ON DELETE CASCADE,
    lease_id UUID NOT NULL,
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    session_id UUID NOT NULL REFERENCES node_sessions(id) ON DELETE CASCADE,
    result_kind TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    action TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (task_id, lease_id)
);

CREATE INDEX IF NOT EXISTS idx_node_task_submissions_task_lease ON node_task_submissions(task_id, lease_id);

-- ============================================================================
-- 管理端监控查询性能优化索引
-- ============================================================================

-- node_tasks 监控追踪查询优化索引
-- 用于 admin_monitoring.rs 中的 traces 查询（ORDER BY created_at DESC LIMIT 50）
CREATE INDEX IF NOT EXISTS idx_node_tasks_created_at_desc ON node_tasks(created_at DESC);

-- node_tasks 完成时间统计优化索引（部分索引）
-- 用于 admin_monitoring.rs 中的 avg_node_latency_ms 统计（WHERE finished_at IS NOT NULL）
CREATE INDEX IF NOT EXISTS idx_node_tasks_finished_at ON node_tasks(finished_at) WHERE finished_at IS NOT NULL;

-- node_task_submissions 监控查询优化索引
-- 用于 admin_monitoring.rs 中的 LEFT JOIN LATERAL 子查询（WHERE task_id = nt.id ORDER BY created_at DESC）
CREATE INDEX IF NOT EXISTS idx_node_task_submissions_task_id_created_at ON node_task_submissions(task_id, created_at DESC);

-- node_sessions 监控查询优化索引
-- 用于 admin_monitoring.rs 和 admin_node_gateway.rs 中的 LEFT JOIN LATERAL 子查询
-- （WHERE node_id = n.id ORDER BY last_seen_at DESC LIMIT 1）
CREATE INDEX IF NOT EXISTS idx_node_sessions_node_id_last_seen_at ON node_sessions(node_id, last_seen_at DESC);

-- ============================================================================
-- user_node_gateway_tokens: 用户节点网关注册令牌表
--
-- 审批流程：
--   1. 用户申请 → status='pending'
--   2. Admin 审批 → status='approved'，token 可被 GET 返回明文（始终可重建）
--   3. 用户注册节点 → status='consumed'（一次性使用）
--
-- Token 格式: kcng-{token_id}-{signature}
--   - token_id = UUID v4 去连字符（同时也是本表的 id）
--   - signature = HMAC-SHA256(secret, token_id) 后 32 个十六进制字符（取 HMAC 后 16 字节）
-- ============================================================================

CREATE TABLE IF NOT EXISTS user_node_gateway_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- token 的 SHA-256 hash（冗余存储，用于额外校验）
    token_hash TEXT NOT NULL UNIQUE,
    -- token 预览（前 16 位，用于 UI 展示，例如 "kcng-a1b2c3d4e5f6"）
    token_preview TEXT NOT NULL,
    -- 状态：pending(待审批) / approved(已审批) / rejected(已拒绝) / consumed(已使用)
    status TEXT NOT NULL DEFAULT 'pending',
    -- token 是否已被用户查看过明文（标记已查看，用于安全提醒）
    is_revealed BOOLEAN NOT NULL DEFAULT FALSE,
    -- 审批人 ID
    approved_by UUID REFERENCES users(id),
    -- 管理员操作时间（审批通过/拒绝时均会更新）
    actioned_at TIMESTAMPTZ,
    -- 消费时间（节点注册时设置）
    consumed_at TIMESTAMPTZ,
    -- 消费该 token 注册的节点 ID
    consumed_node_id UUID REFERENCES nodes(id) ON DELETE SET NULL,
    -- 吊销原因（Admin 吊销令牌时填写）
    revoke_reason TEXT,
    -- 签发时间（用户申请时间）
    issued_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 索引
CREATE INDEX IF NOT EXISTS idx_user_node_gateway_tokens_user_id ON user_node_gateway_tokens(user_id);
-- 待审批列表查询
CREATE INDEX IF NOT EXISTS idx_user_node_gateway_tokens_pending ON user_node_gateway_tokens(status) WHERE status = 'pending';
-- 已审批且未被消费的 token 查询（注册时使用）
CREATE INDEX IF NOT EXISTS idx_user_node_gateway_tokens_approved ON user_node_gateway_tokens(status) WHERE status = 'approved';
-- token_hash 上有 UNIQUE 约束，已自动创建唯一索引，无需额外 B-tree 索引
-- 确保每用户同一时间仅有一个活跃 token（pending 或 approved），防止并发 POST 创建多个
CREATE UNIQUE INDEX IF NOT EXISTS idx_user_node_gateway_tokens_one_active ON user_node_gateway_tokens(user_id) WHERE status IN ('pending', 'approved');

-- 注释
COMMENT ON TABLE user_node_gateway_tokens IS '用户节点网关注册令牌表（审批制 + HMAC 签名 + 一次性使用）';
COMMENT ON COLUMN user_node_gateway_tokens.id IS '令牌记录 ID（同时也是 token 中的 token_id）';
COMMENT ON COLUMN user_node_gateway_tokens.user_id IS '所属用户 ID';
COMMENT ON COLUMN user_node_gateway_tokens.token_hash IS '令牌 SHA-256 hash';
COMMENT ON COLUMN user_node_gateway_tokens.token_preview IS '令牌预览（前 16 位）';
COMMENT ON COLUMN user_node_gateway_tokens.status IS '状态：pending/approved/rejected/consumed';
COMMENT ON COLUMN user_node_gateway_tokens.is_revealed IS 'token 明文是否已被用户查看';
COMMENT ON COLUMN user_node_gateway_tokens.approved_by IS '审批人 ID';
COMMENT ON COLUMN user_node_gateway_tokens.actioned_at IS '管理员操作时间（审批通过/拒绝时均会更新）';
COMMENT ON COLUMN user_node_gateway_tokens.consumed_at IS 'token 使用时间';
COMMENT ON COLUMN user_node_gateway_tokens.consumed_node_id IS '使用 token 注册的节点 ID';
COMMENT ON COLUMN user_node_gateway_tokens.issued_at IS '签发时间（用户申请时间）';
COMMENT ON COLUMN user_node_gateway_tokens.updated_at IS '最后更新时间';

-- ============================================================================
-- node_tips: 节点租赁小费表
--
-- 当用户通过 node gateway 发起会话并完成计费后，节点提供者（owner）获得小费
-- tips = usage_log.user_amount * node_tip_ratio
-- ============================================================================

CREATE TABLE IF NOT EXISTS node_tips (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- 关联的计费记录
    usage_log_id UUID NOT NULL
        CONSTRAINT uk_node_tips_usage_log_id UNIQUE
        REFERENCES usage_logs(id) ON DELETE CASCADE,
    -- 提供服务的节点 ID
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    -- 节点所有者（同时也是 tips 受益人）
    owner_user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 消费该服务的用户（付费方）
    consumer_user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 小费金额（10 位小数，与 usage_logs.user_amount DECIMAL(20,10) 对齐）
    tip_amount DECIMAL(20, 10) NOT NULL,
    -- 币种
    currency VARCHAR(8) NOT NULL DEFAULT 'CNY',
    -- 计算比例（快照，如 0.9000）
    tip_ratio DECIMAL(5, 4) NOT NULL,
    -- 原始计费金额（快照，审计用，10 位小数与 usage_logs.user_amount DECIMAL(20,10) 对齐）
    bill_amount DECIMAL(20, 10) NOT NULL,
    -- 创建时间
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- 最后更新时间
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_node_tips_owner_user_id ON node_tips(owner_user_id);
-- usage_log_id 上有 UNIQUE 约束，已自动创建唯一索引，无需额外 B-tree 索引
-- 按用户查询历史记录的复合索引（覆盖 list_by_user 的 ORDER BY created_at DESC）
CREATE INDEX IF NOT EXISTS idx_node_tips_owner_created ON node_tips(owner_user_id, created_at DESC);
COMMENT ON TABLE node_tips IS '节点租赁小费表';
COMMENT ON COLUMN node_tips.usage_log_id IS '关联的计费记录 ID';
COMMENT ON COLUMN node_tips.node_id IS '提供服务的节点 ID';
COMMENT ON COLUMN node_tips.owner_user_id IS '节点所有者（tips 受益人）';
COMMENT ON COLUMN node_tips.consumer_user_id IS '消费用户（付费方）';
COMMENT ON COLUMN node_tips.tip_amount IS '小费金额（元）';
COMMENT ON COLUMN node_tips.tip_ratio IS '计算比例（快照）';
COMMENT ON COLUMN node_tips.bill_amount IS '原始计费金额（快照，审计用）';

-- ============================================================================
-- node_tip_withdrawals: 小费提现记录表
--
-- 支持两种提现方式：
--   1. alipay  - 用户提供支付宝账户+姓名，管理员线下打款
--   2. balance - 直接转入用户 available_balance
--
-- PII 敏感信息加密存储：
--   - alipay_account 和 real_name 使用 AES-256-GCM 加密
--   - 加密格式：base64(nonce || ciphertext)
--   - 密钥复用 CRYPTO__SECRET_KEY 配置
-- ============================================================================

CREATE TABLE IF NOT EXISTS node_tip_withdrawals (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- 申请人
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 提现方式：alipay / balance
    withdrawal_type VARCHAR(20) NOT NULL
        CONSTRAINT chk_node_tip_withdrawals_type CHECK (withdrawal_type IN ('alipay', 'balance')),
    -- 提现总额（10 位小数与计费精度对齐）
    total_amount DECIMAL(20, 10) NOT NULL,
    -- 币种
    currency VARCHAR(8) NOT NULL DEFAULT 'CNY',
    -- 加密的支付宝账号（仅 alipay 方式）
    -- 格式：base64(nonce || ciphertext)，使用 AES-256-GCM 加密
    -- 密钥复用 CRYPTO__SECRET_KEY 配置
    encrypted_alipay_account TEXT,
    -- 加密的真实姓名（仅 alipay 方式）
    -- 格式：base64(nonce || ciphertext)，使用 AES-256-GCM 加密
    -- 密钥复用 CRYPTO__SECRET_KEY 配置
    encrypted_real_name TEXT,
    -- 状态：pending / approved / completed / rejected
    status VARCHAR(20) NOT NULL DEFAULT 'pending'
        CONSTRAINT chk_node_tip_withdrawals_status CHECK (status IN ('pending', 'approved', 'completed', 'rejected')),
    -- 处理该提现的管理员
    admin_id UUID REFERENCES users(id),
    -- 管理员备注
    admin_remark TEXT,
    -- 管理员操作时间
    actioned_at TIMESTAMPTZ,
    -- 创建时间
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- 更新时间
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_node_tip_withdrawals_user_id ON node_tip_withdrawals(user_id);
CREATE INDEX IF NOT EXISTS idx_node_tip_withdrawals_status ON node_tip_withdrawals(status);
-- 待审批提现列表查询优化
CREATE INDEX IF NOT EXISTS idx_node_tip_withdrawals_pending ON node_tip_withdrawals(status) WHERE status = 'pending';

COMMENT ON TABLE node_tip_withdrawals IS '小费提现记录表';
COMMENT ON COLUMN node_tip_withdrawals.withdrawal_type IS '提现方式：alipay / balance';
COMMENT ON COLUMN node_tip_withdrawals.encrypted_alipay_account IS '加密的支付宝账号（仅 alipay 方式，AES-256-GCM 加密，格式：base64(nonce || ciphertext)）';
COMMENT ON COLUMN node_tip_withdrawals.encrypted_real_name IS '加密的真实姓名（仅 alipay 方式，AES-256-GCM 加密，格式：base64(nonce || ciphertext)）';
COMMENT ON COLUMN node_tip_withdrawals.status IS '状态：pending / approved / completed / rejected';
COMMENT ON COLUMN node_tip_withdrawals.admin_remark IS '管理员备注（审批/操作备注，非审计日志，生产环境建议独立审计表）';
