use std::sync::LazyLock;

use client_api::api::auth::RefreshTokenRequest;
use client_api::error::{ClientError, Result};
use client_api::{ApiClient, AuthApi, ClientConfig};

use crate::stores::auth_store::AuthStore;

/// 全局单例 API 客户端
/// ApiClient 内部持有 Arc，Clone 只是增加引用计数，开销极低
static CLIENT: LazyLock<ApiClient> = LazyLock::new(|| {
    let base_url = option_env!("API_BASE_URL").unwrap_or("").to_string();
    let config = ClientConfig::new(base_url);
    ApiClient::new(config).expect("Failed to create API client")
});

/// 获取全局 API 客户端实例（廉价克隆，仅增加 Arc 引用计数）
pub fn get_client() -> ApiClient {
    CLIENT.clone()
}

/// 归一化配置的 API 基址到根路径（去掉 /auth、/api/v1、/v1 等后缀）
fn normalize_api_root(configured: &str) -> String {
    let mut root = configured.trim_end_matches('/');
    loop {
        // 按最长后缀优先裁剪；用 strip_suffix 而非 trim_end_matches，
        // 避免重复匹配同一后缀时误吞 `api/v1/v1` 这类前缀
        let stripped = root
            .strip_suffix("/api/v1")
            .or_else(|| root.strip_suffix("/auth"))
            .or_else(|| root.strip_suffix("/v1"));
        match stripped {
            Some(next) => root = next.trim_end_matches('/'),
            None => return root.to_string(),
        }
    }
}

/// 获取对外展示用的 API 根路径（不含 /v1 后缀）
///
/// - 如果配置了绝对 `API_BASE_URL`，优先使用配置值并归一化到根路径
/// - 如果当前是同域反代部署（`API_BASE_URL=""`），在浏览器中读取当前站点 origin
///
/// Anthropic SDK 会在 base_url 后自行追加 `/v1/messages`，快速示例需用根路径；
/// OpenAI 兼容端点需要 `/v1` 后缀（见 `public_openai_api_base_url`）。
pub fn public_api_root_url() -> String {
    let client = get_client();
    let configured = client.config().base_url.trim_end_matches('/');

    if configured.is_empty() {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(origin) =
                web_sys::window().and_then(|window| window.location().origin().ok())
            {
                return origin.trim_end_matches('/').to_string();
            }
        }

        return "http://localhost:8080".to_string();
    }

    normalize_api_root(configured)
}

/// 为根路径追加 OpenAI 兼容的 `/v1` 后缀：幂等地处理尾斜杠与已含 `/v1` 的情况。
fn append_v1(root: &str) -> String {
    let root = root.trim_end_matches('/');
    if root.ends_with("/v1") {
        root.to_string()
    } else {
        format!("{root}/v1")
    }
}

/// 获取对外展示用的 OpenAI 兼容 API 基址（以 `/v1` 结尾）
///
/// - 如果配置了绝对 `API_BASE_URL`，优先使用配置值并归一化到 `/v1`
/// - 如果当前是同域反代部署（`API_BASE_URL=""`），在浏览器中读取当前站点 origin
pub fn public_openai_api_base_url() -> String {
    append_v1(&public_api_root_url())
}

/// Token 自动刷新封装器
///
/// 在 service 层调用任意异步 API 时，若返回 `ClientError::Unauthorized`，
/// 则尝试用当前 token 刷新获取新 token，刷新成功后重试原请求。
/// 如果刷新失败，则强制登出。
///
/// # 示例
/// ```rust
/// let result = with_auto_refresh(auth_store, |token| async move {
///     some_service::fetch(&token).await
/// }).await;
/// ```
pub async fn with_auto_refresh<F, Fut, T>(mut auth_store: AuthStore, f: F) -> Result<T>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    // 优先从全局 API 客户端获取 token（登录时会设置到这里）
    let token = get_client()
        .get_token()
        .or_else(|| auth_store.token())
        .unwrap_or_default();

    match f(token.clone()).await {
        Err(ClientError::Unauthorized(_)) => {
            // Token 过期，尝试刷新
            match try_refresh_token(&token).await {
                Ok(new_token) => {
                    // 刷新成功，更新 token 并重试原请求
                    get_client().set_token(new_token.clone());
                    auth_store.login(new_token.clone());
                    f(new_token).await
                }
                Err(_) => {
                    // 刷新失败，强制登出
                    auth_store.logout();
                    get_client().clear_token();
                    Err(ClientError::Unauthorized(
                        "登录已过期，请重新登录".to_string(),
                    ))
                }
            }
        }
        other => other,
    }
}

/// 尝试刷新 Token
async fn try_refresh_token(token: &str) -> Result<String> {
    let client = get_client();
    let req = RefreshTokenRequest::new(token);
    let resp = AuthApi::new(&client).refresh_token(&req).await?;
    Ok(resp.access_token)
}

/// 将 ClientError 转为用户友好的中文提示文本
///
/// 在 UI 层展示错误时调用，避免直接折射原始英文错误字符串给用户。
#[allow(dead_code)]
pub fn localize_error(err: &client_api::error::ClientError) -> String {
    use client_api::error::ClientError;
    match err {
        ClientError::Unauthorized(_) => "登录已过期，请重新登录".to_string(),
        ClientError::Forbidden(_) => "权限不足，无法执行此操作".to_string(),
        ClientError::NotFound(_) => "资源不存在或已被删除".to_string(),
        ClientError::RateLimited(_) => "请求过于频繁，请稍候再试".to_string(),
        ClientError::Verification(_) => "验证码校验失败，请检查后重试".to_string(),
        ClientError::Network(_) => "网络连接失败，请检查网络设置".to_string(),
        ClientError::ServerError(_) => "服务器内部错误，请稍候重试".to_string(),
        ClientError::ServiceUnavailable(_) => "服务暂时不可用，请稍候再试".to_string(),
        ClientError::Serialization(_) | ClientError::InvalidResponse(_) => {
            "数据解析失败，请刷新页面".to_string()
        }
        ClientError::Config(msg) => format!("配置错误：{}", msg),
        ClientError::Http(msg) => {
            // 尝试提取状态码后的消息部分
            if msg.contains("400") {
                "请求参数错误，请检查输入".to_string()
            } else if msg.contains("409") {
                "数据冲突，该资源可能已存在".to_string()
            } else {
                "请求失败，请稍候重试".to_string()
            }
        }
        ClientError::Other(msg) => msg.clone(),
    }
}

/// 优先使用后端返回的业务消息；如消息过于底层，再回退到本地友好文案。
pub fn user_error_message(err: &client_api::error::ClientError) -> String {
    let message = err.message();
    if message.trim().is_empty() {
        return localize_error(err);
    }

    match err {
        ClientError::Network(_)
        | ClientError::Serialization(_)
        | ClientError::InvalidResponse(_) => localize_error(err),
        _ => message,
    }
}

#[cfg(test)]
mod tests {
    use super::{append_v1, normalize_api_root};

    #[test]
    fn normalize_api_root_strips_known_suffixes() {
        assert_eq!(
            normalize_api_root("http://gw.example.com/v1"),
            "http://gw.example.com"
        );
        assert_eq!(
            normalize_api_root("http://gw.example.com/api/v1"),
            "http://gw.example.com"
        );
        assert_eq!(
            normalize_api_root("http://gw.example.com/auth"),
            "http://gw.example.com"
        );
        assert_eq!(
            normalize_api_root("http://gw.example.com/"),
            "http://gw.example.com"
        );
        assert_eq!(
            normalize_api_root("http://gw.example.com"),
            "http://gw.example.com"
        );
        assert_eq!(
            normalize_api_root("http://localhost:8080/v1"),
            "http://localhost:8080"
        );
        // 畸形双后缀：不得吞掉 `/api` 前缀
        assert_eq!(
            normalize_api_root("http://gw.example.com/v1/v1"),
            "http://gw.example.com"
        );
        assert_eq!(
            normalize_api_root("http://gw.example.com/api/v1/v1"),
            "http://gw.example.com"
        );
    }

    /// 空串输入保持空串返回（调用方保证不会传入空串，此处锁定防御性行为）
    #[test]
    fn normalize_api_root_handles_empty_input() {
        assert_eq!(normalize_api_root(""), "");
    }

    #[test]
    fn append_v1_is_idempotent_and_handles_trailing_slash() {
        assert_eq!(
            append_v1("http://gw.example.com"),
            "http://gw.example.com/v1"
        );
        assert_eq!(
            append_v1("http://gw.example.com/"),
            "http://gw.example.com/v1"
        );
        assert_eq!(
            append_v1("http://gw.example.com/v1"),
            "http://gw.example.com/v1"
        );
        assert_eq!(
            append_v1("http://localhost:8080"),
            "http://localhost:8080/v1"
        );
    }

    /// normalize 到根路径后追加 /v1，等价于旧版 public_openai_api_base_url 的行为
    #[test]
    fn normalized_root_plus_v1_matches_previous_openai_base() {
        assert_eq!(
            append_v1(&normalize_api_root("http://gw.example.com/v1")),
            "http://gw.example.com/v1"
        );
        assert_eq!(
            append_v1(&normalize_api_root("http://gw.example.com/api/v1")),
            "http://gw.example.com/v1"
        );
        assert_eq!(
            append_v1(&normalize_api_root("http://gw.example.com")),
            "http://gw.example.com/v1"
        );
        assert_eq!(
            append_v1(&normalize_api_root("http://gw.example.com/auth")),
            "http://gw.example.com/v1"
        );
        assert_eq!(
            append_v1(&normalize_api_root("http://localhost:8080/v1")),
            "http://localhost:8080/v1"
        );
    }
}
