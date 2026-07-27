//! 支付宝配置模块

use serde::Deserialize;
use url::{Host, Url};

/// 支付宝环境
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum AlipayEnv {
    /// 沙箱环境
    Sandbox,
    /// 生产环境
    #[default]
    Production,
}

impl AlipayEnv {
    /// 获取支付宝网关地址
    pub fn gateway_url(&self) -> &'static str {
        match self {
            AlipayEnv::Sandbox => "https://openapi.alipaydev.com/gateway.do",
            AlipayEnv::Production => "https://openapi.alipay.com/gateway.do",
        }
    }

    /// 是否为沙箱环境
    pub fn is_sandbox(&self) -> bool {
        matches!(self, AlipayEnv::Sandbox)
    }
}

/// 支付宝配置
#[derive(Debug, Clone, Deserialize)]
pub struct AlipayConfig {
    /// 应用ID (AppID)
    pub app_id: String,
    /// 应用私钥 (PEM格式)
    pub private_key: String,
    /// 支付宝公钥 (PEM格式，用于验签)
    pub alipay_public_key: String,
    /// 轮换窗口内仅用于验证延迟/重试回调的历史支付宝公钥。
    #[serde(default)]
    pub previous_alipay_public_keys: Vec<String>,
    /// 环境
    #[serde(default)]
    pub env: AlipayEnv,
    /// 异步通知地址
    pub notify_url: String,
    /// 同步返回地址
    pub return_url: Option<String>,
    /// 签名类型 (默认RSA2)
    #[serde(default = "default_sign_type")]
    pub sign_type: String,
    /// 字符集 (默认UTF-8)
    #[serde(default = "default_charset")]
    pub charset: String,
    /// 版本 (默认1.0)
    #[serde(default = "default_version")]
    pub version: String,
    /// 支付超时时间（分钟），默认30分钟
    #[serde(default = "default_timeout")]
    pub timeout_minutes: i32,
}

fn default_sign_type() -> String {
    "RSA2".to_string()
}

fn default_charset() -> String {
    "utf-8".to_string()
}

fn default_version() -> String {
    "1.0".to_string()
}

fn default_timeout() -> i32 {
    30
}

const MAX_TIMEOUT_MINUTES: i32 = 15 * 24 * 60;

impl Default for AlipayConfig {
    fn default() -> Self {
        Self {
            app_id: String::new(),
            private_key: String::new(),
            alipay_public_key: String::new(),
            previous_alipay_public_keys: Vec::new(),
            env: AlipayEnv::default(),
            notify_url: String::new(),
            return_url: None,
            sign_type: default_sign_type(),
            charset: default_charset(),
            version: default_version(),
            timeout_minutes: default_timeout(),
        }
    }
}

impl AlipayConfig {
    /// 从环境变量创建配置
    pub fn from_env() -> Result<Self, ConfigError> {
        let app_id = std::env::var("ALIPAY_APP_ID")
            .map_err(|_| ConfigError::MissingEnvVar("ALIPAY_APP_ID"))?;

        let private_key = std::env::var("ALIPAY_PRIVATE_KEY")
            .map_err(|_| ConfigError::MissingEnvVar("ALIPAY_PRIVATE_KEY"))?;

        let alipay_public_key = std::env::var("ALIPAY_PUBLIC_KEY")
            .map_err(|_| ConfigError::MissingEnvVar("ALIPAY_PUBLIC_KEY"))?;
        let previous_alipay_public_keys =
            parse_previous_public_keys(std::env::var("ALIPAY_PREVIOUS_PUBLIC_KEYS_JSON").ok())?;

        let notify_url = std::env::var("ALIPAY_NOTIFY_URL")
            .map_err(|_| ConfigError::MissingEnvVar("ALIPAY_NOTIFY_URL"))?;

        let env = parse_alipay_env(std::env::var("ALIPAY_ENV").ok())?;

        let return_url = std::env::var("ALIPAY_RETURN_URL")
            .ok()
            .and_then(non_empty_value);
        let timeout_minutes = parse_timeout_minutes(std::env::var("ALIPAY_TIMEOUT_MINUTES").ok())?;

        Ok(Self {
            app_id,
            private_key: format_private_key(&private_key),
            alipay_public_key: format_public_key(&alipay_public_key),
            previous_alipay_public_keys,
            env,
            notify_url,
            return_url,
            sign_type: default_sign_type(),
            charset: default_charset(),
            version: default_version(),
            timeout_minutes,
        })
    }

    /// 验证配置
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.app_id.is_empty() {
            return Err(ConfigError::InvalidConfig("app_id 不能为空"));
        }
        if self.private_key.is_empty() {
            return Err(ConfigError::InvalidConfig("private_key 不能为空"));
        }
        if self.alipay_public_key.is_empty() {
            return Err(ConfigError::InvalidConfig("alipay_public_key 不能为空"));
        }
        if self.notify_url.is_empty() {
            return Err(ConfigError::InvalidConfig("notify_url 不能为空"));
        }
        if !(1..=MAX_TIMEOUT_MINUTES).contains(&self.timeout_minutes) {
            return Err(ConfigError::InvalidConfig(
                "timeout_minutes 必须在 1 到 21600 之间",
            ));
        }
        if !is_secure_notify_url(&self.notify_url) {
            return Err(ConfigError::InvalidConfig(
                "notify_url 在非本地环境必须使用 HTTPS",
            ));
        }
        Ok(())
    }

    /// 获取网关URL
    pub fn gateway_url(&self) -> &'static str {
        self.env.gateway_url()
    }
}

fn parse_previous_public_keys(value: Option<String>) -> Result<Vec<String>, ConfigError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    let keys: Vec<String> = serde_json::from_str(&value).map_err(|_| {
        ConfigError::InvalidConfig("ALIPAY_PREVIOUS_PUBLIC_KEYS_JSON 必须是字符串数组")
    })?;
    if keys.iter().any(|key| key.trim().is_empty()) {
        return Err(ConfigError::InvalidConfig(
            "ALIPAY_PREVIOUS_PUBLIC_KEYS_JSON 不能包含空公钥",
        ));
    }
    Ok(keys
        .into_iter()
        .map(|key| format_public_key(&key))
        .collect())
}

fn parse_alipay_env(value: Option<String>) -> Result<AlipayEnv, ConfigError> {
    match value.as_deref().map(str::trim).map(str::to_ascii_lowercase) {
        None => Ok(AlipayEnv::Production),
        Some(value) if value == "production" || value == "prod" => Ok(AlipayEnv::Production),
        Some(value) if matches!(value.as_str(), "sandbox" | "dev" | "test") => {
            Ok(AlipayEnv::Sandbox)
        }
        Some(_) => Err(ConfigError::InvalidConfig(
            "ALIPAY_ENV 必须是 production 或 sandbox",
        )),
    }
}

fn parse_timeout_minutes(value: Option<String>) -> Result<i32, ConfigError> {
    match value {
        Some(value) => value
            .parse()
            .map_err(|_| ConfigError::InvalidConfig("ALIPAY_TIMEOUT_MINUTES 必须是有效整数")),
        None => Ok(default_timeout()),
    }
}

fn non_empty_value(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn is_secure_notify_url(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    match url.scheme() {
        "https" => url.host().is_some(),
        "http" => match url.host() {
            Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
}

/// 配置错误
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("缺少环境变量: {0}")]
    MissingEnvVar(&'static str),
    #[error("配置无效: {0}")]
    InvalidConfig(&'static str),
}

/// 格式化私钥（添加PEM头尾）
fn format_private_key(key: &str) -> String {
    normalize_pem(key, "PRIVATE KEY")
}

/// 格式化公钥（添加 PEM 头尾）
fn format_public_key(key: &str) -> String {
    normalize_pem(key, "PUBLIC KEY")
}

fn normalize_pem(value: &str, default_label: &str) -> String {
    let expanded = value.replace("\\n", "\n");
    let trimmed = expanded.trim();
    if trimmed.starts_with("-----BEGIN ") {
        // Preserve the original PKCS#1/PKCS#8 label. Relabelling the same DER
        // payload changes its meaning and makes otherwise valid keys fail to parse.
        return trimmed.to_string();
    }

    let payload: String = trimmed.chars().filter(|ch| !ch.is_whitespace()).collect();
    format!("-----BEGIN {default_label}-----\n{payload}\n-----END {default_label}-----")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::{AlipaySigner, AlipayVerifier};
    use rsa::{
        RsaPrivateKey,
        pkcs1::{EncodeRsaPrivateKey, EncodeRsaPublicKey},
        pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding},
        rand_core::OsRng,
    };

    #[test]
    fn test_alipay_env() {
        let env = AlipayEnv::Sandbox;
        assert!(env.is_sandbox());
        assert_eq!(
            env.gateway_url(),
            "https://openapi.alipaydev.com/gateway.do"
        );

        let env = AlipayEnv::Production;
        assert!(!env.is_sandbox());
        assert_eq!(env.gateway_url(), "https://openapi.alipay.com/gateway.do");
    }

    #[test]
    fn unknown_alipay_environment_is_rejected() {
        assert_eq!(parse_alipay_env(None).unwrap(), AlipayEnv::Production);
        assert_eq!(
            parse_alipay_env(Some("sandbox".to_string())).unwrap(),
            AlipayEnv::Sandbox
        );
        assert!(parse_alipay_env(Some("sandox".to_string())).is_err());
    }

    #[test]
    fn malformed_timeout_is_rejected_instead_of_silently_defaulted() {
        assert_eq!(parse_timeout_minutes(None).unwrap(), 30);
        assert_eq!(parse_timeout_minutes(Some("15".to_string())).unwrap(), 15);
        assert!(parse_timeout_minutes(Some("later".to_string())).is_err());
    }

    #[test]
    fn previous_public_keys_are_explicit_json_and_normalized() {
        assert!(parse_previous_public_keys(None).unwrap().is_empty());
        let keys =
            parse_previous_public_keys(Some(r#"["old-key-a", "old-key-b"]"#.to_string())).unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys[0].starts_with("-----BEGIN PUBLIC KEY-----"));
        assert!(parse_previous_public_keys(Some("old-key".to_string())).is_err());
        assert!(parse_previous_public_keys(Some(r#"[""]"#.to_string())).is_err());
    }

    #[test]
    fn provider_timeout_range_is_enforced() {
        let mut config = AlipayConfig {
            app_id: "app".to_string(),
            private_key: "private".to_string(),
            alipay_public_key: "public".to_string(),
            notify_url: "https://example.com/notify".to_string(),
            ..AlipayConfig::default()
        };
        config.timeout_minutes = MAX_TIMEOUT_MINUTES;
        assert!(config.validate().is_ok());
        config.timeout_minutes = MAX_TIMEOUT_MINUTES + 1;
        assert!(config.validate().is_err());
    }

    #[test]
    fn empty_optional_return_url_is_treated_as_unset() {
        assert_eq!(non_empty_value(String::new()), None);
        assert_eq!(non_empty_value("   ".to_string()), None);
        assert_eq!(
            non_empty_value("https://example.com/payments".to_string()).as_deref(),
            Some("https://example.com/payments")
        );
    }

    #[test]
    fn notify_url_requires_https_except_for_exact_loopback_hosts() {
        for valid in [
            "https://payments.example.com/alipay",
            "http://localhost:3000/alipay",
            "http://127.0.0.1:3000/alipay",
            "http://[::1]:3000/alipay",
        ] {
            assert!(is_secure_notify_url(valid), "{valid} should be accepted");
        }
        for invalid in [
            "http://payments.example.com/alipay",
            "http://localhost.evil.example/alipay",
            "http://localhost@evil.example/alipay",
            "javascript:alert(1)",
            "not a url",
        ] {
            assert!(
                !is_secure_notify_url(invalid),
                "{invalid} should be rejected"
            );
        }
    }

    #[test]
    fn test_default_config() {
        let config = AlipayConfig::default();
        assert_eq!(config.sign_type, "RSA2");
        assert_eq!(config.charset, "utf-8");
        assert_eq!(config.version, "1.0");
        assert_eq!(config.timeout_minutes, 30);
    }

    #[test]
    fn test_format_keys() {
        let raw_key = "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC";
        let formatted = format_private_key(raw_key);
        assert!(formatted.contains("-----BEGIN PRIVATE KEY-----"));
        assert!(formatted.contains("-----END PRIVATE KEY-----"));

        let formatted_pub = format_public_key(raw_key);
        assert!(formatted_pub.contains("-----BEGIN PUBLIC KEY-----"));
        assert!(formatted_pub.contains("-----END PUBLIC KEY-----"));
        assert!(format_public_key("abncd").contains("abncd"));
    }

    #[test]
    fn pem_normalization_preserves_pkcs1_and_pkcs8_key_types() {
        let private_key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let public_key = private_key.to_public_key();

        let pkcs1_private = private_key.to_pkcs1_pem(LineEnding::LF).unwrap();
        let pkcs1_public = public_key.to_pkcs1_pem(LineEnding::LF).unwrap();
        let normalized_private = format_private_key(&pkcs1_private.replace('\n', "\\n"));
        let normalized_public = format_public_key(&pkcs1_public.replace('\n', "\\n"));
        assert!(normalized_private.starts_with("-----BEGIN RSA PRIVATE KEY-----"));
        assert!(normalized_public.starts_with("-----BEGIN RSA PUBLIC KEY-----"));
        AlipaySigner::from_pem(&normalized_private).unwrap();
        AlipayVerifier::from_pem(&normalized_public).unwrap();

        let pkcs8_private = private_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let pkcs8_public = public_key.to_public_key_pem(LineEnding::LF).unwrap();
        AlipaySigner::from_pem(&format_private_key(&pkcs8_private)).unwrap();
        AlipayVerifier::from_pem(&format_public_key(&pkcs8_public)).unwrap();
    }
}
