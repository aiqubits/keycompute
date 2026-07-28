//! 协议类型定义
//!
//! 渠道账号只绑定协议（openai / anthropic），不区分具体厂商。
//! 任何厂商（DeepSeek、Ollama、vLLM、Gemini 等）通过
//! `协议 + base_url + api_key` 三元组接入。

use serde::{Deserialize, Serialize};

/// LLM 上游协议类型
///
/// 系统仅支持两种协议，协议名同时用作：
/// - DB `accounts.provider` 列的合法取值
/// - RoutingEngine 的 Provider 名称
/// - GatewayExecutor 的 adapter 注册键
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProtocolType {
    /// OpenAI Chat Completions 协议（OpenAI/DeepSeek/Ollama/vLLM/Gemini 兼容层等）
    Openai,
    /// Anthropic Messages 协议（Claude 等）
    Anthropic,
}

impl ProtocolType {
    /// 所有支持的协议
    pub const ALL: &'static [ProtocolType] = &[ProtocolType::Openai, ProtocolType::Anthropic];

    /// 协议名称（与 DB / 路由 / 注册键一致）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
        }
    }

    /// 解析协议名称（大小写不敏感）
    ///
    /// 用于创建渠道账号时校验 provider 字段
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "openai" => Some(Self::Openai),
            "anthropic" => Some(Self::Anthropic),
            _ => None,
        }
    }

    /// 协议默认 Base URL
    pub fn default_endpoint(&self) -> &'static str {
        match self {
            Self::Openai => "https://api.openai.com/v1",
            Self::Anthropic => "https://api.anthropic.com/v1",
        }
    }
}

impl std::fmt::Display for ProtocolType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for ProtocolType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| {
            format!(
                "Unsupported protocol '{}', expected one of: openai, anthropic",
                s
            )
        })
    }
}

/// 规范化 Base URL
///
/// - 去除尾部 `/`
/// - 要求 http(s) scheme（无 scheme 的输入会在运行期以相对 URL 报错，提前拒绝）
/// - 拒绝以协议路径结尾的输入（endpoint 只存 base URL，路径由协议层拼接）
pub fn normalize_base_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("Base URL cannot be empty".to_string());
    }
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return Err("Base URL must start with http:// or https://".to_string());
    }
    for suffix in ["/chat/completions", "/messages"] {
        if trimmed.ends_with(suffix) {
            return Err(format!(
                "Base URL must not include the API path '{}'; \
                 the protocol layer appends it automatically",
                suffix
            ));
        }
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_type_as_str() {
        assert_eq!(ProtocolType::Openai.as_str(), "openai");
        assert_eq!(ProtocolType::Anthropic.as_str(), "anthropic");
    }

    #[test]
    fn test_protocol_type_parse() {
        assert_eq!(ProtocolType::parse("openai"), Some(ProtocolType::Openai));
        assert_eq!(ProtocolType::parse("OpenAI"), Some(ProtocolType::Openai));
        assert_eq!(
            ProtocolType::parse("anthropic"),
            Some(ProtocolType::Anthropic)
        );
        assert_eq!(ProtocolType::parse("deepseek"), None);
        assert_eq!(ProtocolType::parse(""), None);
    }

    #[test]
    fn test_default_endpoint() {
        assert_eq!(
            ProtocolType::Openai.default_endpoint(),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            ProtocolType::Anthropic.default_endpoint(),
            "https://api.anthropic.com/v1"
        );
    }

    #[test]
    fn test_normalize_base_url() {
        assert_eq!(
            normalize_base_url("https://api.openai.com/v1/").unwrap(),
            "https://api.openai.com/v1"
        );
        assert!(normalize_base_url("https://x.com/v1/chat/completions").is_err());
        assert!(normalize_base_url("https://x.com/v1/messages").is_err());
        assert!(normalize_base_url("  ").is_err());
    }

    #[test]
    fn test_normalize_base_url_requires_http_scheme() {
        // 无 scheme / 非 http(s) scheme 会在运行期以相对 URL 报错，应提前拒绝
        assert!(normalize_base_url("api.openai.com/v1").is_err());
        assert!(normalize_base_url("ftp://api.openai.com/v1").is_err());
        assert_eq!(
            normalize_base_url("http://ollama:11434").unwrap(),
            "http://ollama:11434"
        );
    }

    #[test]
    fn test_normalize_base_url_without_v1_accepted() {
        // 有意行为：base URL 不强制包含 /v1，
        // 自建 vLLM/Ollama 网关可能不使用 /v1 前缀
        assert_eq!(normalize_base_url("https://host").unwrap(), "https://host");
        assert_eq!(
            normalize_base_url("http://localhost:11434").unwrap(),
            "http://localhost:11434"
        );
    }
}
