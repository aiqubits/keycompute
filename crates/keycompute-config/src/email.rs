//! 邮件服务配置
//!
//! 提供 SMTP 邮件发送配置：
//! - SMTP 服务器连接参数
//! - 发件人信息
//! - TLS 配置

use serde::Deserialize;

/// 邮件服务配置
#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct EmailConfig {
    /// SMTP 服务器地址
    pub smtp_host: String,
    /// SMTP 端口（默认 465）
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    /// SMTP 用户名
    pub smtp_username: String,
    /// SMTP 密码
    pub smtp_password: String,
    /// 发件人邮箱地址
    pub from_address: String,
    /// 发件人显示名称（可选）
    pub from_name: Option<String>,
    /// 是否使用 TLS（默认 true）
    #[serde(default = "default_use_tls")]
    pub use_tls: bool,
    /// 邮件发送超时（秒，默认 30）
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// 需求收集表单的接收人邮箱地址（首页"提交算力需求"提交后发送至此，可选）
    #[serde(default)]
    pub requirement_recipient: Option<String>,
}

fn default_smtp_port() -> u16 {
    465
}

fn default_use_tls() -> bool {
    true
}

fn default_timeout_secs() -> u64 {
    30
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            smtp_host: String::new(),
            smtp_port: 465,
            smtp_username: String::new(),
            smtp_password: String::new(),
            from_address: String::new(),
            from_name: Some("KeyCompute".to_string()),
            use_tls: true,
            timeout_secs: 30,
            requirement_recipient: None,
        }
    }
}

impl EmailConfig {
    /// 检查配置是否有效（非默认值）
    pub fn is_configured(&self) -> bool {
        self.smtp_port != 0
            && !self.smtp_host.trim().is_empty()
            && !self.smtp_username.trim().is_empty()
            && !self.smtp_password.trim().is_empty()
            && !self.from_address.trim().is_empty()
    }

    /// 检查是否处于“部分配置”状态
    ///
    /// 只有至少一个 SMTP 必填项（或依赖 SMTP 的需求接收人）已填写、但
    /// 完整配置仍不成立时，才视为部分配置。仅调整显示名称、TLS 或超时
    /// 不代表启用邮件能力。
    pub fn is_partially_configured(&self) -> bool {
        let has_email_intent = [
            self.smtp_host.as_str(),
            self.smtp_username.as_str(),
            self.smtp_password.as_str(),
            self.from_address.as_str(),
        ]
        .into_iter()
        .any(|value| !value.trim().is_empty())
            || self
                .requirement_recipient
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());

        has_email_intent && !self.is_configured()
    }

    /// 获取完整的发件人地址（带名称）
    pub fn from_header(&self) -> String {
        match &self.from_name {
            Some(name) => format!("{} <{}>", name, self.from_address),
            None => self.from_address.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_email_config() {
        let config = EmailConfig::default();
        assert!(config.smtp_host.is_empty());
        assert!(config.from_address.is_empty());
        assert_eq!(config.smtp_port, 465);
        assert!(config.use_tls);
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn test_from_header_with_name() {
        let config = EmailConfig {
            from_address: "noreply@example.com".to_string(),
            from_name: Some("KeyCompute".to_string()),
            ..Default::default()
        };
        assert_eq!(config.from_header(), "KeyCompute <noreply@example.com>");
    }

    #[test]
    fn test_from_header_without_name() {
        let config = EmailConfig {
            from_address: "noreply@example.com".to_string(),
            from_name: None,
            ..Default::default()
        };
        assert_eq!(config.from_header(), "noreply@example.com");
    }

    #[test]
    fn test_is_configured() {
        let default_config = EmailConfig::default();
        assert!(!default_config.is_configured());
        assert!(!default_config.is_partially_configured());

        let configured = EmailConfig {
            smtp_host: "smtp.example.com".to_string(),
            smtp_username: "user".to_string(),
            smtp_password: "pass".to_string(),
            from_address: "noreply@example.com".to_string(),
            ..Default::default()
        };
        assert!(configured.is_configured());
        assert!(!configured.is_partially_configured());
    }

    #[test]
    fn test_is_partially_configured() {
        let partial = EmailConfig {
            smtp_host: "smtp.example.com".to_string(),
            smtp_username: "   ".to_string(),
            ..Default::default()
        };

        assert!(!partial.is_configured());
        assert!(partial.is_partially_configured());

        let optional_only = EmailConfig {
            from_name: Some("Custom Brand".to_string()),
            use_tls: false,
            timeout_secs: 5,
            ..Default::default()
        };
        assert!(!optional_only.is_partially_configured());

        let recipient_without_smtp = EmailConfig {
            requirement_recipient: Some("ops@example.com".to_string()),
            ..Default::default()
        };
        assert!(recipient_without_smtp.is_partially_configured());
    }
}
