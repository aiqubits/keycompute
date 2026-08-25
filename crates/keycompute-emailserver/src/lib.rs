//! 邮件服务模块
//!
//! 提供 SMTP 邮件发送功能：
//! - 注册验证码邮件
//! - 密码重置邮件
//! - 通用邮件发送
//!
//! # 配置
//!
//! 通过 `keycompute-config` 模块加载配置：
//! - debug 开发启动：`config.toml` 中的 `[email]` 部分
//! - release 生产启动：`KC__EMAIL__SMTP_HOST`、`KC__EMAIL__SMTP_PORT` 等环境变量
//!
//! # 热更新支持
//!
//! 支持运行时配置更新：
//! ```rust,ignore
//! email_service.update_config(new_config).await;
//! ```

// 重新导出配置类型，方便调用方使用
pub use keycompute_config::EmailConfig;

use keycompute_types::KeyComputeError;
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use url::Url;

/// 邮件发送错误
#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    /// 配置错误
    #[error("邮件服务未配置")]
    NotConfigured,

    /// 邮件配置无效
    #[error("邮件配置无效: {0}")]
    InvalidConfig(String),

    /// 邮箱地址格式错误
    #[error("无效的邮箱地址: {0}")]
    InvalidAddress(String),

    /// 邮件构建错误
    #[error("邮件构建失败: {0}")]
    BuildError(String),

    /// SMTP 发送错误
    #[error("邮件发送失败: {0}")]
    SendError(String),
}

impl From<EmailError> for KeyComputeError {
    fn from(err: EmailError) -> Self {
        match err {
            EmailError::NotConfigured | EmailError::SendError(_) => {
                KeyComputeError::ServiceUnavailable(err.to_string())
            }
            EmailError::InvalidAddress(_) => KeyComputeError::ValidationError(err.to_string()),
            EmailError::BuildError(_) | EmailError::InvalidConfig(_) => {
                KeyComputeError::ConfigError(err.to_string())
            }
        }
    }
}

#[derive(Clone)]
enum TransportState {
    Disabled,
    InvalidConfig(String),
    Ready(AsyncSmtpTransport<Tokio1Executor>),
}

impl TransportState {
    fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    fn require_ready(&self) -> Result<&AsyncSmtpTransport<Tokio1Executor>, EmailError> {
        match self {
            Self::Ready(transport) => Ok(transport),
            Self::Disabled => Err(EmailError::NotConfigured),
            Self::InvalidConfig(msg) => Err(EmailError::InvalidConfig(msg.clone())),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SmtpSecurityMode {
    StartTls,
    ImplicitTls,
    Plain,
}

#[derive(Clone)]
struct EmailRuntime {
    config: EmailConfig,
    transport: TransportState,
}

/// 邮件服务
#[derive(Clone)]
pub struct EmailService {
    runtime: Arc<RwLock<EmailRuntime>>,
}

fn is_local_development_host(url: &Url) -> bool {
    match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false),
        None => false,
    }
}

fn validate_public_app_base_url(app_base_url: &str) -> Result<Url, EmailError> {
    let parsed = Url::parse(app_base_url).map_err(|e| {
        EmailError::InvalidConfig(format!("APP_BASE_URL 必须是合法的绝对 URL: {}", e))
    })?;

    if parsed.host_str().is_none() {
        return Err(EmailError::InvalidConfig(
            "APP_BASE_URL 必须包含主机名".to_string(),
        ));
    }

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(EmailError::InvalidConfig(
            "APP_BASE_URL 不能包含用户名或密码".to_string(),
        ));
    }

    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(EmailError::InvalidConfig(
            "APP_BASE_URL 不能包含查询参数或片段".to_string(),
        ));
    }

    match parsed.scheme() {
        "https" => Ok(parsed),
        "http" if is_local_development_host(&parsed) => Ok(parsed),
        "http" => Err(EmailError::InvalidConfig(
            "APP_BASE_URL 在非本地环境必须使用 https".to_string(),
        )),
        scheme => Err(EmailError::InvalidConfig(format!(
            "APP_BASE_URL 仅支持 http/https 协议，当前为 {}",
            scheme
        ))),
    }
}

fn smtp_timeout(timeout_secs: u64) -> Option<Duration> {
    if timeout_secs == 0 {
        None
    } else {
        Some(Duration::from_secs(timeout_secs))
    }
}

fn smtp_security_mode(config: &EmailConfig) -> SmtpSecurityMode {
    if config.use_tls {
        if config.smtp_port == 465 {
            SmtpSecurityMode::ImplicitTls
        } else {
            SmtpSecurityMode::StartTls
        }
    } else {
        SmtpSecurityMode::Plain
    }
}

fn build_password_reset_url(app_base_url: &str, token: &str) -> Result<String, EmailError> {
    let mut parsed = validate_public_app_base_url(app_base_url)?;
    let current_path = parsed.path().trim_end_matches('/');
    let next_path = if current_path.is_empty() || current_path == "/" {
        format!("/auth/reset-password/{}", token)
    } else {
        format!("{}/auth/reset-password/{}", current_path, token)
    };
    parsed.set_path(&next_path);
    Ok(parsed.into())
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn build_welcome_greeting(name: Option<&str>) -> (String, String) {
    match name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => (
            format!("您好，{}！", name),
            format!("您好，{}！", escape_html(name)),
        ),
        None => ("您好！".to_string(), "您好！".to_string()),
    }
}

impl EmailService {
    /// 创建邮件服务实例
    pub fn new(config: EmailConfig) -> Self {
        let transport = Self::build_transport(&config);
        Self {
            runtime: Arc::new(RwLock::new(EmailRuntime { config, transport })),
        }
    }

    /// 从 Arc<EmailConfig> 创建（克隆内部数据）
    ///
    /// 注意：此方法会克隆 EmailConfig 的内部数据，不会共享 Arc。
    /// 如需共享配置，请直接使用 new()。
    pub fn from_arc(config: Arc<EmailConfig>) -> Self {
        Self::new((*config).clone())
    }

    /// 获取需求收集表单的接收人邮箱地址（来自配置）
    pub async fn requirement_recipient(&self) -> Option<String> {
        self.runtime
            .read()
            .await
            .config
            .requirement_recipient
            .clone()
    }

    /// 构建 SMTP 传输
    fn build_transport(config: &EmailConfig) -> TransportState {
        if !config.is_configured() {
            tracing::warn!("邮件服务未配置，邮件发送将被禁用");
            return TransportState::Disabled;
        }

        let creds = Credentials::new(config.smtp_username.clone(), config.smtp_password.clone());

        // lettre 0.11 的 pool 配置在启用 pool feature 后自动生效
        // 使用默认连接池配置（最大 10 个连接）
        let timeout = smtp_timeout(config.timeout_secs);

        // 添加调试日志
        tracing::info!(
            host = %config.smtp_host,
            port = config.smtp_port,
            username = %config.smtp_username,
            use_tls = config.use_tls,
            "正在构建 SMTP 传输"
        );

        let transport = match smtp_security_mode(config) {
            SmtpSecurityMode::StartTls => {
                match AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.smtp_host) {
                    Ok(builder) => builder
                        .credentials(creds)
                        // 163 邮箱可能需要显式指定认证机制
                        .authentication(vec![
                            lettre::transport::smtp::authentication::Mechanism::Login,
                        ])
                        .port(config.smtp_port)
                        .timeout(timeout)
                        .build(),
                    Err(e) => {
                        let msg = format!(
                            "无法为 SMTP 主机 '{}' 构建 STARTTLS 连接: {}",
                            config.smtp_host, e
                        );
                        tracing::error!(error = %msg, "邮件服务配置错误");
                        return TransportState::InvalidConfig(msg);
                    }
                }
            }
            SmtpSecurityMode::ImplicitTls => {
                match AsyncSmtpTransport::<Tokio1Executor>::relay(&config.smtp_host) {
                    Ok(builder) => builder
                        .credentials(creds)
                        // 163 邮箱可能需要显式指定认证机制
                        .authentication(vec![
                            lettre::transport::smtp::authentication::Mechanism::Login,
                        ])
                        .port(config.smtp_port)
                        .timeout(timeout)
                        .build(),
                    Err(e) => {
                        let msg = format!(
                            "无法为 SMTP 主机 '{}' 构建 SMTPS 连接: {}",
                            config.smtp_host, e
                        );
                        tracing::error!(error = %msg, "邮件服务配置错误");
                        return TransportState::InvalidConfig(msg);
                    }
                }
            }
            SmtpSecurityMode::Plain => {
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.smtp_host)
                    .credentials(creds)
                    .port(config.smtp_port)
                    .timeout(timeout)
                    .build()
            }
        };

        TransportState::Ready(transport)
    }

    /// 检查服务是否已配置
    pub async fn is_configured(&self) -> bool {
        self.runtime.read().await.transport.is_ready()
    }

    /// 更新配置（支持热更新）
    ///
    /// 更新配置后会原子性地替换 SMTP 运行时状态。
    pub async fn update_config(&self, config: EmailConfig) {
        let transport = Self::build_transport(&config);
        let mut runtime = self.runtime.write().await;
        *runtime = EmailRuntime { config, transport };

        tracing::info!("邮件服务配置已更新");
    }

    /// 获取当前配置的克隆
    pub async fn config(&self) -> EmailConfig {
        self.runtime.read().await.config.clone()
    }

    /// 发送注册验证码邮件
    pub async fn send_registration_code_email(
        &self,
        to: &str,
        code: &str,
        expires_minutes: i64,
    ) -> Result<(), EmailError> {
        let subject = "您的注册验证码";
        let text_body = format!(
            r#"您好！

您正在注册 KeyCompute。

您的邮箱验证码是：{}

验证码将在 {} 分钟后失效。如非本人操作，请忽略此邮件。

祝好，
KeyCompute 团队
"#,
            code, expires_minutes
        );

        let html_body = format!(
            r#"<html>
<body style="font-family: Arial, sans-serif; line-height: 1.6; color: #333;">
<div style="max-width: 600px; margin: 0 auto; padding: 20px;">
<h2 style="color: #2c5282;">您的注册验证码</h2>
<p>您好！</p>
<p>您正在注册 KeyCompute。</p>
<p>请输入以下验证码完成注册：</p>
<div style="margin: 24px 0; padding: 16px; background: #f7fafc; border: 1px solid #e2e8f0; border-radius: 8px; text-align: center;">
<span style="font-size: 28px; letter-spacing: 8px; font-weight: bold; color: #2d3748;">{}</span>
</div>
<p style="color: #718096; font-size: 14px;">验证码将在 {} 分钟后失效。如非本人操作，请忽略此邮件。</p>
<hr style="border: none; border-top: 1px solid #e2e8f0; margin: 20px 0;">
<p style="color: #718096; font-size: 12px;">KeyCompute 团队</p>
</div>
</body>
</html>"#,
            code, expires_minutes
        );

        self.send_html_email(to, subject, &text_body, &html_body)
            .await
    }

    /// 发送密码重置邮件
    pub async fn send_password_reset_email(
        &self,
        to: &str,
        token: &str,
        app_base_url: &str,
    ) -> Result<(), EmailError> {
        let reset_url = build_password_reset_url(app_base_url, token)?;

        let subject = "重置您的密码";
        let text_body = format!(
            r#"您好！

我们收到了重置您密码的请求。

请点击以下链接重置密码：
{}

此链接将在 1 小时后过期。

如果您没有请求重置密码，请忽略此邮件，您的密码不会改变。

祝好，
KeyCompute 团队
"#,
            reset_url
        );

        let html_body = format!(
            r#"<html>
<body style="font-family: Arial, sans-serif; line-height: 1.6; color: #333;">
<div style="max-width: 600px; margin: 0 auto; padding: 20px;">
<h2 style="color: #2c5282;">重置您的密码</h2>
<p>您好！</p>
<p>我们收到了重置您密码的请求。</p>
<p>请点击以下按钮重置密码：</p>
<p>
<a href="{}" style="display: inline-block; padding: 12px 24px; background-color: #e53e3e; color: white; text-decoration: none; border-radius: 4px;">
重置密码
</a>
</p>
<p>或复制以下链接到浏览器：<br><code style="word-break: break-all;">{}</code></p>
<p style="color: #718096; font-size: 14px;">此链接将在 1 小时后过期。</p>
<p style="color: #718096; font-size: 14px;">如果您没有请求重置密码，请忽略此邮件，您的密码不会改变。</p>
<hr style="border: none; border-top: 1px solid #e2e8f0; margin: 20px 0;">
<p style="color: #718096; font-size: 12px;">KeyCompute 团队</p>
</div>
</body>
</html>"#,
            reset_url, reset_url
        );

        self.send_html_email(to, subject, &text_body, &html_body)
            .await
    }

    /// 发送欢迎邮件（邮箱验证成功后）
    pub async fn send_welcome_email(&self, to: &str, name: Option<&str>) -> Result<(), EmailError> {
        let (text_greeting, html_greeting) = build_welcome_greeting(name);

        let subject = "欢迎加入 KeyCompute";
        let text_body = format!(
            "{text_greeting}\n\n恭喜您成功验证了邮箱地址。\n\n现在您可以开始使用 KeyCompute 的全部功能：\n• 创建和管理 API Key\n• 配置 LLM Provider\n• 监控使用量和费用\n\n如果您有任何问题，请随时联系我们的支持团队。\n\n祝好，\nKeyCompute 团队\n"
        );

        let html_body = format!(
            r#"<html>
<body style="font-family: Arial, sans-serif; line-height: 1.6; color: #333;">
<div style="max-width: 600px; margin: 0 auto; padding: 20px;">
<h2 style="color: #2c5282;">欢迎加入 KeyCompute</h2>
<p>{}</p>
<p>恭喜您成功验证了邮箱地址。</p>
<p>现在您可以开始使用 KeyCompute 的全部功能：</p>
<ul>
<li>创建和管理 API Key</li>
<li>配置 LLM Provider</li>
<li>监控使用量和费用</li>
</ul>
<p>如果您有任何问题，请随时联系我们的支持团队。</p>
<hr style="border: none; border-top: 1px solid #e2e8f0; margin: 20px 0;">
<p style="color: #718096; font-size: 12px;">KeyCompute 团队</p>
</div>
</body>
</html>"#,
            html_greeting
        );

        self.send_html_email(to, subject, &text_body, &html_body)
            .await
    }

    /// 发送纯文本邮件
    pub async fn send_text_email(
        &self,
        to: &str,
        subject: &str,
        body: &str,
    ) -> Result<(), EmailError> {
        let runtime = self.runtime.read().await.clone();
        let transport = runtime.transport.require_ready()?;
        let from_mailbox = Self::build_from_mailbox(&runtime.config)?;

        let to_mailbox: Mailbox = to
            .parse()
            .map_err(|_| EmailError::InvalidAddress(to.to_string()))?;

        // 构建邮件
        let email = Message::builder()
            .from(from_mailbox)
            .to(to_mailbox)
            .subject(subject)
            .body(body.to_string())
            .map_err(|e| EmailError::BuildError(e.to_string()))?;

        transport
            .send(email)
            .await
            .map_err(|e| EmailError::SendError(e.to_string()))?;

        tracing::info!(
            to = %to,
            subject = %subject,
            "邮件发送成功"
        );

        Ok(())
    }

    /// 构建发件人邮箱地址
    fn build_from_mailbox(config: &EmailConfig) -> Result<Mailbox, EmailError> {
        let from_str = match &config.from_name {
            Some(name) => format!("{} <{}>", name, config.from_address),
            None => config.from_address.clone(),
        };

        from_str.parse().map_err(|_| {
            EmailError::BuildError(format!("Invalid from address: {}", config.from_address))
        })
    }

    /// 发送带 HTML 正文的多部分邮件
    pub async fn send_html_email(
        &self,
        to: &str,
        subject: &str,
        text_body: &str,
        html_body: &str,
    ) -> Result<(), EmailError> {
        let runtime = self.runtime.read().await.clone();
        let transport = runtime.transport.require_ready()?;
        let from_mailbox = Self::build_from_mailbox(&runtime.config)?;

        let to_mailbox: Mailbox = to
            .parse()
            .map_err(|_| EmailError::InvalidAddress(to.to_string()))?;

        // 构建邮件（不需要 transport）
        let email = Message::builder()
            .from(from_mailbox)
            .to(to_mailbox)
            .subject(subject)
            .multipart(
                lettre::message::MultiPart::alternative()
                    .singlepart(
                        lettre::message::SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(text_body.to_string()),
                    )
                    .singlepart(
                        lettre::message::SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(html_body.to_string()),
                    ),
            )
            .map_err(|e| EmailError::BuildError(e.to_string()))?;

        transport
            .send(email)
            .await
            .map_err(|e| EmailError::SendError(e.to_string()))?;

        tracing::info!(
            to = %to,
            subject = %subject,
            "HTML 邮件发送成功"
        );

        Ok(())
    }

    /// 发送节点网关注册令牌邮件（管理员审批通过后通知用户）
    ///
    /// 邮件中包含完整的 token 明文，用户可使用该 token 注册节点。
    /// 邮件发送失败不阻塞审批流程。
    pub async fn send_node_gateway_token_email(
        &self,
        to: &str,
        token_plaintext: &str,
        token_preview: &str,
    ) -> Result<(), EmailError> {
        let subject = "您的节点网关注册令牌已审批通过";
        let text_body = format!(
            r#"您好！

您的节点网关注册令牌已通过管理员审批。

您的令牌（Token）：
{token_plaintext}

令牌预览：{token_preview}

使用方式：
在节点的配置文件中设置此令牌，即可完成节点注册。
请注意：此令牌为一次性使用，注册后即失效。

请妥善保管此令牌，切勿泄露给他人。

祝好，
KeyCompute 团队
"#
        );

        let html_body = format!(
            r#"<html>
<body style="font-family: Arial, sans-serif; line-height: 1.6; color: #333;">
<div style="max-width: 600px; margin: 0 auto; padding: 20px;">
<h2 style="color: #2c5282;">节点网关注册令牌已审批通过</h2>
<p>您好！</p>
<p>您的节点网关注册令牌已通过管理员审批。</p>
<div style="margin: 24px 0; padding: 16px; background: #f7fafc; border: 1px solid #e2e8f0; border-radius: 8px;">
<p style="margin: 0 0 8px 0; color: #718096; font-size: 14px;">您的令牌（Token）：</p>
<code style="display: block; padding: 12px; background: #edf2f7; border-radius: 4px; word-break: break-all; font-size: 14px; color: #2d3748;">{token_plaintext}</code>
<p style="margin: 8px 0 0 0; color: #a0aec0; font-size: 12px;">令牌预览：{token_preview}</p>
</div>
<p>使用方式：在节点的配置文件中设置此令牌，即可完成节点注册。</p>
<p style="color: #e53e3e; font-weight: bold;">请注意：此令牌为一次性使用，注册后即失效。</p>
<p style="color: #e53e3e;">请妥善保管此令牌，切勿泄露给他人。</p>
<hr style="border: none; border-top: 1px solid #e2e8f0; margin: 20px 0;">
<p style="color: #718096; font-size: 12px;">KeyCompute 团队</p>
</div>
</body>
</html>"#
        );

        self.send_html_email(to, subject, &text_body, &html_body)
            .await
    }
}

impl std::fmt::Debug for EmailService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Debug 实现避免使用异步操作，防止阻塞和线程池问题
        // 只显示类型信息，不尝试获取锁
        f.debug_struct("EmailService")
            .field("type", &"EmailService")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> EmailConfig {
        EmailConfig {
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: 465,
            smtp_username: "test@example.com".to_string(),
            smtp_password: "testpass".to_string(),
            from_address: "noreply@example.com".to_string(),
            from_name: Some("KeyCompute".to_string()),
            use_tls: true,
            timeout_secs: 30,
            requirement_recipient: None,
        }
    }

    #[tokio::test]
    async fn test_email_service_creation() {
        let service = EmailService::new(test_config());
        assert!(service.is_configured().await);
    }

    #[tokio::test]
    async fn test_localhost_email_service_creation() {
        let mut config = test_config();
        config.smtp_host = "localhost".to_string();
        config.use_tls = false;

        let service = EmailService::new(config);
        assert!(service.is_configured().await);
    }

    #[tokio::test]
    async fn test_email_service_not_configured() {
        let service = EmailService::new(EmailConfig::default());
        assert!(!service.is_configured().await);
    }

    #[tokio::test]
    async fn test_invalid_email_address() {
        let service = EmailService::new(test_config());

        let result = service
            .send_text_email("invalid-email", "Test", "Body")
            .await;

        assert!(matches!(result, Err(EmailError::InvalidAddress(_))));
    }

    #[tokio::test]
    async fn test_send_without_config() {
        let service = EmailService::new(EmailConfig::default());

        let text_result = service
            .send_text_email("test@example.com", "Test", "Body")
            .await;
        let html_result = service
            .send_html_email("test@example.com", "Test", "Body", "<p>Body</p>")
            .await;

        assert!(matches!(text_result, Err(EmailError::NotConfigured)));
        assert!(matches!(html_result, Err(EmailError::NotConfigured)));
    }

    #[tokio::test]
    async fn test_config_update() {
        let service = EmailService::new(EmailConfig::default());
        assert!(!service.is_configured().await);

        // 更新配置
        let new_config = test_config();
        service.update_config(new_config).await;

        assert!(service.is_configured().await);
    }

    #[tokio::test]
    async fn test_from_name_usage() {
        let mut config = test_config();
        config.from_name = Some("Test Sender".to_string());

        let service = EmailService::new(config);
        let cfg = service.config().await;

        assert_eq!(cfg.from_name, Some("Test Sender".to_string()));
    }

    #[test]
    fn test_build_password_reset_url_trims_trailing_slash() {
        let reset_url =
            build_password_reset_url("https://app.example.com/", "reset456").expect("valid URL");

        assert_eq!(
            reset_url,
            "https://app.example.com/auth/reset-password/reset456"
        );
    }

    #[test]
    fn test_build_password_reset_url_preserves_base_path() {
        let reset_url = build_password_reset_url("https://app.example.com/console", "reset456")
            .expect("valid URL");

        assert_eq!(
            reset_url,
            "https://app.example.com/console/auth/reset-password/reset456"
        );
    }

    #[test]
    fn test_build_password_reset_url_rejects_remote_http() {
        let err = build_password_reset_url("http://example.com", "reset456")
            .expect_err("remote http should be rejected");

        assert!(matches!(err, EmailError::InvalidConfig(_)));
    }

    #[test]
    fn test_smtp_timeout_zero_disables_timeout() {
        assert_eq!(smtp_timeout(0), None);
        assert_eq!(smtp_timeout(30), Some(Duration::from_secs(30)));
    }

    #[test]
    fn test_smtp_security_mode_prefers_starttls_for_standard_submission() {
        let mut config = test_config();
        config.smtp_port = 587;
        config.use_tls = true;

        assert_eq!(smtp_security_mode(&config), SmtpSecurityMode::StartTls);
    }

    #[test]
    fn test_smtp_security_mode_uses_implicit_tls_for_port_465() {
        let mut config = test_config();
        config.smtp_port = 465;
        config.use_tls = true;

        assert_eq!(smtp_security_mode(&config), SmtpSecurityMode::ImplicitTls);
    }

    #[test]
    fn test_smtp_security_mode_plain_when_tls_disabled() {
        let mut config = test_config();
        config.use_tls = false;

        assert_eq!(smtp_security_mode(&config), SmtpSecurityMode::Plain);
    }

    #[tokio::test]
    async fn test_invalid_public_base_url_reports_configuration_error() {
        let service = EmailService::new(test_config());
        let err = service
            .send_password_reset_email("test@example.com", "token", "http://example.com")
            .await
            .expect_err("invalid public base URL should fail");

        assert!(matches!(err, EmailError::InvalidConfig(_)));
    }

    #[test]
    fn test_build_welcome_greeting_without_name() {
        let (text_greeting, html_greeting) = build_welcome_greeting(None);

        assert_eq!(text_greeting, "您好！");
        assert_eq!(html_greeting, "您好！");
    }

    #[test]
    fn test_build_welcome_greeting_trims_blank_name() {
        let (text_greeting, html_greeting) = build_welcome_greeting(Some("   "));

        assert_eq!(text_greeting, "您好！");
        assert_eq!(html_greeting, "您好！");
    }

    #[test]
    fn test_build_welcome_greeting_escapes_html_name() {
        let (text_greeting, html_greeting) = build_welcome_greeting(Some(" <b>Alice & Bob</b> "));

        assert_eq!(text_greeting, "您好，<b>Alice & Bob</b>！");
        assert_eq!(html_greeting, "您好，&lt;b&gt;Alice &amp; Bob&lt;/b&gt;！");
    }
}
