//! OpenAI 协议实现
//!
//! OpenAI Chat Completions 协议的适配器实现。
//! 所有 OpenAI 兼容上游（OpenAI/DeepSeek/Ollama/vLLM/Gemini 兼容层等）
//! 均通过本协议 + Base URL + API Key 接入，不区分具体厂商。

pub mod adapter;
pub mod protocol;
pub mod stream;

pub use adapter::{
    OPENAI_CHAT_ENDPOINT, OPENAI_IMAGE_EDIT_ENDPOINT, OPENAI_IMAGE_GEN_ENDPOINT,
    OPENAI_IMAGE_VARIATION_ENDPOINT, OPENAI_RESPONSES_ENDPOINT, OpenAIProvider,
};
pub use protocol::{
    ImageData, ImageEditRequest, ImageGenerationRequest, ImageGenerationResponse,
    ImageVariationRequest, OpenAIContent, OpenAIContentPart, OpenAIImageUrl, OpenAIMessage,
    OpenAIRequest, OpenAIResponse, OpenAIStreamResponse, ResponsesInput, ResponsesInputPart,
    ResponsesOutputContent, ResponsesOutputItem, ResponsesRequest, ResponsesResponse,
    ResponsesTool, ResponsesUsage, StreamOptions, convert_message_content,
};
