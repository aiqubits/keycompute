//! KeyCompute 全局共享类型定义
//!
//! 本 crate 包含所有后端 crate 共享的核心类型，无任何业务逻辑，
//! 仅用于类型定义和数据结构。

pub mod error;
pub mod execution_plan;
pub mod node;
pub mod pricing;
pub mod request;
pub mod response;
pub mod usage;
pub mod user;

// 重新导出最常用的类型
pub use error::{ErrorCategory, KeyComputeError, Result};
pub use execution_plan::{ExecutionPlan, ExecutionTarget, SensitiveString};
pub use node::{
    ImageData, ImageEditRequest, ImageGenerationRequest, ImageGenerationResponse, NodeCapabilities,
    NodeHeartbeatRequest, NodeHeartbeatResponse, NodeId, NodeLeaseId, NodeModelCapability,
    NodePollRequest, NodePollResponse, NodeRegisterRequest, NodeRegisterResponse, NodeSessionId,
    NodeTaskCompleteAction, NodeTaskCompleteRequest, NodeTaskCompleteResponse, NodeTaskEnvelope,
    NodeTaskId, NodeTaskPayload, NodeTaskResult,
};
pub use pricing::PricingSnapshot;
pub use request::{
    ChatCompletionRequest, ContentPart, ExecutedProviderAccount, ImageUrl, Message, MessageContent,
    MessageRole, RequestContext,
};
pub use response::{
    ChatCompletionChunk, ChatCompletionResponse, Choice, ErrorResponse, MessageDelta, ModelInfo,
    ModelListResponse, Usage,
};
pub use usage::{UsageAccumulator, UsageRecord};
pub use user::{AssignableUserRole, UserRole};

/// 为 `#[serde(untagged)]` 枚举生成自定义 `Deserialize` 实现
///
/// 该枚举有两个变体：纯文本字符串 (`Text(String)`) 和多模态内容块数组 (`Parts(Vec<Part>)`)。
/// 自定义反序列化在遇到空数组 `[]` 时返回错误，避免多模态数据静默丢弃。
///
/// # 参数
/// - `$enum_ty`: 枚举类型名（如 `MessageContent`）
/// - `$part_ty`: Part 类型（如 `ContentPart`）
/// - `$err_desc`: 空数组时的错误描述字符串
///
/// # 示例
/// ```ignore
/// impl_untagged_content_deserialize!(MessageContent, ContentPart, "non-empty array of content parts");
/// ```
#[macro_export]
macro_rules! impl_untagged_content_deserialize {
    ($enum_ty:ty, $part_ty:ty, $err_desc:expr) => {
        impl<'de> serde::Deserialize<'de> for $enum_ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                use serde::de;

                struct ContentVisitor;

                impl<'de> de::Visitor<'de> for ContentVisitor {
                    type Value = $enum_ty;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                        formatter.write_str("a string or a non-empty array of content parts")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        Ok(<$enum_ty>::Text(value.to_string()))
                    }

                    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        Ok(<$enum_ty>::Text(value))
                    }

                    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
                    where
                        A: de::SeqAccess<'de>,
                    {
                        let mut parts = Vec::new();
                        while let Some(part) = seq.next_element::<$part_ty>()? {
                            parts.push(part);
                        }
                        if parts.is_empty() {
                            return Err(de::Error::invalid_value(de::Unexpected::Seq, &$err_desc));
                        }
                        Ok(<$enum_ty>::Parts(parts))
                    }
                }

                deserializer.deserialize_any(ContentVisitor)
            }
        }
    };
}
