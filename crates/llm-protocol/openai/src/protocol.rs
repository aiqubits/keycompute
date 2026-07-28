//! OpenAI API 协议类型
//!
//! OpenAI Chat Completions API 的请求/响应结构定义
//! 支持 Vision 多模态（图片理解）、图片生成、图片编辑

use keycompute_types::{ContentPart, MessageContent};
use serde::{Deserialize, Serialize};

// ============================================================================
// Chat Completions — 请求
// ============================================================================

/// OpenAI Chat Completions 请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIRequest {
    /// 模型名称
    pub model: String,
    /// 消息列表
    pub messages: Vec<OpenAIMessage>,
    /// 是否流式输出
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// 最大生成 token 数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// 温度参数 (0-2)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Top P 参数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// 停止序列
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    /// 是否返回用量信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
}

/// 流选项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamOptions {
    /// 在流式输出的最后一条消息中包含用量信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_usage: Option<bool>,
}

// ============================================================================
// Chat Completions — 消息（支持 Vision 多模态）
// ============================================================================

/// OpenAI 消息结构
/// 支持纯文本和 Vision 多模态内容
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIMessage {
    /// 角色: system, user, assistant, tool
    pub role: String,
    /// 消息内容：纯文本字符串或 Vision 内容块数组
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<OpenAIContent>,
    /// 工具调用（assistant 消息中）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// 工具调用 ID（tool 消息中）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// 名称（function 消息中）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// OpenAI 消息内容类型
/// 支持纯文本字符串和 Vision 内容块数组
///
/// 反序列化时拒绝空数组 `[]`，对齐 `MessageContent` 的行为。
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum OpenAIContent {
    /// 纯文本内容
    Text(String),
    /// Vision 内容块列表
    Parts(Vec<OpenAIContentPart>),
}

// 使用宏生成自定义 Deserialize 实现，拒绝空数组 []
keycompute_types::impl_untagged_content_deserialize!(
    OpenAIContent,
    OpenAIContentPart,
    "non-empty array of OpenAIContentPart"
);

/// OpenAI Vision 内容块
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OpenAIContentPart {
    /// 文本块
    #[serde(rename = "text")]
    Text { text: String },
    /// 图片 URL 块
    #[serde(rename = "image_url")]
    ImageUrl { image_url: OpenAIImageUrl },
}

/// OpenAI 图片 URL
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIImageUrl {
    /// 图片 URL（http/https 或 base64 data URI）
    pub url: String,
    /// 细节级别：low / high / auto
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 将 core 层的 MessageContent 转换为 OpenAI 的 OpenAIContent
///
/// 此函数覆盖所有 MessageContent 变体，始终返回确定的 OpenAIContent。
/// 空字符串保留为 Text("")，保持与 OpenAI API 的语义兼容。
pub fn convert_message_content(mc: MessageContent) -> OpenAIContent {
    match mc {
        MessageContent::Text(s) => OpenAIContent::Text(s),
        MessageContent::Parts(parts) => {
            let openai_parts: Vec<OpenAIContentPart> = parts
                .into_iter()
                .map(|p| match p {
                    ContentPart::Text { text } => OpenAIContentPart::Text { text },
                    ContentPart::ImageUrl { image_url } => OpenAIContentPart::ImageUrl {
                        image_url: OpenAIImageUrl {
                            url: image_url.url,
                            detail: image_url.detail,
                        },
                    },
                })
                .collect();
            OpenAIContent::Parts(openai_parts)
        }
    }
}

impl OpenAIContent {
    /// 返回纯文本引用（零拷贝），Parts 变体返回 None
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// 提取纯文本内容（Parts 变体拼接所有文本块）
    pub fn extract_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Parts(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    OpenAIContentPart::Text { text } => Some(text.as_str()),
                    OpenAIContentPart::ImageUrl { .. } => None,
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

// ============================================================================
// Chat Completions — 响应
// ============================================================================

/// OpenAI Chat Completions 响应（非流式）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Option<Usage>,
}

/// 选择结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub index: i32,
    pub message: OpenAIMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<serde_json::Value>,
}

/// 用量信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub total_tokens: i32,
}

impl OpenAIResponse {
    /// 返回纯文本引用（零拷贝），仅当首个 choice 为纯文本且非空时返回 Some
    pub fn text(&self) -> Option<&str> {
        self.choices
            .first()
            .and_then(|c| c.message.content.as_ref())
            .and_then(|c| c.text())
    }

    /// 提取文本内容（返回 String 以正确处理多模态 Parts）
    pub fn extract_text(&self) -> String {
        self.choices
            .first()
            .and_then(|c| c.message.content.as_ref())
            .map(|c| c.extract_text())
            .unwrap_or_default()
    }

    /// 提取 finish_reason
    pub fn extract_finish_reason(&self) -> Option<String> {
        self.choices.first().and_then(|c| c.finish_reason.clone())
    }
}

impl OpenAIMessage {
    /// 获取纯文本引用（零拷贝）
    pub fn text(&self) -> Option<&str> {
        self.content.as_ref().and_then(|c| c.text())
    }

    /// 获取消息内容（返回 String 以正确处理多模态 Parts）
    pub fn content(&self) -> String {
        self.content
            .as_ref()
            .map(|c| c.extract_text())
            .unwrap_or_default()
    }
}

// ============================================================================
// Chat Completions — 流式响应
// ============================================================================

/// OpenAI 流式响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIStreamResponse {
    // 元信息字段全部宽容解析：部分 OpenAI 兼容上游（vLLM/中转代理）
    // 的 chunk 会缺失其中某些字段，不应因此中断整条流
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub model: String,
    /// 部分上游的 usage-only 末块会省略 choices 字段
    #[serde(default)]
    pub choices: Vec<StreamChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// 流式选择结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChoice {
    #[serde(default)]
    pub index: i32,
    #[serde(default)]
    pub delta: DeltaMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<serde_json::Value>,
}

/// Delta 消息（流式增量）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeltaMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

// ============================================================================
// 工具调用
// ============================================================================

/// 工具调用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub r#type: String,
    pub function: FunctionCall,
}

/// 函数调用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

// ============================================================================
// 图片生成 API
// POST https://api.openai.com/v1/images/generations
// ============================================================================

/// 图片生成请求
/// 参考: https://platform.openai.com/docs/api-reference/images/generate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageGenerationRequest {
    /// 图片描述提示词（必需，最大 4000 字符 for dall-e-3）
    pub prompt: String,
    /// 模型名称（如 dall-e-3, dall-e-2）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 生成图片数量（1-10，dall-e-3 仅支持 n=1）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    /// 图片质量：standard / hd（仅 dall-e-3）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// 响应格式：url / b64_json
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,
    /// 图片尺寸：256x256, 512x512, 1024x1024 (dall-e-2),
    /// 1024x1024, 1792x1024, 1024x1792 (dall-e-3)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// 图片风格：vivid / natural（仅 dall-e-3）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// 用户标识（用于滥用监控）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

impl ImageGenerationRequest {
    /// 创建图片生成请求
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            model: None,
            n: None,
            quality: None,
            response_format: None,
            size: None,
            style: None,
            user: None,
        }
    }

    /// 设置模型
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// 设置生成数量
    pub fn with_n(mut self, n: u32) -> Self {
        self.n = Some(n);
        self
    }

    /// 设置图片质量
    pub fn with_quality(mut self, quality: impl Into<String>) -> Self {
        self.quality = Some(quality.into());
        self
    }

    /// 设置响应格式
    pub fn with_response_format(mut self, format: impl Into<String>) -> Self {
        self.response_format = Some(format.into());
        self
    }

    /// 设置图片尺寸
    pub fn with_size(mut self, size: impl Into<String>) -> Self {
        self.size = Some(size.into());
        self
    }
}

/// 图片生成响应
/// 参考: https://platform.openai.com/docs/api-reference/images/object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageGenerationResponse {
    /// 创建时间戳（Unix）
    pub created: i64,
    /// 图片数据列表
    pub data: Vec<ImageData>,
}

/// 图片数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageData {
    /// 图片 URL（当 response_format 为 url 时）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Base64 编码的图片数据（当 response_format 为 b64_json 时）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub b64_json: Option<String>,
    /// 修订后的提示词（仅 dall-e-3）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revised_prompt: Option<String>,
}

impl ImageGenerationResponse {
    /// 提取所有图片 URL
    pub fn image_urls(&self) -> Vec<&str> {
        self.data.iter().filter_map(|d| d.url.as_deref()).collect()
    }

    /// 提取所有 base64 数据
    pub fn b64_images(&self) -> Vec<&str> {
        self.data
            .iter()
            .filter_map(|d| d.b64_json.as_deref())
            .collect()
    }
}

// ============================================================================
// 图片编辑 API
// POST https://api.openai.com/v1/images/edits
// ============================================================================

/// 图片编辑请求
/// 参考: https://platform.openai.com/docs/api-reference/images/edit
///
/// 注意：OpenAI /v1/images/edits 使用 multipart/form-data，
/// image/mask 字段在发送时作为文件二进制数据，不做 JSON 序列化。
#[derive(Debug, Clone)]
pub struct ImageEditRequest {
    /// 编辑提示词（描述如何修改图片）
    pub prompt: String,
    /// 原始图片字节数据
    pub image: Vec<u8>,
    /// 图片文件名（用于 multipart Content-Disposition）
    pub image_filename: String,
    /// 图片 MIME 类型（如 image/png）
    pub image_content_type: String,
    /// 遮罩图片字节数据（透明区域表示需要编辑的部分）
    pub mask: Option<Vec<u8>>,
    /// 遮罩文件名
    pub mask_filename: Option<String>,
    /// 遮罩 MIME 类型
    pub mask_content_type: Option<String>,
    /// 模型名称（仅 dall-e-2）
    pub model: Option<String>,
    /// 生成图片数量（1-10）
    pub n: Option<u32>,
    /// 图片尺寸：256x256, 512x512, 1024x1024
    pub size: Option<String>,
    /// 响应格式：url / b64_json
    pub response_format: Option<String>,
    /// 用户标识
    pub user: Option<String>,
}

impl ImageEditRequest {
    /// 创建图片编辑请求
    pub fn new(
        prompt: impl Into<String>,
        image: Vec<u8>,
        image_filename: impl Into<String>,
        image_content_type: impl Into<String>,
    ) -> Self {
        Self {
            prompt: prompt.into(),
            image,
            image_filename: image_filename.into(),
            image_content_type: image_content_type.into(),
            mask: None,
            mask_filename: None,
            mask_content_type: None,
            model: None,
            n: None,
            size: None,
            response_format: None,
            user: None,
        }
    }

    /// 设置遮罩
    pub fn with_mask(
        mut self,
        mask: Vec<u8>,
        filename: impl Into<String>,
        content_type: impl Into<String>,
    ) -> Self {
        self.mask = Some(mask);
        self.mask_filename = Some(filename.into());
        self.mask_content_type = Some(content_type.into());
        self
    }
}

// ============================================================================
// 图片变体 API
// POST https://api.openai.com/v1/images/variations
// ============================================================================

/// 图片变体请求
/// 参考: https://platform.openai.com/docs/api-reference/images/variations
///
/// 注意：OpenAI /v1/images/variations 使用 multipart/form-data，
/// image 字段在发送时作为文件二进制数据，不做 JSON 序列化。
#[derive(Debug, Clone)]
pub struct ImageVariationRequest {
    /// 原始图片字节数据
    pub image: Vec<u8>,
    /// 图片文件名（用于 multipart Content-Disposition）
    pub image_filename: String,
    /// 图片 MIME 类型（如 image/png）
    pub image_content_type: String,
    /// 模型名称（仅 dall-e-2）
    pub model: Option<String>,
    /// 生成图片数量（1-10）
    pub n: Option<u32>,
    /// 图片尺寸：256x256, 512x512, 1024x1024
    pub size: Option<String>,
    /// 响应格式：url / b64_json
    pub response_format: Option<String>,
    /// 用户标识
    pub user: Option<String>,
}

impl ImageVariationRequest {
    /// 创建图片变体请求
    pub fn new(
        image: Vec<u8>,
        image_filename: impl Into<String>,
        image_content_type: impl Into<String>,
    ) -> Self {
        Self {
            image,
            image_filename: image_filename.into(),
            image_content_type: image_content_type.into(),
            model: None,
            n: None,
            size: None,
            response_format: None,
            user: None,
        }
    }

    /// 设置模型
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// 设置生成数量
    pub fn with_n(mut self, n: u32) -> Self {
        self.n = Some(n);
        self
    }

    /// 设置图片尺寸
    pub fn with_size(mut self, size: impl Into<String>) -> Self {
        self.size = Some(size.into());
        self
    }

    /// 设置响应格式
    pub fn with_response_format(mut self, format: impl Into<String>) -> Self {
        self.response_format = Some(format.into());
        self
    }
}

// ============================================================================
// Responses API
// POST https://api.openai.com/v1/responses
// ============================================================================

/// Responses API 请求（OpenAI 新统一接口）
/// 参考: https://platform.openai.com/docs/api-reference/responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesRequest {
    /// 模型名称
    pub model: String,
    /// 输入内容：纯文本字符串或内容块数组
    pub input: ResponsesInput,
    /// 工具列表（如 image_generation, web_search 等）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ResponsesTool>>,
    /// 是否流式输出
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// 最大输出 token 数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// 温度参数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Top P 参数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// 指令（system prompt）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// Responses API 输入类型
///
/// 反序列化时拒绝空数组 `[]`，与 `MessageContent` / `OpenAIContent` 行为一致，
/// 避免多模态数据静默丢弃。
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum ResponsesInput {
    /// 纯文本输入
    Text(String),
    /// 多模态输入块列表
    Parts(Vec<ResponsesInputPart>),
}

// 使用宏生成自定义 Deserialize 实现，拒绝空数组 []
keycompute_types::impl_untagged_content_deserialize!(
    ResponsesInput,
    ResponsesInputPart,
    "non-empty array of ResponsesInputPart"
);

impl ResponsesInput {
    /// 从纯文本创建
    pub fn text(content: impl Into<String>) -> Self {
        Self::Text(content.into())
    }

    /// 提取纯文本内容
    pub fn extract_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Parts(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    ResponsesInputPart::InputText { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

impl From<String> for ResponsesInput {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

impl From<&str> for ResponsesInput {
    fn from(s: &str) -> Self {
        Self::Text(s.to_string())
    }
}

/// Responses API 输入内容块
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ResponsesInputPart {
    /// 文本输入
    #[serde(rename = "input_text")]
    InputText { text: String },
    /// 图片输入（URL 或 base64）
    #[serde(rename = "input_image")]
    InputImage { image_url: String },
}

/// Responses API 工具定义
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ResponsesTool {
    /// 图片生成工具
    #[serde(rename = "image_generation")]
    ImageGeneration,
    /// 网页搜索工具
    #[serde(rename = "web_search")]
    WebSearch,
    /// 文件搜索工具
    #[serde(rename = "file_search")]
    FileSearch { vector_store_ids: Vec<String> },
}

/// Responses API 响应（非流式）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesResponse {
    /// 响应 ID
    pub id: String,
    /// 对象类型
    pub object: String,
    /// 模型名称
    pub model: String,
    /// 输出列表
    pub output: Vec<ResponsesOutputItem>,
    /// 用量信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ResponsesUsage>,
}

/// Responses API 输出项
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ResponsesOutputItem {
    /// 消息输出
    #[serde(rename = "message")]
    Message {
        id: String,
        role: String,
        content: Vec<ResponsesOutputContent>,
    },
    /// 图片生成调用
    #[serde(rename = "image_generation_call")]
    ImageGenerationCall {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        image_url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        b64_json: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        revised_prompt: Option<String>,
    },
}

/// Responses API 输出内容
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ResponsesOutputContent {
    /// 文本输出
    #[serde(rename = "output_text")]
    OutputText { text: String },
    /// 拒绝输出
    #[serde(rename = "refusal")]
    Refusal { refusal: String },
}

/// Responses API 用量信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesUsage {
    /// 输入 token 数
    pub input_tokens: u32,
    /// 输出 token 数
    pub output_tokens: u32,
    /// 总 token 数
    pub total_tokens: u32,
}

impl ResponsesRequest {
    /// 创建新的 Responses 请求
    pub fn new(model: impl Into<String>, input: impl Into<ResponsesInput>) -> Self {
        Self {
            model: model.into(),
            input: input.into(),
            tools: None,
            stream: None,
            max_output_tokens: None,
            temperature: None,
            top_p: None,
            instructions: None,
        }
    }

    /// 设置流式输出
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = Some(stream);
        self
    }

    /// 添加工具
    pub fn with_tools(mut self, tools: Vec<ResponsesTool>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// 添加图片生成工具
    pub fn with_image_generation(mut self) -> Self {
        self.tools = Some(vec![ResponsesTool::ImageGeneration]);
        self
    }

    /// 设置最大输出 token 数
    pub fn with_max_output_tokens(mut self, max_tokens: u32) -> Self {
        self.max_output_tokens = Some(max_tokens);
        self
    }

    /// 设置指令
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }
}

impl ResponsesResponse {
    /// 提取所有文本输出
    pub fn extract_text(&self) -> String {
        self.output
            .iter()
            .filter_map(|item| match item {
                ResponsesOutputItem::Message { content, .. } => Some(
                    content
                        .iter()
                        .filter_map(|c| match c {
                            ResponsesOutputContent::OutputText { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(""),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// 提取所有图片 URL
    pub fn image_urls(&self) -> Vec<&str> {
        self.output
            .iter()
            .filter_map(|item| match item {
                ResponsesOutputItem::ImageGenerationCall { image_url, .. } => image_url.as_deref(),
                _ => None,
            })
            .collect()
    }

    /// 提取所有 base64 图片数据
    pub fn b64_images(&self) -> Vec<&str> {
        self.output
            .iter()
            .filter_map(|item| match item {
                ResponsesOutputItem::ImageGenerationCall { b64_json, .. } => b64_json.as_deref(),
                _ => None,
            })
            .collect()
    }
}

impl OpenAIRequest {
    /// 创建新的请求
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            messages: Vec::new(),
            stream: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            stop: None,
            stream_options: None,
        }
    }

    /// 添加纯文本消息
    pub fn add_message(mut self, role: impl Into<String>, content: impl Into<String>) -> Self {
        self.messages.push(OpenAIMessage {
            role: role.into(),
            content: Some(OpenAIContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });
        self
    }

    /// 设置流式输出
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = Some(stream);
        self
    }

    /// 设置最大 token 数
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// 设置温度参数
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// 启用流式用量统计
    pub fn with_usage_in_stream(mut self) -> Self {
        self.stream_options = Some(StreamOptions {
            include_usage: Some(true),
        });
        self
    }
}

impl OpenAIMessage {
    /// 创建系统消息
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(OpenAIContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// 创建用户消息
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(OpenAIContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// 创建助手消息
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: Some(OpenAIContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use keycompute_types::{ContentPart as KtContentPart, ImageUrl as KtImageUrl, MessageContent};

    #[test]
    fn test_openai_request_serialization() {
        let request = OpenAIRequest::new("gpt-4o")
            .add_message("system", "You are helpful")
            .add_message("user", "Hello")
            .with_stream(true)
            .with_max_tokens(100)
            .with_temperature(0.7);

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("gpt-4o"));
        assert!(json.contains("You are helpful"));
        assert!(json.contains("true"));
    }

    #[test]
    fn test_vision_message_serialization() {
        let parts = vec![
            OpenAIContentPart::Text {
                text: "What's in this image?".to_string(),
            },
            OpenAIContentPart::ImageUrl {
                image_url: OpenAIImageUrl {
                    url: "https://example.com/image.png".to_string(),
                    detail: Some("high".to_string()),
                },
            },
        ];

        let msg = OpenAIMessage {
            role: "user".to_string(),
            content: Some(OpenAIContent::Parts(parts)),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("image_url"));
        assert!(json.contains("https://example.com/image.png"));
        assert!(json.contains("high"));
    }

    #[test]
    fn test_vision_message_deserialization() {
        let json = r#"{
            "role": "user",
            "content": [
                {"type": "text", "text": "Describe this image"},
                {"type": "image_url", "image_url": {"url": "https://example.com/photo.jpg"}}
            ]
        }"#;

        let msg: OpenAIMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.role, "user");
        assert!(msg.content.is_some());
        match msg.content.unwrap() {
            OpenAIContent::Parts(parts) => {
                assert_eq!(parts.len(), 2);
            }
            _ => panic!("Expected Parts"),
        }
    }

    #[test]
    fn test_text_message_serialization() {
        let msg = OpenAIMessage::user("Hello");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"content\":\"Hello\""));
    }

    #[test]
    fn test_message_content_from_core_types_text() {
        let mc = MessageContent::text("Hello world");
        let oc = convert_message_content(mc);
        assert_eq!(oc.extract_text(), "Hello world");
    }

    #[test]
    fn test_message_content_from_core_types_vision() {
        let parts = vec![
            KtContentPart::Text {
                text: "What is this?".to_string(),
            },
            KtContentPart::ImageUrl {
                image_url: KtImageUrl {
                    url: "https://example.com/img.png".to_string(),
                    detail: Some("auto".to_string()),
                },
            },
        ];
        let mc = MessageContent::Parts(parts);
        let oc = convert_message_content(mc);
        assert_eq!(oc.extract_text(), "What is this?");
    }

    #[test]
    fn test_image_generation_request_serialization() {
        let req = ImageGenerationRequest::new("A cute cat")
            .with_model("dall-e-3")
            .with_n(1)
            .with_size("1024x1024")
            .with_quality("hd");

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("A cute cat"));
        assert!(json.contains("dall-e-3"));
        assert!(json.contains("1024x1024"));
    }

    #[test]
    fn test_image_generation_response_deserialization() {
        let json = r#"{
            "created": 1589478378,
            "data": [
                {"url": "https://example.com/img1.png"},
                {"url": "https://example.com/img2.png"}
            ]
        }"#;
        let resp: ImageGenerationResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.image_urls().len(), 2);
    }

    #[test]
    fn test_image_edit_request_builder() {
        let image_bytes = b"fake-png-data";
        let mask_bytes = b"fake-mask-data";
        let req = ImageEditRequest::new(
            "Add a hat to the cat",
            image_bytes.to_vec(),
            "cat.png",
            "image/png",
        )
        .with_mask(mask_bytes.to_vec(), "mask.png", "image/png");

        assert_eq!(req.prompt, "Add a hat to the cat");
        assert_eq!(req.image, image_bytes);
        assert_eq!(req.image_filename, "cat.png");
        assert_eq!(req.image_content_type, "image/png");
        assert!(req.mask.is_some());
        assert_eq!(req.mask.unwrap(), mask_bytes);
    }

    // ========================================================================
    // Image Variations 测试
    // ========================================================================

    #[test]
    fn test_image_variation_request_builder() {
        let image_bytes = b"fake-png-data";
        let req = ImageVariationRequest::new(image_bytes.to_vec(), "source.png", "image/png")
            .with_model("dall-e-2")
            .with_n(2)
            .with_size("512x512");

        assert_eq!(req.image, image_bytes);
        assert_eq!(req.image_filename, "source.png");
        assert_eq!(req.image_content_type, "image/png");
        assert_eq!(req.model, Some("dall-e-2".to_string()));
        assert_eq!(req.n, Some(2));
        assert_eq!(req.size, Some("512x512".to_string()));
    }

    #[test]
    fn test_image_variation_response_deserialization() {
        let json = r#"{
            "created": 1589478378,
            "data": [
                {"url": "https://example.com/variant1.png"},
                {"url": "https://example.com/variant2.png"}
            ]
        }"#;
        let resp: ImageGenerationResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.image_urls().len(), 2);
    }

    // ========================================================================
    // Responses API 测试
    // ========================================================================

    #[test]
    fn test_responses_request_text_serialization() {
        let req = ResponsesRequest::new("gpt-4o", ResponsesInput::text("Hello, world!"))
            .with_instructions("You are helpful")
            .with_max_output_tokens(100);

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("gpt-4o"));
        assert!(json.contains("Hello, world!"));
        assert!(json.contains("You are helpful"));
    }

    #[test]
    fn test_responses_request_vision_serialization() {
        let parts = vec![
            ResponsesInputPart::InputText {
                text: "What's in this image?".to_string(),
            },
            ResponsesInputPart::InputImage {
                image_url: "https://example.com/photo.jpg".to_string(),
            },
        ];
        let req =
            ResponsesRequest::new("gpt-4o", ResponsesInput::Parts(parts)).with_image_generation();

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("input_image"));
        assert!(json.contains("image_generation"));
        assert!(json.contains("https://example.com/photo.jpg"));
    }

    #[test]
    fn test_responses_request_vision_deserialization() {
        let json = r#"{
            "model": "gpt-4o",
            "input": [
                {"type": "input_text", "text": "Describe this image"},
                {"type": "input_image", "image_url": "https://example.com/img.jpg"}
            ],
            "tools": [{"type": "image_generation"}]
        }"#;
        let req: ResponsesRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "gpt-4o");
        assert!(req.tools.is_some());

        // 验证 input 是 Parts 变体
        match &req.input {
            ResponsesInput::Parts(parts) => {
                assert_eq!(parts.len(), 2);
            }
            _ => panic!("Expected Parts input"),
        }
    }

    #[test]
    fn test_responses_response_deserialization_message() {
        let json = r#"{
            "id": "resp_abc123",
            "object": "response",
            "model": "gpt-4o",
            "output": [
                {
                    "type": "message",
                    "id": "msg_001",
                    "role": "assistant",
                    "content": [
                        {"type": "output_text", "text": "Hello! How can I help?"}
                    ]
                }
            ],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 20,
                "total_tokens": 30
            }
        }"#;
        let resp: ResponsesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, "resp_abc123");
        assert_eq!(resp.extract_text(), "Hello! How can I help?");
        assert!(resp.usage.is_some());
        assert_eq!(resp.usage.unwrap().total_tokens, 30);
    }

    #[test]
    fn test_responses_response_deserialization_image() {
        let json = r#"{
            "id": "resp_xyz",
            "object": "response",
            "model": "gpt-4o",
            "output": [
                {
                    "type": "message",
                    "id": "msg_001",
                    "role": "assistant",
                    "content": [
                        {"type": "output_text", "text": "Here is your image:"}
                    ]
                },
                {
                    "type": "image_generation_call",
                    "id": "ig_001",
                    "image_url": "https://example.com/generated.png",
                    "revised_prompt": "A beautiful sunset"
                }
            ]
        }"#;
        let resp: ResponsesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.image_urls(), vec!["https://example.com/generated.png"]);
        assert_eq!(resp.extract_text(), "Here is your image:");
    }

    #[test]
    fn test_responses_input_extract_text() {
        let input = ResponsesInput::text("Hello world");
        assert_eq!(input.extract_text(), "Hello world");

        let parts = ResponsesInput::Parts(vec![
            ResponsesInputPart::InputText {
                text: "Part 1".to_string(),
            },
            ResponsesInputPart::InputImage {
                image_url: "https://example.com/img.png".to_string(),
            },
            ResponsesInputPart::InputText {
                text: "Part 2".to_string(),
            },
        ]);
        assert_eq!(parts.extract_text(), "Part 1 Part 2");
    }

    // ========================================================================
    // 空数组拒绝测试
    // ========================================================================

    #[test]
    fn test_openai_content_rejects_empty_array() {
        let json = r#"{"role":"user","content":[]}"#;
        let result: Result<OpenAIMessage, _> = serde_json::from_str(json);
        assert!(result.is_err(), "Empty array [] should be rejected");
    }

    #[test]
    fn test_responses_input_rejects_empty_array() {
        let json = r#"{"model":"gpt-4o","input":[]}"#;
        let result: Result<ResponsesRequest, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "Empty array [] should be rejected for ResponsesInput"
        );
    }

    // ========================================================================
    // Roundtrip 测试（序列化 → 反序列化 → 验证一致性）
    // ========================================================================

    #[test]
    fn test_openai_content_roundtrip_text() {
        let original = OpenAIContent::Text("Hello world".to_string());
        let msg = OpenAIMessage {
            role: "user".to_string(),
            content: Some(original),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: OpenAIMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.role, "user");
        assert!(deserialized.content.is_some());
        assert_eq!(deserialized.content.unwrap().extract_text(), "Hello world");
    }

    #[test]
    fn test_openai_content_roundtrip_vision() {
        let parts = vec![
            OpenAIContentPart::Text {
                text: "Describe this".to_string(),
            },
            OpenAIContentPart::ImageUrl {
                image_url: OpenAIImageUrl {
                    url: "https://example.com/img.png".to_string(),
                    detail: Some("high".to_string()),
                },
            },
        ];
        let original = OpenAIContent::Parts(parts);
        let msg = OpenAIMessage {
            role: "user".to_string(),
            content: Some(original),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: OpenAIMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.role, "user");
        let roundtripped = deserialized.content.unwrap();
        assert_eq!(roundtripped.extract_text(), "Describe this");
    }

    #[test]
    fn test_responses_input_roundtrip_text() {
        let input = ResponsesInput::Text("Hello".to_string());
        let req = ResponsesRequest::new("gpt-4o", input);
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: ResponsesRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.model, "gpt-4o");
        assert_eq!(deserialized.input.extract_text(), "Hello");
    }

    #[test]
    fn test_responses_input_roundtrip_vision() {
        let parts = vec![
            ResponsesInputPart::InputText {
                text: "Analyze this".to_string(),
            },
            ResponsesInputPart::InputImage {
                image_url: "https://example.com/img.jpg".to_string(),
            },
        ];
        let input = ResponsesInput::Parts(parts);
        let req = ResponsesRequest::new("gpt-4o", input);
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: ResponsesRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.model, "gpt-4o");
        assert_eq!(deserialized.input.extract_text(), "Analyze this");
    }
}
