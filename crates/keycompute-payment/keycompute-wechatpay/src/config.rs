use serde::Deserialize;
use url::{Host, Url};

#[derive(Debug, Clone, Deserialize)]
pub struct WechatPayConfig {
    pub appid: String,
    pub mchid: String,
    pub merchant_serial_no: String,
    pub merchant_private_key: String,
    pub api_v3_key: String,
    pub wechatpay_public_key_id: String,
    pub wechatpay_public_key: String,
    /// 轮换窗口内仅用于存量回调验签和解密的历史密钥。
    #[serde(default)]
    pub previous_callback_keys: Vec<WechatPayCallbackKey>,
    pub notify_url: String,
    #[serde(default = "default_timeout_minutes")]
    pub timeout_minutes: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WechatPayCallbackKey {
    pub public_key_id: String,
    pub public_key: String,
    pub api_v3_key: String,
}

fn default_timeout_minutes() -> i32 {
    15
}

const MAX_TIMEOUT_MINUTES: i32 = 15 * 24 * 60;

impl WechatPayConfig {
    pub fn from_env() -> Result<Self, WechatPayConfigError> {
        Ok(Self {
            appid: env("WECHATPAY_APP_ID", "KC__PAYMENT__WECHATPAY__APPID")?,
            mchid: env("WECHATPAY_MCH_ID", "KC__PAYMENT__WECHATPAY__MCHID")?,
            merchant_serial_no: env(
                "WECHATPAY_MERCHANT_SERIAL_NO",
                "KC__PAYMENT__WECHATPAY__MERCHANT_SERIAL_NO",
            )?,
            merchant_private_key: normalize_private_key(&env(
                "WECHATPAY_MERCHANT_PRIVATE_KEY",
                "KC__PAYMENT__WECHATPAY__MERCHANT_PRIVATE_KEY",
            )?),
            api_v3_key: env("WECHATPAY_API_V3_KEY", "KC__PAYMENT__WECHATPAY__API_V3_KEY")?,
            wechatpay_public_key_id: env(
                "WECHATPAY_PUBLIC_KEY_ID",
                "KC__PAYMENT__WECHATPAY__WECHATPAY_PUBLIC_KEY_ID",
            )?,
            wechatpay_public_key: normalize_public_key(&env(
                "WECHATPAY_PUBLIC_KEY",
                "KC__PAYMENT__WECHATPAY__WECHATPAY_PUBLIC_KEY",
            )?),
            previous_callback_keys: parse_previous_callback_keys(env_optional(
                "WECHATPAY_PREVIOUS_CALLBACK_KEYS_JSON",
                "KC__PAYMENT__WECHATPAY__PREVIOUS_CALLBACK_KEYS_JSON",
            ))?,
            notify_url: env("WECHATPAY_NOTIFY_URL", "KC__PAYMENT__WECHATPAY__NOTIFY_URL")?,
            timeout_minutes: parse_timeout_minutes(env_optional(
                "WECHATPAY_TIMEOUT_MINUTES",
                "KC__PAYMENT__WECHATPAY__TIMEOUT_MINUTES",
            ))?,
        })
    }

    pub fn validate(&self) -> Result<(), WechatPayConfigError> {
        for (name, value) in [
            ("appid", &self.appid),
            ("mchid", &self.mchid),
            ("merchant_serial_no", &self.merchant_serial_no),
            ("merchant_private_key", &self.merchant_private_key),
            ("wechatpay_public_key_id", &self.wechatpay_public_key_id),
            ("wechatpay_public_key", &self.wechatpay_public_key),
            ("notify_url", &self.notify_url),
        ] {
            if value.trim().is_empty() {
                return Err(WechatPayConfigError::Invalid(format!("{name} is empty")));
            }
        }
        if self.api_v3_key.len() != 32 {
            return Err(WechatPayConfigError::Invalid(
                "api_v3_key must contain exactly 32 bytes".to_string(),
            ));
        }
        for key in &self.previous_callback_keys {
            if key.public_key_id.trim().is_empty() || key.public_key.trim().is_empty() {
                return Err(WechatPayConfigError::Invalid(
                    "previous callback key id and public key must not be empty".to_string(),
                ));
            }
            if key.api_v3_key.len() != 32 {
                return Err(WechatPayConfigError::Invalid(
                    "previous callback API v3 key must contain exactly 32 bytes".to_string(),
                ));
            }
        }
        if !(1..=MAX_TIMEOUT_MINUTES).contains(&self.timeout_minutes) {
            return Err(WechatPayConfigError::Invalid(
                "timeout_minutes must be between 1 and 21600".to_string(),
            ));
        }
        if !is_secure_notify_url(&self.notify_url) {
            return Err(WechatPayConfigError::Invalid(
                "notify_url must use HTTPS outside local development".to_string(),
            ));
        }
        Ok(())
    }
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

fn env(primary: &'static str, fallback: &'static str) -> Result<String, WechatPayConfigError> {
    env_optional(primary, fallback).ok_or(WechatPayConfigError::Missing(primary))
}

fn env_optional(primary: &str, fallback: &str) -> Option<String> {
    select_env_value(std::env::var(primary).ok(), std::env::var(fallback).ok())
}

fn select_env_value(primary: Option<String>, fallback: Option<String>) -> Option<String> {
    primary
        .filter(|value| !value.trim().is_empty())
        .or_else(|| fallback.filter(|value| !value.trim().is_empty()))
}

fn parse_timeout_minutes(value: Option<String>) -> Result<i32, WechatPayConfigError> {
    match value {
        Some(value) => value.parse().map_err(|_| {
            WechatPayConfigError::Invalid("timeout_minutes must be a valid integer".to_string())
        }),
        None => Ok(default_timeout_minutes()),
    }
}

fn parse_previous_callback_keys(
    value: Option<String>,
) -> Result<Vec<WechatPayCallbackKey>, WechatPayConfigError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    let mut keys: Vec<WechatPayCallbackKey> = serde_json::from_str(&value).map_err(|_| {
        WechatPayConfigError::Invalid("previous callback keys must be a JSON array".to_string())
    })?;
    for key in &mut keys {
        key.public_key = normalize_public_key(&key.public_key);
    }
    Ok(keys)
}

fn normalize_private_key(value: &str) -> String {
    value.replace("\\n", "\n")
}

fn normalize_public_key(value: &str) -> String {
    value.replace("\\n", "\n")
}

#[derive(Debug, thiserror::Error)]
pub enum WechatPayConfigError {
    #[error("missing environment variable: {0}")]
    Missing(&'static str),
    #[error("invalid WeChat Pay configuration: {0}")]
    Invalid(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_url_requires_https_except_for_exact_loopback_hosts() {
        for valid in [
            "https://payments.example.com/wechatpay",
            "http://localhost:3000/wechatpay",
            "http://127.0.0.1:3000/wechatpay",
            "http://[::1]:3000/wechatpay",
        ] {
            assert!(is_secure_notify_url(valid), "{valid} should be accepted");
        }
        for invalid in [
            "http://payments.example.com/wechatpay",
            "http://localhost.evil.example/wechatpay",
            "http://localhost@evil.example/wechatpay",
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
    fn blank_primary_environment_value_uses_non_blank_fallback() {
        assert_eq!(
            select_env_value(Some("   ".to_string()), Some("fallback".to_string())).as_deref(),
            Some("fallback")
        );
        assert_eq!(
            select_env_value(Some("primary".to_string()), Some("fallback".to_string())).as_deref(),
            Some("primary")
        );
        assert_eq!(select_env_value(Some(String::new()), None), None);
    }

    #[test]
    fn malformed_timeout_is_rejected_instead_of_silently_defaulted() {
        assert_eq!(parse_timeout_minutes(None).unwrap(), 15);
        assert_eq!(parse_timeout_minutes(Some("30".to_string())).unwrap(), 30);
        assert!(parse_timeout_minutes(Some("soon".to_string())).is_err());
    }

    #[test]
    fn provider_timeout_has_a_bounded_range() {
        assert_eq!(MAX_TIMEOUT_MINUTES, 21_600);
        assert!(parse_timeout_minutes(Some("0".to_string())).is_ok());
        // Parsing and semantic range validation are intentionally separate.
        let mut config = WechatPayConfig {
            appid: "appid".to_string(),
            mchid: "mchid".to_string(),
            merchant_serial_no: "serial".to_string(),
            merchant_private_key: "private-key".to_string(),
            api_v3_key: "0123456789abcdef0123456789abcdef".to_string(),
            wechatpay_public_key_id: "public-key-id".to_string(),
            wechatpay_public_key: "public-key".to_string(),
            previous_callback_keys: Vec::new(),
            notify_url: "https://example.com/notify".to_string(),
            timeout_minutes: 15,
        };
        config.timeout_minutes = 0;
        assert!(config.validate().is_err());
        config.timeout_minutes = MAX_TIMEOUT_MINUTES;
        assert!(config.validate().is_ok());
        config.timeout_minutes = MAX_TIMEOUT_MINUTES + 1;
        assert!(config.validate().is_err());
    }

    #[test]
    fn previous_callback_keys_require_explicit_json() {
        assert!(parse_previous_callback_keys(None).unwrap().is_empty());
        let json = r#"[{"public_key_id":"old-id","public_key":"old-key","api_v3_key":"0123456789abcdef0123456789abcdef"}]"#;
        let keys = parse_previous_callback_keys(Some(json.to_string())).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].public_key, "old-key");
        assert!(parse_previous_callback_keys(Some("old-key".to_string())).is_err());
    }

    #[test]
    fn previous_callback_api_keys_are_validated() {
        let mut config = WechatPayConfig {
            appid: "appid".to_string(),
            mchid: "mchid".to_string(),
            merchant_serial_no: "serial".to_string(),
            merchant_private_key: "private-key".to_string(),
            api_v3_key: "0123456789abcdef0123456789abcdef".to_string(),
            wechatpay_public_key_id: "public-key-id".to_string(),
            wechatpay_public_key: "public-key".to_string(),
            previous_callback_keys: vec![WechatPayCallbackKey {
                public_key_id: "old-id".to_string(),
                public_key: "old-public-key".to_string(),
                api_v3_key: "too-short".to_string(),
            }],
            notify_url: "https://example.com/notify".to_string(),
            timeout_minutes: 15,
        };
        assert!(config.validate().is_err());
        config.previous_callback_keys[0].api_v3_key =
            "abcdef0123456789abcdef0123456789".to_string();
        assert!(config.validate().is_ok());
    }
}
