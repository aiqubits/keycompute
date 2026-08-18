//! API Key 快速使用示例文本生成。
//!
//! 与后端双协议网关对应：OpenAI 兼容（/v1/chat/completions）与
//! Anthropic Messages（/v1/messages）各有一套 env / python / node / curl 示例。
//! 示例文本生成与视图解耦，便于单元测试，防止占位符与参数失配导致
//! 复制出去的示例不可用。

use client_api::api::openai::ModelInfo;

/// Anthropic 示例的默认模型：模型列表中无 Claude 模型时的回退值。
/// 与后端 Anthropic 协议测试与文档中广泛使用的命名保持一致。
/// `pub`：供视图层把模型名代入翻译文案（见 list.rs 的 example_note_anthropic）。
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-3-5-sonnet-20241022";

/// 一套四种示例文本（env / python / node / curl）
pub struct ApiExamples {
    pub env: String,
    pub python: String,
    pub node: String,
    pub curl: String,
}

impl ApiExamples {
    /// 按 tab 名取对应示例；未知 tab 回退到 env
    pub fn for_tab(&self, tab: &str) -> &str {
        match tab {
            "python" => &self.python,
            "node" => &self.node,
            "curl" => &self.curl,
            _ => &self.env,
        }
    }
}

/// OpenAI 兼容协议示例（调用 /v1/chat/completions）
pub fn openai_examples(
    api_url: &str,
    api_key: &str,
    model: &str,
    env_comment: &str,
) -> ApiExamples {
    ApiExamples {
        env: format!(
            r#"# {}
API_URL="{}"
API_KEY="{}"
API_MODEL="{}""#,
            env_comment, api_url, api_key, model
        ),
        python: format!(
            r#"from openai import OpenAI

client = OpenAI(
    base_url="{}",
    api_key="{}",
)

response = client.chat.completions.create(
    model="{}",
    messages=[{{"role": "user", "content": "Hello"}}],
)

print(response.choices[0].message.content)"#,
            api_url, api_key, model
        ),
        node: format!(
            r#"import OpenAI from "openai";

const client = new OpenAI({{
  baseURL: "{}",
  apiKey: "{}",
}});

const response = await client.chat.completions.create({{
  model: "{}",
  messages: [{{ role: "user", content: "Hello" }}],
}});

console.log(response.choices[0].message.content);"#,
            api_url, api_key, model
        ),
        curl: format!(
            r#"curl "{}/chat/completions" \
    -H "Authorization: Bearer {}" \
    -H "Content-Type: application/json" \
  -d '{{
    "model": "{}",
    "messages": [
      {{"role": "user", "content": "Hello"}}
    ]
  }}'"#,
            api_url, api_key, model
        ),
    }
}

/// Anthropic Messages 协议示例（调用 /v1/messages）。
///
/// `api_root` 必须是不含 `/v1` 的根路径：官方 Anthropic SDK 会在 base_url
/// 后自行追加 `/v1/messages`，传以 `/v1` 结尾的地址会拼出 `/v1/v1/messages`。
pub fn anthropic_examples(
    api_root: &str,
    api_key: &str,
    model: &str,
    env_comment: &str,
) -> ApiExamples {
    // 防御尾斜杠：调用方可能传入 "http://gw.example.com/"，
    // 与 api_client 的 normalize 惯例保持一致，避免拼出 //v1/messages。
    let root = api_root.trim_end_matches('/');
    ApiExamples {
        env: format!(
            r#"# {}
ANTHROPIC_BASE_URL="{}"
ANTHROPIC_API_KEY="{}""#,
            env_comment, root, api_key
        ),
        python: format!(
            r#"from anthropic import Anthropic

client = Anthropic(
    base_url="{}",
    api_key="{}",
)

message = client.messages.create(
    model="{}",
    max_tokens=1024,
    messages=[{{"role": "user", "content": "Hello"}}],
)

print(message.content[0].text)"#,
            root, api_key, model
        ),
        node: format!(
            r#"import Anthropic from "@anthropic-ai/sdk";

const client = new Anthropic({{
  baseURL: "{}",
  apiKey: "{}",
}});

const message = await client.messages.create({{
  model: "{}",
  max_tokens: 1024,
  messages: [{{ role: "user", content: "Hello" }}],
}});

console.log(message.content[0].text);"#,
            root, api_key, model
        ),
        curl: format!(
            r#"curl "{}/v1/messages" \
    -H "x-api-key: {}" \
    -H "anthropic-version: 2023-06-01" \
    -H "Content-Type: application/json" \
  -d '{{
    "model": "{}",
    "max_tokens": 1024,
    "messages": [
      {{"role": "user", "content": "Hello"}}
    ]
  }}'"#,
            root, api_key, model
        ),
    }
}

/// 从模型列表中选取展示用的默认模型（列表为空时回退 deepseek-chat）
pub fn pick_sample_model(models: &[ModelInfo]) -> String {
    models
        .first()
        .map(|model| model.id.clone())
        .unwrap_or_else(|| "deepseek-chat".to_string())
}

/// 从模型列表中选取 Anthropic 示例模型：优先第一个 Claude 模型
/// （大小写不敏感），列表中没有 Claude 模型时回退默认值。
pub fn pick_claude_model(models: &[ModelInfo]) -> String {
    models
        .iter()
        .map(|model| model.id.clone())
        .find(|id| id.to_lowercase().starts_with("claude"))
        .unwrap_or_else(|| DEFAULT_CLAUDE_MODEL.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 断言一段示例文本不含未替换的 `{}` 占位符（format 参数与占位符失配的哨兵）
    fn assert_no_leftover_placeholder(text: &str) {
        assert!(!text.contains("{}"), "示例文本残留未替换占位符: {text}");
    }

    fn model(id: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            object: "model".to_string(),
            created: 0,
            owned_by: "test".to_string(),
        }
    }

    #[test]
    fn openai_examples_are_fully_formatted() {
        let e = openai_examples(
            "http://gw.example.com/v1",
            "sk-test",
            "deepseek-chat",
            "# env",
        );
        for text in [&e.env, &e.python, &e.node, &e.curl] {
            assert_no_leftover_placeholder(text);
        }
        assert!(e.python.contains("from openai import OpenAI"));
        assert!(
            e.curl
                .contains("\"http://gw.example.com/v1/chat/completions\"")
        );
        assert!(e.curl.contains("-H \"Authorization: Bearer sk-test\""));
    }

    #[test]
    fn anthropic_examples_are_fully_formatted() {
        let e = anthropic_examples(
            "http://gw.example.com",
            "sk-test",
            "claude-3-5-sonnet-20241022",
            "# env",
        );
        for text in [&e.env, &e.python, &e.node, &e.curl] {
            assert_no_leftover_placeholder(text);
        }
        assert!(e.python.contains("from anthropic import Anthropic"));
        assert!(e.python.contains("max_tokens=1024"));
    }

    /// Anthropic SDK 会在 base_url 后自行追加 /v1/messages，示例必须使用
    /// 不含 /v1 的根路径，否则会拼出 /v1/v1/messages
    #[test]
    fn anthropic_sdk_examples_use_root_url_without_v1() {
        let e = anthropic_examples(
            "http://gw.example.com",
            "sk-test",
            "claude-3-5-sonnet",
            "# env",
        );
        assert!(e.python.contains("base_url=\"http://gw.example.com\""));
        assert!(e.node.contains("baseURL: \"http://gw.example.com\""));
        assert!(
            e.env
                .contains("ANTHROPIC_BASE_URL=\"http://gw.example.com\"")
        );
        assert!(!e.python.contains("v1/v1"));
        assert!(!e.node.contains("v1/v1"));
    }

    /// curl 直接指向网关挂载点 {root}/v1/messages
    #[test]
    fn anthropic_curl_targets_messages_endpoint() {
        let e = anthropic_examples(
            "http://gw.example.com",
            "sk-test",
            "claude-3-5-sonnet",
            "# env",
        );
        assert!(e.curl.contains("\"http://gw.example.com/v1/messages\""));
        assert!(e.curl.contains("-H \"x-api-key: sk-test\""));
        assert!(e.curl.contains("-H \"anthropic-version: 2023-06-01\""));
    }

    /// 尾斜杠防御：api_root 带尾斜杠时不得拼出 //v1/messages
    #[test]
    fn anthropic_examples_tolerate_trailing_slash_on_root() {
        let e = anthropic_examples(
            "http://gw.example.com/",
            "sk-test",
            "claude-3-5-sonnet",
            "# env",
        );
        assert!(e.curl.contains("\"http://gw.example.com/v1/messages\""));
        assert!(!e.curl.contains("//v1"));
        assert!(e.python.contains("base_url=\"http://gw.example.com\""));
    }

    #[test]
    fn for_tab_maps_all_tabs_and_falls_back_to_env() {
        let e = anthropic_examples("http://gw.example.com", "sk-test", "claude", "# env");
        assert_eq!(e.for_tab("python"), &e.python);
        assert_eq!(e.for_tab("node"), &e.node);
        assert_eq!(e.for_tab("curl"), &e.curl);
        assert_eq!(e.for_tab("env"), &e.env);
        assert_eq!(e.for_tab("unknown"), &e.env);
    }

    #[test]
    fn pick_claude_model_prefers_first_claude_case_insensitive() {
        let models = [
            model("deepseek-chat"),
            model("Claude-3-5-Sonnet"),
            model("claude-3-opus"),
        ];
        assert_eq!(pick_claude_model(&models), "Claude-3-5-Sonnet");
    }

    #[test]
    fn pick_claude_model_falls_back_when_no_claude_listed() {
        assert_eq!(
            pick_claude_model(&[model("deepseek-chat"), model("gpt-4o")]),
            "claude-3-5-sonnet-20241022"
        );
        assert_eq!(pick_claude_model(&[]), "claude-3-5-sonnet-20241022");
    }

    #[test]
    fn pick_sample_model_returns_first_or_fallback() {
        assert_eq!(
            pick_sample_model(&[model("gpt-4o"), model("gpt-4o-mini")]),
            "gpt-4o"
        );
        assert_eq!(pick_sample_model(&[]), "deepseek-chat");
    }
}
