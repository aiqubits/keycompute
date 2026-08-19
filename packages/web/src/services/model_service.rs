//! 模型服务
//!
//! 获取系统支持的模型列表

use client_api::ClientConfig;
use client_api::error::Result;
use client_api::{OpenAiClient, api::openai::ModelListResponse};

use super::api_client::get_client;

/// 获取可用模型列表（无需认证）
///
/// `protocol` 为入口协议（openai / anthropic）：后端按协议过滤账号
/// 声明的模型，与入口协议隔离保持一致（OpenAI 兼容入口只列出
/// openai 协议模型，Anthropic 示例需显式传 anthropic）。
/// 注：protocol 取值受限于视图层固定值（openai/anthropic），直接拼接
/// query 无编码风险；web 为 WASM 包不引入 urlencoding 依赖。
pub async fn list_models(protocol: &str) -> Result<ModelListResponse> {
    let client = get_client();
    let base_url = client.config().base_url.clone();
    let openai_client = OpenAiClient::new(ClientConfig::new(base_url))?;
    // 使用空字符串作为 API key，因为后端允许匿名访问 /v1/models
    openai_client
        .get_json(&format!("/v1/models?protocol={protocol}"), "")
        .await
}
