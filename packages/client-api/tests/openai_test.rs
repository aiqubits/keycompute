//! OpenAI 兼容 API 模块集成测试

use client_api::api::openai::{ChatCompletionRequest, Message, OpenAiApi};
use client_api::client::OpenAiClient;
use client_api::config::ClientConfig;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 创建 OpenAI 测试客户端
async fn create_openai_test_client() -> (OpenAiClient, MockServer) {
    let mock_server = MockServer::start().await;
    // 绕过系统代理，确保请求直连本地 Mock 服务器
    let config = ClientConfig::new(mock_server.uri()).with_no_proxy(true);
    let client = OpenAiClient::new(config).expect("Failed to create OpenAI client");
    (client, mock_server)
}

#[tokio::test]
async fn test_chat_completions_success() {
    let (client, mock_server) = create_openai_test_client().await;
    let openai_api = OpenAiApi::new(&client);

    // 注意：ChatCompletionRequest 使用 skip_serializing_if = "Option::is_none"
    // 所以当字段为 None 时，不会被序列化到 JSON 中
    let expected_body = serde_json::json!({
        "model": "gpt-4",
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": "Hello!"}
        ]
    });

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_json(&expected_body))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Hello! How can I help you today?"
                    },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 9,
                "completion_tokens": 12,
                "total_tokens": 21
            }
        })))
        .mount(&mock_server)
        .await;

    let messages = vec![
        Message::system("You are a helpful assistant."),
        Message::user("Hello!"),
    ];
    let req = ChatCompletionRequest::new("gpt-4", messages);
    let result = openai_api.chat_completions(&req, "sk-test-api-key").await;

    assert!(result.is_ok());
    let resp = result.unwrap();
    assert_eq!(resp.id, "chatcmpl-123");
    assert_eq!(resp.model, "gpt-4");
    assert_eq!(resp.choices.len(), 1);
    assert_eq!(resp.choices[0].message.role, "assistant");
}

#[tokio::test]
async fn test_chat_completions_with_options() {
    let (client, mock_server) = create_openai_test_client().await;
    let openai_api = OpenAiApi::new(&client);

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-456",
            "object": "chat.completion",
            "created": 1677652289,
            "model": "gpt-3.5-turbo",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "This is a creative response."
                    },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 15,
                "total_tokens": 25
            }
        })))
        .mount(&mock_server)
        .await;

    let messages = vec![Message::user("Be creative!")];
    let req = ChatCompletionRequest::new("gpt-3.5-turbo", messages)
        .with_temperature(0.9)
        .with_max_tokens(100);
    let result = openai_api.chat_completions(&req, "sk-test-api-key").await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_list_models_success() {
    let (client, mock_server) = create_openai_test_client().await;
    let openai_api = OpenAiApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "object": "list",
            "data": [
                {
                    "id": "gpt-4",
                    "object": "model",
                    "created": 1687882411,
                    "owned_by": "openai"
                },
                {
                    "id": "gpt-3.5-turbo",
                    "object": "model",
                    "created": 1677610602,
                    "owned_by": "openai"
                },
                {
                    "id": "text-embedding-ada-002",
                    "object": "model",
                    "created": 1671217299,
                    "owned_by": "openai"
                }
            ]
        })))
        .mount(&mock_server)
        .await;

    let result = openai_api.list_models("sk-test-api-key").await;

    assert!(result.is_ok());
    let models = result.unwrap();
    assert_eq!(models.data.len(), 3);
    assert_eq!(models.data[0].id, "gpt-4");
    assert_eq!(models.data[1].id, "gpt-3.5-turbo");
}

#[tokio::test]
async fn test_list_models_by_protocol_success() {
    let (client, mock_server) = create_openai_test_client().await;
    let openai_api = OpenAiApi::new(&client);

    // 网关扩展：按入口协议过滤模型（与入口协议隔离路由保持一致）
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(query_param("protocol", "anthropic"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "object": "list",
            "data": [
                {
                    "id": "claude-3-5-sonnet-20241022",
                    "object": "model",
                    "created": 1729680000,
                    "owned_by": "anthropic"
                }
            ]
        })))
        .mount(&mock_server)
        .await;

    let result = openai_api
        .list_models_for_protocol("sk-test-api-key", Some("anthropic"))
        .await;

    assert!(result.is_ok());
    let models = result.unwrap();
    assert_eq!(models.data.len(), 1);
    assert_eq!(models.data[0].id, "claude-3-5-sonnet-20241022");
}

#[tokio::test]
async fn test_retrieve_model_success() {
    let (client, mock_server) = create_openai_test_client().await;
    let openai_api = OpenAiApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/v1/models/gpt-4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "gpt-4",
            "object": "model",
            "created": 1687882411,
            "owned_by": "openai"
        })))
        .mount(&mock_server)
        .await;

    let result = openai_api.retrieve_model("gpt-4", "sk-test-api-key").await;

    assert!(result.is_ok());
    let model = result.unwrap();
    assert_eq!(model.id, "gpt-4");
    assert_eq!(model.owned_by, "openai");
}

#[tokio::test]
async fn test_retrieve_model_not_found() {
    let (client, mock_server) = create_openai_test_client().await;
    let openai_api = OpenAiApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/v1/models/nonexistent-model"))
        .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
            "error": {
                "message": "The model 'nonexistent-model' does not exist",
                "type": "invalid_request_error",
                "param": null,
                "code": "model_not_found"
            }
        })))
        .mount(&mock_server)
        .await;

    let result = openai_api
        .retrieve_model("nonexistent-model", "sk-test-api-key")
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_invalid_api_key() {
    let (client, mock_server) = create_openai_test_client().await;
    let openai_api = OpenAiApi::new(&client);

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": {
                "message": "Incorrect API key provided",
                "type": "invalid_request_error",
                "param": null,
                "code": "invalid_api_key"
            }
        })))
        .mount(&mock_server)
        .await;

    let result = openai_api.list_models("invalid-key").await;

    assert!(result.is_err());
}
