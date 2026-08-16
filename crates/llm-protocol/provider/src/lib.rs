//! LLM Protocol Provider
//!
//! llm-protocol 体系的基础抽象层：
//! - `ProviderAdapter` trait：协议实现的统一上游调用接口
//! - `ProtocolType`：系统仅支持 openai / anthropic 两种协议
//! - 请求/流事件/HTTP 传输层类型

use async_trait::async_trait;
use futures::Stream;
use keycompute_types::{KeyComputeError, Result, SensitiveString};
use std::pin::Pin;

pub mod http;
pub mod protocol;
pub mod request;
pub mod stream;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use http::{ByteStream, DefaultHttpTransport, GetBinaryResponse, HttpTransport};
pub use protocol::{ProtocolType, normalize_base_url};
pub use request::{UpstreamMessage, UpstreamRequest};
pub use stream::StreamEvent;

/// Provider 适配器 trait
///
/// 所有 LLM Provider 必须实现此 trait，提供统一的上游调用接口
#[async_trait]
pub trait ProviderAdapter: Send + Sync + std::fmt::Debug {
    /// Provider 名称
    fn name(&self) -> &'static str;

    /// 支持的模型列表
    fn supported_models(&self) -> Vec<&'static str>;

    /// 检查是否支持指定模型
    fn supports_model(&self, model: &str) -> bool {
        self.supported_models().contains(&model)
    }

    /// 发起流式请求
    ///
    /// # 参数
    /// - `transport`: HTTP 传输层，用于发送请求
    /// - `request`: 上游请求
    async fn stream_chat(
        &self,
        transport: &dyn HttpTransport,
        request: UpstreamRequest,
    ) -> Result<StreamBox>;

    /// 非流式请求（默认通过 stream 实现）
    async fn chat(
        &self,
        transport: &dyn HttpTransport,
        request: UpstreamRequest,
    ) -> Result<String> {
        let mut stream = self.stream_chat(transport, request).await?;
        let mut content = String::new();

        use futures::StreamExt;
        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::Delta { content: delta, .. } => {
                    content.push_str(&delta);
                }
                StreamEvent::Done => break,
                StreamEvent::Error { message } => {
                    return Err(keycompute_types::KeyComputeError::ProviderError(message));
                }
                _ => {}
            }
        }

        Ok(content)
    }

    /// 是否支持图片生成（默认不支持）
    fn supports_image_generation(&self) -> bool {
        false
    }

    /// 是否支持图片编辑（默认不支持）
    fn supports_image_editing(&self) -> bool {
        false
    }

    /// 获取上游模型列表（兼作连通性验证）
    ///
    /// 默认实现为 OpenAI 风格：`GET {base}/models` + Bearer 认证。
    /// 非 Bearer 认证的协议（如 Anthropic 的 x-api-key）需覆盖此方法。
    /// 两种协议的响应均为 `{"data": [{"id": ...}]}` 结构。
    ///
    /// # 参数
    /// - `endpoint`: Base URL（不含路径，如 `https://api.openai.com/v1`）
    /// - `api_key`: 上游 API Key
    async fn list_models(
        &self,
        transport: &dyn HttpTransport,
        endpoint: &str,
        api_key: &SensitiveString,
    ) -> Result<Vec<String>> {
        let url = format!("{}/models", endpoint.trim_end_matches('/'));
        let headers = vec![(
            "Authorization".to_string(),
            format!("Bearer {}", api_key.expose()),
        )];
        let response = transport.get_binary(&url, headers).await?;
        parse_models_response(&response.body)
    }

    /// 验证上游 API Key 连通性（用于渠道账号测试）
    ///
    /// 默认通过 `list_models` 实现，协议实现只需覆盖 `list_models`。
    async fn verify_key(
        &self,
        transport: &dyn HttpTransport,
        endpoint: &str,
        api_key: &SensitiveString,
    ) -> Result<()> {
        self.list_models(transport, endpoint, api_key)
            .await
            .map(|_| ())
    }
}

/// 解析上游 `/models` 接口响应，提取模型 ID 列表
///
/// openai 与 anthropic 协议的响应均为 `{"data": [{"id": ...}]}` 结构。
///
/// 合法的空列表 `{"data": []}` 表示连接正常但当前账号没有可列出的模型；
/// 非法 JSON 或不符合该结构的响应则说明请求未到达兼容的 `/models` 端点，
/// 必须令渠道连接测试失败，不能误报成功。
pub fn parse_models_response(body: &[u8]) -> Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct ModelsResponse {
        data: Vec<Model>,
    }

    #[derive(serde::Deserialize)]
    struct Model {
        id: String,
    }

    let response: ModelsResponse = serde_json::from_slice(body).map_err(|_| {
        KeyComputeError::ProviderError(
            "Invalid /models response: expected an object with a data array of model IDs"
                .to_string(),
        )
    })?;

    Ok(response.data.into_iter().map(|model| model.id).collect())
}

/// 流返回类型
pub type StreamBox = Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RecordingGetTransport;

    #[derive(Debug)]
    struct DefaultModelListAdapter;

    #[async_trait::async_trait]
    impl ProviderAdapter for DefaultModelListAdapter {
        fn name(&self) -> &'static str {
            "test"
        }

        fn supported_models(&self) -> Vec<&'static str> {
            Vec::new()
        }

        async fn stream_chat(
            &self,
            _transport: &dyn HttpTransport,
            _request: UpstreamRequest,
        ) -> Result<StreamBox> {
            Err(KeyComputeError::ProviderError("not used".into()))
        }
    }

    #[test]
    fn test_stream_event_serialization() {
        let event = StreamEvent::Delta {
            content: "Hello".to_string(),
            finish_reason: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("Hello"));
    }

    #[test]
    fn test_parse_models_response_valid() {
        let body = br#"{"data": [{"id": "gpt-4o"}, {"id": "gpt-4o-mini"}]}"#;
        assert_eq!(
            parse_models_response(body).unwrap(),
            vec!["gpt-4o", "gpt-4o-mini"]
        );
    }

    #[test]
    fn test_parse_models_response_accepts_empty_model_list() {
        assert!(
            parse_models_response(br#"{"data": []}"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn test_parse_models_response_rejects_invalid_or_incompatible_responses() {
        for body in [
            b"not json".as_slice(),
            br#"{"data": "oops"}"#,
            br#"{"models": [{"id": "x"}]}"#,
            br#"{"data": [{"name": "a"}]}"#,
        ] {
            assert!(parse_models_response(body).is_err());
        }
    }

    #[test]
    fn default_list_models_propagates_invalid_response() {
        let transport = RecordingGetTransport::new(br#"{"data": "invalid"}"#.to_vec());
        let adapter = DefaultModelListAdapter;
        let api_key = SensitiveString::new("test-key");

        let error = futures::executor::block_on(adapter.list_models(
            &transport,
            "https://provider.example/v1/",
            &api_key,
        ))
        .unwrap_err();

        assert!(matches!(error, KeyComputeError::ProviderError(_)));
        assert_eq!(
            transport.requests(),
            vec![(
                "https://provider.example/v1/models".to_string(),
                vec![("Authorization".to_string(), "Bearer test-key".to_string())],
            )]
        );
    }
}
