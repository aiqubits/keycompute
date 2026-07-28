//! HTTP Transport 层端到端测试
//!
//! 测试覆盖：
//! - MockHttpTransport 基础功能
//! - 代理配置测试
//! - 自定义 HTTP 客户端测试
//! - 各种响应模拟测试
//! - 流式响应测试
//! - Provider 与 MockHttpTransport 集成

use bytes::Bytes;
use futures::StreamExt;
use integration_tests::common::VerificationChain;
use integration_tests::mocks::http_transport::{
    MockHttpTransport, MockHttpTransportFactory, MockResponse, MockStreamConfig, ProxyConfig,
};
use integration_tests::mocks::provider::MockProviderFactory;
use llm_protocol_provider::{HttpTransport, ProviderAdapter, UpstreamRequest};
use std::time::Duration;

// ============================================================================
// 第一部分：MockHttpTransport 基础功能测试
// ============================================================================

/// 测试 MockHttpTransport 基础创建
#[tokio::test]
async fn test_mock_transport_basic() {
    let mut chain = VerificationChain::new();

    // 1. 创建基本 Mock Transport
    let transport = MockHttpTransport::new();
    chain.add_step(
        "integration-tests::mocks",
        "MockHttpTransport::new",
        "Basic mock transport created",
        true,
    );

    // 2. 验证默认超时
    chain.add_step(
        "llm-protocol-provider",
        "HttpTransport::request_timeout",
        format!("Default request timeout: {:?}", transport.request_timeout()),
        transport.request_timeout() == Duration::from_secs(120),
    );

    chain.add_step(
        "llm-protocol-provider",
        "HttpTransport::stream_timeout",
        format!("Default stream timeout: {:?}", transport.stream_timeout()),
        transport.stream_timeout() == Duration::from_secs(600),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试成功响应
#[tokio::test]
async fn test_mock_transport_success_response() {
    let mut chain = VerificationChain::new();

    // 1. 创建返回成功响应的 Transport
    let transport = MockHttpTransportFactory::success();
    chain.add_step(
        "integration-tests::mocks",
        "MockHttpTransportFactory::success",
        "Success transport created",
        true,
    );

    // 2. 发送请求
    let result = transport
        .post_json("http://test-api", vec![], r#"{"test": true}"#.to_string())
        .await;

    chain.add_step(
        "llm-protocol-provider",
        "HttpTransport::post_json",
        format!("Request succeeded: {}", result.is_ok()),
        result.is_ok(),
    );

    // 3. 验证响应内容
    if let Ok(body) = result {
        chain.add_step(
            "integration-tests::mocks",
            "MockTransport::response_body",
            format!("Response body: {}", body),
            body.contains("success"),
        );
    }

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试错误响应
#[tokio::test]
async fn test_mock_transport_error_response() {
    let mut chain = VerificationChain::new();

    // 1. 创建返回错误的 Transport
    let transport = MockHttpTransportFactory::error(500, "Internal server error");
    chain.add_step(
        "integration-tests::mocks",
        "MockHttpTransportFactory::error",
        "Error transport created",
        true,
    );

    // 2. 发送请求
    let result = transport
        .post_json("http://test-api", vec![], "{}".to_string())
        .await;

    chain.add_step(
        "integration-tests::mocks",
        "MockTransport::error_response",
        format!("Request failed: {}", result.is_err()),
        result.is_err(),
    );

    // 3. 验证错误内容
    if let Err(e) = result {
        chain.add_step(
            "integration-tests::mocks",
            "MockTransport::error_message",
            format!("Error message: {}", e),
            e.to_string().contains("500"),
        );
    }

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试请求历史记录
#[tokio::test]
async fn test_mock_transport_request_history() {
    let mut chain = VerificationChain::new();

    // 1. 创建 Transport
    let transport = MockHttpTransport::new();
    transport.add_response(MockResponse::ok("response1"));
    transport.add_response(MockResponse::ok("response2"));

    chain.add_step(
        "integration-tests::mocks",
        "MockHttpTransport::with_history",
        "Transport with request history created",
        true,
    );

    // 2. 发送多个请求
    let _ = transport
        .post_json(
            "http://url1",
            vec![("Auth".to_string(), "token1".to_string())],
            r#"{"request": 1}"#.to_string(),
        )
        .await;

    let _ = transport
        .post_json(
            "http://url2",
            vec![("Auth".to_string(), "token2".to_string())],
            r#"{"request": 2}"#.to_string(),
        )
        .await;

    // 3. 验证历史记录
    chain.add_step(
        "integration-tests::mocks",
        "MockTransport::request_count",
        format!("Request count: {}", transport.request_count()),
        transport.request_count() == 2,
    );

    let last = transport.last_request().unwrap();
    chain.add_step(
        "integration-tests::mocks",
        "MockTransport::last_request_url",
        format!("Last request URL: {}", last.url),
        last.url == "http://url2",
    );

    chain.add_step(
        "integration-tests::mocks",
        "MockTransport::last_request_body",
        format!("Last request body: {}", last.body),
        last.body.contains("request"),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

// ============================================================================
// 第二部分：代理配置测试
// ============================================================================

/// 测试代理配置创建
#[test]
fn test_proxy_config_creation() {
    let mut chain = VerificationChain::new();

    // 1. 创建代理配置
    let config = ProxyConfig::new()
        .http("http://proxy.example.com:8080")
        .https("https://proxy.example.com:8443");

    chain.add_step(
        "integration-tests::mocks",
        "ProxyConfig::http_proxy",
        format!("HTTP proxy: {:?}", config.http_proxy),
        config.http_proxy == Some("http://proxy.example.com:8080".to_string()),
    );

    chain.add_step(
        "integration-tests::mocks",
        "ProxyConfig::https_proxy",
        format!("HTTPS proxy: {:?}", config.https_proxy),
        config.https_proxy == Some("https://proxy.example.com:8443".to_string()),
    );

    chain.add_step(
        "integration-tests::mocks",
        "ProxyConfig::enabled",
        format!("Proxy enabled: {}", config.enabled),
        config.enabled,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试带认证的代理配置
#[test]
fn test_proxy_config_with_auth() {
    let mut chain = VerificationChain::new();

    // 1. 创建带认证的代理配置
    let config = ProxyConfig::new()
        .http("http://proxy.example.com:8080")
        .auth("proxy_user", "proxy_password");

    chain.add_step(
        "integration-tests::mocks",
        "ProxyConfig::username",
        format!("Proxy username: {:?}", config.username),
        config.username == Some("proxy_user".to_string()),
    );

    chain.add_step(
        "integration-tests::mocks",
        "ProxyConfig::password",
        format!("Proxy password: {:?}", config.password),
        config.password == Some("proxy_password".to_string()),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试带代理的 Mock Transport
#[tokio::test]
async fn test_mock_transport_with_proxy() {
    let mut chain = VerificationChain::new();

    // 1. 创建代理配置
    let proxy_config = ProxyConfig::new()
        .http("http://proxy.example.com:8080")
        .auth("user", "pass");

    // 2. 创建带代理的 Transport
    let transport = MockHttpTransportFactory::with_proxy(proxy_config.clone());
    chain.add_step(
        "integration-tests::mocks",
        "MockHttpTransportFactory::with_proxy",
        "Transport with proxy created",
        true,
    );

    // 3. 验证代理配置
    let stored_config = transport.proxy_config();
    chain.add_step(
        "integration-tests::mocks",
        "MockTransport::proxy_config",
        format!("Proxy enabled: {}", stored_config.enabled),
        stored_config.enabled,
    );

    chain.add_step(
        "integration-tests::mocks",
        "MockTransport::proxy_url",
        format!("HTTP proxy URL: {:?}", stored_config.http_proxy),
        stored_config.http_proxy == proxy_config.http_proxy,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试代理连接模拟
#[tokio::test]
async fn test_proxy_connection_simulation() {
    let mut chain = VerificationChain::new();

    // 1. 创建带代理的 Transport，模拟代理连接
    let proxy_config = ProxyConfig::new()
        .http("http://proxy.test:8080")
        .auth("proxy_user", "proxy_pass");

    let transport = MockHttpTransport::new()
        .with_proxy(proxy_config)
        .with_response(MockResponse::ok(r#"{"proxied": true}"#));

    chain.add_step(
        "integration-tests::mocks",
        "ProxySimulation::setup",
        "Proxy simulation setup complete",
        true,
    );

    // 2. 通过代理发送请求
    let result = transport
        .post_json("http://target-api", vec![], "{}".to_string())
        .await;

    chain.add_step(
        "integration-tests::mocks",
        "ProxySimulation::request_success",
        format!("Proxied request succeeded: {}", result.is_ok()),
        result.is_ok(),
    );

    // 3. 验证请求经过了代理（通过检查代理配置）
    chain.add_step(
        "integration-tests::mocks",
        "ProxySimulation::proxy_used",
        format!("Proxy was configured: {}", transport.proxy_config().enabled),
        transport.proxy_config().enabled,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

// ============================================================================
// 第三部分：各种响应模拟测试
// ============================================================================

/// 测试 HTTP 状态码响应
#[tokio::test]
async fn test_various_http_status_codes() {
    let mut chain = VerificationChain::new();

    // 1. 测试 200 OK
    let transport = MockHttpTransportFactory::success();
    let result = transport
        .post_json("http://test", vec![], "{}".to_string())
        .await;
    chain.add_step(
        "integration-tests::mocks",
        "HttpStatus::200_ok",
        format!("200 OK: {}", result.is_ok()),
        result.is_ok(),
    );

    // 2. 测试 401 Unauthorized
    let transport = MockHttpTransportFactory::unauthorized();
    let result = transport
        .post_json("http://test", vec![], "{}".to_string())
        .await;
    chain.add_step(
        "integration-tests::mocks",
        "HttpStatus::401_unauthorized",
        format!("401 error: {}", result.is_err()),
        result.is_err(),
    );
    if let Err(e) = result {
        chain.add_step(
            "integration-tests::mocks",
            "HttpStatus::401_message",
            format!("Error contains 401: {}", e.to_string().contains("401")),
            e.to_string().contains("401"),
        );
    }

    // 3. 测试 429 Rate Limited
    let transport = MockHttpTransportFactory::rate_limited(60);
    let result = transport
        .post_json("http://test", vec![], "{}".to_string())
        .await;
    chain.add_step(
        "integration-tests::mocks",
        "HttpStatus::429_rate_limited",
        format!("429 error: {}", result.is_err()),
        result.is_err(),
    );

    // 4. 测试 500 Server Error
    let transport = MockHttpTransportFactory::server_error();
    let result = transport
        .post_json("http://test", vec![], "{}".to_string())
        .await;
    chain.add_step(
        "integration-tests::mocks",
        "HttpStatus::500_server_error",
        format!("500 error: {}", result.is_err()),
        result.is_err(),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试响应序列
#[tokio::test]
async fn test_response_sequence() {
    let mut chain = VerificationChain::new();

    // 1. 创建响应序列：成功 -> 成功 -> 失败 -> 成功
    let responses = vec![
        MockResponse::ok("first"),
        MockResponse::ok("second"),
        MockResponse::error(500, "temporary error"),
        MockResponse::ok("recovered"),
    ];

    let transport = MockHttpTransportFactory::sequence(responses);
    chain.add_step(
        "integration-tests::mocks",
        "ResponseSequence::created",
        "Response sequence created (4 responses)",
        true,
    );

    // 2. 按顺序消费响应
    let r1 = transport
        .post_json("http://test", vec![], "{}".to_string())
        .await;
    chain.add_step(
        "integration-tests::mocks",
        "ResponseSequence::first",
        format!("First response: {:?}", r1.is_ok()),
        r1.is_ok() && r1.unwrap() == "first",
    );

    let r2 = transport
        .post_json("http://test", vec![], "{}".to_string())
        .await;
    chain.add_step(
        "integration-tests::mocks",
        "ResponseSequence::second",
        format!("Second response: {:?}", r2.is_ok()),
        r2.is_ok() && r2.unwrap() == "second",
    );

    let r3 = transport
        .post_json("http://test", vec![], "{}".to_string())
        .await;
    chain.add_step(
        "integration-tests::mocks",
        "ResponseSequence::third_error",
        format!("Third response (error): {:?}", r3.is_err()),
        r3.is_err(),
    );

    let r4 = transport
        .post_json("http://test", vec![], "{}".to_string())
        .await;
    chain.add_step(
        "integration-tests::mocks",
        "ResponseSequence::fourth_recovered",
        format!("Fourth response: {:?}", r4.is_ok()),
        r4.is_ok() && r4.unwrap() == "recovered",
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试延迟响应
#[tokio::test]
async fn test_delayed_response() {
    let mut chain = VerificationChain::new();

    // 1. 创建带延迟的响应
    let transport = MockHttpTransport::new()
        .with_response(MockResponse::ok("delayed").with_delay(Duration::from_millis(100)));

    chain.add_step(
        "integration-tests::mocks",
        "DelayedResponse::created",
        "Delayed response created (100ms)",
        true,
    );

    // 2. 测量响应时间
    let start = std::time::Instant::now();
    let result = transport
        .post_json("http://test", vec![], "{}".to_string())
        .await;
    let elapsed = start.elapsed();

    chain.add_step(
        "integration-tests::mocks",
        "DelayedResponse::success",
        format!("Request succeeded: {}", result.is_ok()),
        result.is_ok(),
    );

    chain.add_step(
        "integration-tests::mocks",
        "DelayedResponse::delay_applied",
        format!("Elapsed time: {:?}", elapsed),
        elapsed >= Duration::from_millis(100),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

// ============================================================================
// 第四部分：流式响应测试
// ============================================================================

/// 测试 SSE 流响应
#[tokio::test]
async fn test_sse_stream_response() {
    let mut chain = VerificationChain::new();

    // 1. 创建流式响应
    let chunks = vec![
        "data: {\"content\": \"Hello\"}".to_string(),
        "data: {\"content\": \" World\"}".to_string(),
        "data: [DONE]".to_string(),
    ];

    let transport = MockHttpTransportFactory::stream(chunks);
    chain.add_step(
        "integration-tests::mocks",
        "SSEStream::created",
        "SSE stream transport created",
        true,
    );

    // 2. 发送流式请求
    let result = transport
        .post_stream("http://test", vec![], "{}".to_string())
        .await;
    chain.add_step(
        "integration-tests::mocks",
        "SSEStream::request_success",
        format!("Stream request succeeded: {}", result.is_ok()),
        result.is_ok(),
    );

    // 3. 消费流
    if let Ok(mut stream) = result {
        let mut received_chunks = Vec::new();
        while let Some(chunk) = stream.next().await {
            if let Ok(data) = chunk {
                received_chunks.push(data);
            }
        }

        chain.add_step(
            "integration-tests::mocks",
            "SSEStream::chunk_count",
            format!("Received {} chunks", received_chunks.len()),
            received_chunks.len() == 3,
        );
    }

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试 OpenAI 风格流
#[tokio::test]
async fn test_openai_style_stream() {
    let mut chain = VerificationChain::new();

    // 1. 创建 OpenAI 风格的流
    let transport = MockHttpTransportFactory::openai_stream("Hello World from OpenAI");
    chain.add_step(
        "integration-tests::mocks",
        "OpenAIStream::created",
        "OpenAI style stream created",
        true,
    );

    // 2. 消费流
    let result = transport
        .post_stream("http://test", vec![], "{}".to_string())
        .await;
    if let Ok(mut stream) = result {
        let mut chunks = Vec::new();
        while let Some(chunk) = stream.next().await {
            if let Ok(data) = chunk {
                chunks.push(data);
            }
        }

        chain.add_step(
            "integration-tests::mocks",
            "OpenAIStream::has_chunks",
            format!("Received {} chunks", chunks.len()),
            chunks.len() >= 4, // 3 words + [DONE]
        );

        // 验证包含 OpenAI 格式
        let has_openai_format = chunks.iter().any(|c: &Bytes| {
            let s = String::from_utf8_lossy(c);
            s.contains("chat.completion.chunk")
        });
        chain.add_step(
            "integration-tests::mocks",
            "OpenAIStream::format",
            format!("Has OpenAI format: {}", has_openai_format),
            has_openai_format,
        );
    }

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试流中注入错误
#[tokio::test]
async fn test_stream_with_error() {
    let mut chain = VerificationChain::new();

    // 1. 创建带错误的流配置
    let config = MockStreamConfig::simple(vec![
        "data: chunk1".to_string(),
        "data: chunk2".to_string(),
        "data: chunk3".to_string(),
        "data: chunk4".to_string(),
    ])
    .with_error_at(2, "Stream interrupted");

    let transport = MockHttpTransport::new().with_stream_config(config);
    chain.add_step(
        "integration-tests::mocks",
        "StreamError::created",
        "Stream with error created",
        true,
    );

    // 2. 消费流
    let result = transport
        .post_stream("http://test", vec![], "{}".to_string())
        .await;
    if let Ok(mut stream) = result {
        let mut success_count = 0;
        let mut error_found = false;

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(_) => success_count += 1,
                Err(_) => error_found = true,
            }
        }

        chain.add_step(
            "integration-tests::mocks",
            "StreamError::partial_success",
            format!("Successful chunks: {}", success_count),
            success_count >= 2, // 至少 2 个成功（错误前）
        );

        chain.add_step(
            "integration-tests::mocks",
            "StreamError::error_injected",
            format!("Error was injected: {}", error_found),
            error_found,
        );
    }

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试慢速流
#[tokio::test]
async fn test_slow_stream() {
    let mut chain = VerificationChain::new();

    // 1. 创建慢速流（每个 chunk 延迟 50ms）
    let config = MockStreamConfig::simple(vec![
        "data: slow1".to_string(),
        "data: slow2".to_string(),
        "data: slow3".to_string(),
    ])
    .with_chunk_delay(Duration::from_millis(50));

    let transport = MockHttpTransport::new().with_stream_config(config);
    chain.add_step(
        "integration-tests::mocks",
        "SlowStream::created",
        "Slow stream created (50ms per chunk)",
        true,
    );

    // 2. 测量流消费时间
    let start = std::time::Instant::now();
    let result = transport
        .post_stream("http://test", vec![], "{}".to_string())
        .await;

    if let Ok(mut stream) = result {
        while stream.next().await.is_some() {}
    }

    let elapsed = start.elapsed();

    chain.add_step(
        "integration-tests::mocks",
        "SlowStream::total_delay",
        format!("Total time: {:?}", elapsed),
        elapsed >= Duration::from_millis(150), // 3 chunks * 50ms
    );

    chain.print_report();
    assert!(chain.all_passed());
}

// ============================================================================
// 第五部分：Provider 与 MockHttpTransport 集成测试
// ============================================================================

/// 测试 Provider 使用 MockHttpTransport
#[tokio::test]
async fn test_provider_with_mock_transport() {
    let mut chain = VerificationChain::new();

    // 1. 创建 Mock Provider 和 Mock Transport
    let provider = MockProviderFactory::create_openai();
    let transport = MockHttpTransport::new();

    chain.add_step(
        "integration-tests::mocks",
        "ProviderTransportIntegration::created",
        "Provider and transport created",
        true,
    );

    // 2. 发送请求
    let request = UpstreamRequest::new("http://mock-openai", "test-key", "gpt-4o");
    let result = provider.stream_chat(&transport, request).await;

    chain.add_step(
        "llm-protocol-provider",
        "ProviderAdapter::stream_chat",
        format!("Stream created: {}", result.is_ok()),
        result.is_ok(),
    );

    // 3. 消费流
    if let Ok(mut stream) = result {
        let mut event_count = 0;
        while let Some(event) = stream.next().await {
            if event.is_ok() {
                event_count += 1;
            }
        }

        chain.add_step(
            "llm-protocol-provider",
            "ProviderTransportIntegration::events",
            format!("Events received: {}", event_count),
            event_count > 0,
        );
    }

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试多个 Transport 实例
#[tokio::test]
async fn test_multiple_transport_instances() {
    let mut chain = VerificationChain::new();

    // 1. 创建不同的 Transport 实例
    let success_transport = MockHttpTransportFactory::success();
    let error_transport = MockHttpTransportFactory::error(500, "Server error");
    let rate_limited_transport = MockHttpTransportFactory::rate_limited(60);

    chain.add_step(
        "integration-tests::mocks",
        "MultipleTransports::created",
        "Multiple transport instances created",
        true,
    );

    // 2. 测试成功 Transport
    let r1 = success_transport
        .post_json("http://test", vec![], "{}".to_string())
        .await;
    chain.add_step(
        "integration-tests::mocks",
        "MultipleTransports::success",
        format!("Success transport: {}", r1.is_ok()),
        r1.is_ok(),
    );

    // 3. 测试错误 Transport
    let r2 = error_transport
        .post_json("http://test", vec![], "{}".to_string())
        .await;
    chain.add_step(
        "integration-tests::mocks",
        "MultipleTransports::error",
        format!("Error transport: {}", r2.is_err()),
        r2.is_err(),
    );

    // 4. 测试限流 Transport
    let r3 = rate_limited_transport
        .post_json("http://test", vec![], "{}".to_string())
        .await;
    chain.add_step(
        "integration-tests::mocks",
        "MultipleTransports::rate_limited",
        format!("Rate limited transport: {}", r3.is_err()),
        r3.is_err(),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试自定义超时配置
#[tokio::test]
async fn test_custom_timeout_configuration() {
    let mut chain = VerificationChain::new();

    // 1. 创建自定义超时的 Transport
    let transport = MockHttpTransport::new()
        .with_request_timeout(Duration::from_secs(30))
        .with_stream_timeout(Duration::from_secs(120));

    chain.add_step(
        "integration-tests::mocks",
        "CustomTimeout::created",
        "Transport with custom timeouts created",
        true,
    );

    // 2. 验证超时配置
    chain.add_step(
        "integration-tests::mocks",
        "CustomTimeout::request_timeout",
        format!("Request timeout: {:?}", transport.request_timeout()),
        transport.request_timeout() == Duration::from_secs(30),
    );

    chain.add_step(
        "integration-tests::mocks",
        "CustomTimeout::stream_timeout",
        format!("Stream timeout: {:?}", transport.stream_timeout()),
        transport.stream_timeout() == Duration::from_secs(120),
    );

    chain.print_report();
    assert!(chain.all_passed());
}

/// 测试请求头传递
#[tokio::test]
async fn test_header_passing() {
    let mut chain = VerificationChain::new();

    // 1. 创建 Transport
    let transport = MockHttpTransport::new();
    transport.add_response(MockResponse::ok("ok"));

    // 2. 发送带自定义头的请求
    let headers = vec![
        ("Authorization".to_string(), "Bearer test-token".to_string()),
        ("X-Request-ID".to_string(), "req-123".to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
    ];

    let _ = transport
        .post_json("http://test", headers.clone(), "{}".to_string())
        .await;

    chain.add_step(
        "integration-tests::mocks",
        "HeaderPassing::request_sent",
        "Request with custom headers sent",
        true,
    );

    // 3. 验证请求头被记录
    let last_request = transport.last_request().unwrap();
    chain.add_step(
        "integration-tests::mocks",
        "HeaderPassing::header_count",
        format!("Headers recorded: {}", last_request.headers.len()),
        last_request.headers.len() == 3,
    );

    let has_auth = last_request
        .headers
        .iter()
        .any(|(k, v)| k == "Authorization" && v == "Bearer test-token");
    chain.add_step(
        "integration-tests::mocks",
        "HeaderPassing::auth_header",
        format!("Has correct auth header: {}", has_auth),
        has_auth,
    );

    chain.print_report();
    assert!(chain.all_passed());
}

// ============================================================================
// 第六部分：并发测试
// ============================================================================

/// 测试并发请求处理
#[tokio::test]
async fn test_concurrent_requests_with_mock_transport() {
    use tokio::task::JoinSet;

    let mut chain = VerificationChain::new();
    let concurrent_requests = 20;

    // 1. 创建共享的 Transport
    let transport = std::sync::Arc::new(MockHttpTransport::new());
    for _ in 0..concurrent_requests {
        transport.add_response(MockResponse::ok(r#"{"status": "ok"}"#));
    }

    chain.add_step(
        "integration-tests::mocks",
        "ConcurrentMockTransport::setup",
        format!(
            "Transport setup for {} concurrent requests",
            concurrent_requests
        ),
        true,
    );

    // 2. 并发发送请求
    let mut tasks = JoinSet::new();
    let success_count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    for _ in 0..concurrent_requests {
        let t = transport.clone();
        let counter = success_count.clone();

        tasks.spawn(async move {
            let result = t.post_json("http://test", vec![], "{}".to_string()).await;
            if result.is_ok() {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        });
    }

    // 3. 等待完成
    while tasks.join_next().await.is_some() {}

    chain.add_step(
        "integration-tests::mocks",
        "ConcurrentMockTransport::all_succeeded",
        format!(
            "All requests succeeded: {}",
            success_count.load(std::sync::atomic::Ordering::Relaxed)
        ),
        success_count.load(std::sync::atomic::Ordering::Relaxed) == concurrent_requests as u64,
    );

    chain.add_step(
        "integration-tests::mocks",
        "ConcurrentMockTransport::history_count",
        format!("History recorded: {}", transport.request_count()),
        transport.request_count() == concurrent_requests,
    );

    chain.print_report();
    assert!(chain.all_passed());
}
