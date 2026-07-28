//! 协议适配器端到端测试
//!
//! 验证 openai / anthropic 两种协议适配器的协议转换和流处理。
//! 系统仅支持两种协议，任何厂商（DeepSeek、Ollama、vLLM 等）
//! 通过 `协议 + base_url + api_key` 接入，不区分具体厂商实现。

use integration_tests::common::VerificationChain;
use llm_protocol_anthropic::AnthropicProvider;
use llm_protocol_openai::OpenAIProvider;
use llm_protocol_provider::{
    ProtocolType, ProviderAdapter, StreamEvent, UpstreamRequest, normalize_base_url,
};

/// 测试 Provider trait 基础功能
#[test]
fn test_provider_trait_basics() {
    let mut chain = VerificationChain::new();

    // 1. 测试 UpstreamRequest 构建器（endpoint 为 Base URL，路径由协议层拼接）
    let request = UpstreamRequest::new("https://api.openai.com/v1", "sk-test-key", "gpt-4o")
        .with_message("system", "You are a helpful assistant")
        .with_message("user", "Hello")
        .with_stream(true)
        .with_max_tokens(1000)
        .with_temperature(0.7);

    chain.add_step(
        "llm-protocol-provider",
        "UpstreamRequest::builder",
        format!("Model: {}", request.model),
        request.model == "gpt-4o",
    );
    chain.add_step(
        "llm-protocol-provider",
        "UpstreamRequest::messages",
        format!("Message count: {}", request.messages.len()),
        request.messages.len() == 2,
    );
    chain.add_step(
        "llm-protocol-provider",
        "UpstreamRequest::stream",
        format!("Stream enabled: {}", request.stream),
        request.stream,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试 StreamEvent 类型
#[test]
fn test_provider_stream_events() {
    let mut chain = VerificationChain::new();

    // 1. Delta 事件
    let delta = StreamEvent::Delta {
        content: "Hello".to_string(),
        finish_reason: None,
    };
    chain.add_step(
        "llm-protocol-provider",
        "StreamEvent::Delta",
        "Delta event created",
        matches!(delta, StreamEvent::Delta { .. }),
    );

    // 2. Usage 事件
    let usage = StreamEvent::Usage {
        input_tokens: 100,
        output_tokens: 50,
    };
    chain.add_step(
        "llm-protocol-provider",
        "StreamEvent::Usage",
        "Usage event created",
        matches!(usage, StreamEvent::Usage { .. }),
    );

    // 3. Done 事件
    let done = StreamEvent::Done;
    chain.add_step(
        "llm-protocol-provider",
        "StreamEvent::Done",
        "Done event created",
        matches!(done, StreamEvent::Done),
    );

    // 4. Error 事件
    let error = StreamEvent::Error {
        message: "Test error".to_string(),
    };
    chain.add_step(
        "llm-protocol-provider",
        "StreamEvent::Error",
        "Error event created",
        matches!(error, StreamEvent::Error { .. }),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试 ProtocolType 枚举
#[test]
fn test_protocol_type() {
    let mut chain = VerificationChain::new();

    // 1. 仅支持两种协议
    chain.add_step(
        "llm-protocol-provider",
        "ProtocolType::ALL",
        format!("Protocol count: {}", ProtocolType::ALL.len()),
        ProtocolType::ALL.len() == 2,
    );

    // 2. 协议名解析（大小写不敏感）
    chain.add_step(
        "llm-protocol-provider",
        "ProtocolType::parse_openai",
        "openai parsed",
        ProtocolType::parse("openai") == Some(ProtocolType::Openai)
            && ProtocolType::parse("OpenAI") == Some(ProtocolType::Openai),
    );
    chain.add_step(
        "llm-protocol-provider",
        "ProtocolType::parse_anthropic",
        "anthropic parsed",
        ProtocolType::parse("anthropic") == Some(ProtocolType::Anthropic),
    );

    // 3. 厂商名不是合法协议（厂商通过协议 + base_url 接入）
    chain.add_step(
        "llm-protocol-provider",
        "ProtocolType::parse_vendor_rejected",
        "vendor names rejected",
        ProtocolType::parse("deepseek").is_none()
            && ProtocolType::parse("ollama").is_none()
            && ProtocolType::parse("claude").is_none()
            && ProtocolType::parse("gemini").is_none(),
    );

    // 4. 默认端点
    chain.add_step(
        "llm-protocol-provider",
        "ProtocolType::default_endpoint",
        "default endpoints",
        ProtocolType::Openai.default_endpoint() == "https://api.openai.com/v1"
            && ProtocolType::Anthropic.default_endpoint() == "https://api.anthropic.com/v1",
    );

    // 5. Base URL 规范化：拒绝带协议路径的输入
    chain.add_step(
        "llm-protocol-provider",
        "normalize_base_url",
        "base url normalization",
        normalize_base_url("https://api.deepseek.com/v1/").as_deref()
            == Ok("https://api.deepseek.com/v1")
            && normalize_base_url("https://x.com/v1/chat/completions").is_err()
            && normalize_base_url("https://x.com/v1/messages").is_err(),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试 UpstreamMessage 辅助函数
#[test]
fn test_provider_upstream_message() {
    use llm_protocol_provider::UpstreamMessage;

    let mut chain = VerificationChain::new();

    // 1. 创建系统消息
    let sys = UpstreamMessage::system("You are helpful");
    chain.add_step(
        "llm-protocol-provider",
        "UpstreamMessage::system",
        format!("Role: {}", sys.role),
        sys.role == "system",
    );

    // 2. 创建用户消息
    let user = UpstreamMessage::user("Hello");
    chain.add_step(
        "llm-protocol-provider",
        "UpstreamMessage::user",
        format!("Role: {}", user.role),
        user.role == "user",
    );

    // 3. 创建助手消息
    let assistant = UpstreamMessage::assistant("Hi there");
    chain.add_step(
        "llm-protocol-provider",
        "UpstreamMessage::assistant",
        format!("Role: {}", assistant.role),
        assistant.role == "assistant",
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试 OpenAI 协议适配器
#[test]
fn test_protocol_openai() {
    let mut chain = VerificationChain::new();

    // 1. 创建 OpenAI 协议适配器
    let provider = OpenAIProvider::new();
    chain.add_step(
        "llm-protocol-openai",
        "OpenAIProvider::new",
        "OpenAI protocol adapter created",
        true,
    );

    // 2. 检查名称（与 ProtocolType::as_str / DB accounts.provider 一致）
    let name = provider.name();
    chain.add_step(
        "llm-protocol-openai",
        "OpenAIProvider::name",
        format!("Protocol name: {}", name),
        name == "openai",
    );

    // 3. 协议层不维护模型白名单
    let models = provider.supported_models();
    chain.add_step(
        "llm-protocol-openai",
        "OpenAIProvider::supported_models",
        format!("Supported models: {:?}", models),
        models.is_empty(),
    );

    // 4. 协议层接受任意模型（模型由账号 models_supported 声明，路由层过滤）
    chain.add_step(
        "llm-protocol-openai",
        "OpenAIProvider::supports_any_model",
        "accepts any model",
        provider.supports_model("gpt-4o")
            && provider.supports_model("deepseek-chat")
            && provider.supports_model("llama3.2")
            && provider.supports_model("Qwen/Qwen2.5-7B-Instruct"),
    );

    // 5. OpenAI 协议支持图片生成/编辑
    chain.add_step(
        "llm-protocol-openai",
        "OpenAIProvider::image_capabilities",
        "image generation/editing supported",
        provider.supports_image_generation() && provider.supports_image_editing(),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试 Anthropic 协议适配器
#[test]
fn test_protocol_anthropic() {
    let mut chain = VerificationChain::new();

    // 1. 创建 Anthropic 协议适配器
    let provider = AnthropicProvider::new();
    chain.add_step(
        "llm-protocol-anthropic",
        "AnthropicProvider::new",
        "Anthropic protocol adapter created",
        true,
    );

    // 2. 检查名称（与 ProtocolType::as_str / DB accounts.provider 一致）
    let name = provider.name();
    chain.add_step(
        "llm-protocol-anthropic",
        "AnthropicProvider::name",
        format!("Protocol name: {}", name),
        name == "anthropic",
    );

    // 3. 协议层不维护模型白名单
    let models = provider.supported_models();
    chain.add_step(
        "llm-protocol-anthropic",
        "AnthropicProvider::supported_models",
        format!("Supported models count: {}", models.len()),
        models.is_empty(),
    );

    // 4. 协议层接受任意模型
    chain.add_step(
        "llm-protocol-anthropic",
        "AnthropicProvider::supports_any_model",
        "accepts any model",
        provider.supports_model("claude-3-5-sonnet-20241022")
            && provider.supports_model("claude-opus-4"),
    );

    // 5. API 版本头常量
    chain.add_step(
        "llm-protocol-anthropic",
        "ANTHROPIC_API_VERSION",
        format!("Version: {}", llm_protocol_anthropic::ANTHROPIC_API_VERSION),
        llm_protocol_anthropic::ANTHROPIC_API_VERSION == "2023-06-01",
    );

    chain.print_report();
    assert!(chain.all_passed());
}
