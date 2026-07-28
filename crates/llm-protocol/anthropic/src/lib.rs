//! Anthropic 协议实现
//!
//! Anthropic Messages API 协议的适配器实现。
//! Claude 系列及其它 Anthropic 协议兼容上游均通过
//! 本协议 + Base URL + API Key 接入，不区分具体厂商。
//!
//! ## API 路径
//! `{base_url}/messages`（base_url 默认 https://api.anthropic.com/v1）
//!
//! ## 认证方式
//! 使用 `x-api-key` 头部（而非 Bearer Token），并携带 `anthropic-version` 头

pub mod adapter;
pub mod protocol;
pub mod stream;

pub use adapter::{ANTHROPIC_API_VERSION, AnthropicProvider};
pub use protocol::{
    AnthropicContent, AnthropicError, AnthropicMessage, AnthropicRequest, AnthropicResponse,
    AnthropicStreamEvent, AnthropicStreamMessage, AnthropicUsage, ContentBlock, ContentDelta,
    ImageSource, MessageDeltaInfo,
};
pub use stream::parse_anthropic_stream;

#[cfg(test)]
mod tests {
    use super::*;
    use llm_protocol_provider::ProviderAdapter;

    #[test]
    fn test_anthropic_provider_exports() {
        let provider = AnthropicProvider::new();
        assert_eq!(provider.name(), "anthropic");
    }
}
