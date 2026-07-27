use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use rand::{Rng, distributions::Alphanumeric};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    NotifyHeaders, VerifiedNotify, WechatPayConfig, WechatPayConfigError, WechatPayNotify,
    crypto::WechatCrypto,
};

const API_BASE: &str = "https://api.mch.weixin.qq.com";

pub struct WechatPayClient {
    config: WechatPayConfig,
    crypto: WechatCrypto,
    http: reqwest::Client,
}

impl WechatPayClient {
    pub fn new(config: WechatPayConfig) -> Result<Self, WechatPayError> {
        config.validate()?;
        let crypto = WechatCrypto::new(&config)?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            config,
            crypto,
            http,
        })
    }

    pub fn config(&self) -> &WechatPayConfig {
        &self.config
    }

    pub async fn create_native_order(
        &self,
        mut request: NativeOrderRequest,
    ) -> Result<NativeOrderResponse, WechatPayError> {
        request.appid = self.config.appid.clone();
        request.mchid = self.config.mchid.clone();
        request.notify_url = self.config.notify_url.clone();
        self.request(Method::POST, "/v3/pay/transactions/native", Some(&request))
            .await
    }

    pub async fn query_order(&self, out_trade_no: &str) -> Result<WechatTrade, WechatPayError> {
        let path = format!(
            "/v3/pay/transactions/out-trade-no/{out_trade_no}?mchid={}",
            self.config.mchid
        );
        self.request::<(), _>(Method::GET, &path, None).await
    }

    pub async fn close_order(&self, out_trade_no: &str) -> Result<(), WechatPayError> {
        #[derive(Serialize)]
        struct CloseRequest<'a> {
            mchid: &'a str,
        }
        let path = format!("/v3/pay/transactions/out-trade-no/{out_trade_no}/close");
        self.request_empty(
            Method::POST,
            &path,
            Some(&CloseRequest {
                mchid: &self.config.mchid,
            }),
        )
        .await
    }

    pub fn verify_and_decode_notify(
        &self,
        headers: &NotifyHeaders,
        raw_body: &[u8],
    ) -> Result<(WechatPayNotify, VerifiedNotify), WechatPayError> {
        let envelope = self.crypto.verify_notify(headers, raw_body)?;
        let resource = self.crypto.decrypt_notify(&envelope.resource)?;
        Ok((envelope, resource))
    }

    async fn request<B: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<R, WechatPayError> {
        let (status, raw) = self.send(method, path, body).await?;
        if !status.is_success() {
            return Err(api_error(status, &raw));
        }
        serde_json::from_slice(&raw).map_err(|e| WechatPayError::Decode(e.to_string()))
    }

    async fn request_empty<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<(), WechatPayError> {
        let (status, raw) = self.send(method, path, body).await?;
        if status.is_success() {
            Ok(())
        } else {
            Err(api_error(status, &raw))
        }
    }

    async fn send<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<(StatusCode, Vec<u8>), WechatPayError> {
        let body_text = match body {
            Some(value) => {
                serde_json::to_string(value).map_err(|e| WechatPayError::Decode(e.to_string()))?
            }
            None => String::new(),
        };
        let timestamp = Utc::now().timestamp().to_string();
        let nonce: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(24)
            .map(char::from)
            .collect();
        let message = format!(
            "{}\n{}\n{}\n{}\n{}\n",
            method.as_str(),
            path,
            timestamp,
            nonce,
            body_text
        );
        let signature = self.crypto.sign(&message);
        let authorization = format!(
            "WECHATPAY2-SHA256-RSA2048 mchid=\"{}\",nonce_str=\"{}\",timestamp=\"{}\",serial_no=\"{}\",signature=\"{}\"",
            self.config.mchid, nonce, timestamp, self.config.merchant_serial_no, signature
        );
        let mut builder = self
            .http
            .request(method, format!("{API_BASE}{path}"))
            .header("Authorization", authorization)
            .header("Accept", "application/json");
        if !body_text.is_empty() {
            builder = builder
                .header("Content-Type", "application/json")
                .body(body_text);
        }
        let response = builder.send().await?;
        let status = response.status();
        let headers = response.headers().clone();
        let raw = response.bytes().await?.to_vec();
        // API v3 的非空响应（包括错误响应）都必须验证微信支付签名。
        // 例外：完全缺失签名头的 5xx 应答通常来自网关/代理故障而非微信
        // 业务后端，若按验签失败处理会被误判为终局凭据错误并打开熔断；
        // 此时跳过验签，让调用方归类为可重试的 Api 错误。部分缺头或
        // 非 5xx 的无签名应答仍严格拒绝。
        if !raw.is_empty() && should_verify_response(status, &headers) {
            verify_response(&self.crypto, &headers, &raw)?;
        }
        Ok((status, raw))
    }
}

const RESPONSE_SIGNATURE_HEADERS: [&str; 4] = [
    "Wechatpay-Timestamp",
    "Wechatpay-Nonce",
    "Wechatpay-Signature",
    "Wechatpay-Serial",
];

fn should_verify_response(status: StatusCode, headers: &reqwest::header::HeaderMap) -> bool {
    let lacks_all_signature_headers = RESPONSE_SIGNATURE_HEADERS
        .iter()
        .all(|name| !headers.contains_key(*name));
    !(status.is_server_error() && lacks_all_signature_headers)
}

fn verify_response(
    crypto: &WechatCrypto,
    headers: &reqwest::header::HeaderMap,
    body: &[u8],
) -> Result<(), WechatPayError> {
    let get = |name: &'static str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .ok_or_else(|| WechatPayError::InvalidSignature(format!("missing {name}")))
    };
    let timestamp = get("Wechatpay-Timestamp")?;
    let nonce = get("Wechatpay-Nonce")?;
    let signature = get("Wechatpay-Signature")?;
    let serial = get("Wechatpay-Serial")?;
    let timestamp_value: i64 = timestamp
        .parse()
        .map_err(|_| WechatPayError::InvalidSignature("invalid response timestamp".to_string()))?;
    if Utc::now().timestamp().abs_diff(timestamp_value) > 300 {
        return Err(WechatPayError::InvalidSignature(
            "response timestamp outside five minute window".to_string(),
        ));
    }
    let body = std::str::from_utf8(body).map_err(|e| WechatPayError::Decode(e.to_string()))?;
    crypto.verify(
        &serial,
        &format!("{timestamp}\n{nonce}\n{body}\n"),
        &signature,
    )
}

fn api_error(status: StatusCode, body: &[u8]) -> WechatPayError {
    #[derive(Deserialize)]
    struct ErrorBody {
        code: Option<String>,
        message: Option<String>,
    }
    let parsed: Option<ErrorBody> = serde_json::from_slice(body).ok();
    WechatPayError::Api {
        status: status.as_u16(),
        code: parsed.as_ref().and_then(|e| e.code.clone()),
        message: parsed
            .and_then(|e| e.message)
            .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned()),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeOrderRequest {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub appid: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mchid: String,
    pub description: String,
    pub out_trade_no: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notify_url: String,
    pub time_expire: String,
    pub amount: TradeAmount,
}

impl NativeOrderRequest {
    pub fn new(
        description: String,
        out_trade_no: String,
        expires_at: DateTime<Utc>,
        total_fen: i64,
    ) -> Self {
        Self {
            appid: String::new(),
            mchid: String::new(),
            description,
            out_trade_no,
            notify_url: String::new(),
            time_expire: expires_at.to_rfc3339_opts(SecondsFormat::Secs, true),
            amount: TradeAmount {
                total: total_fen,
                currency: "CNY".to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeAmount {
    pub total: i64,
    pub currency: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NativeOrderResponse {
    pub code_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WechatTrade {
    pub appid: String,
    pub mchid: String,
    pub out_trade_no: String,
    pub transaction_id: Option<String>,
    pub trade_type: Option<String>,
    pub trade_state: TradeState,
    pub trade_state_desc: Option<String>,
    pub amount: TradeAmount,
    pub success_time: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum TradeState {
    Success,
    Refund,
    Notpay,
    Closed,
    Revoked,
    Userpaying,
    Payerror,
}

#[derive(Debug, thiserror::Error)]
pub enum WechatPayError {
    #[error(transparent)]
    Config(#[from] WechatPayConfigError),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("WeChat Pay API error ({status}, {code:?}): {message}")]
    Api {
        status: u16,
        code: Option<String>,
        message: String,
    },
    #[error("cryptography error: {0}")]
    Crypto(String),
    #[error("invalid WeChat Pay signature: {0}")]
    InvalidSignature(String),
    #[error("decode error: {0}")]
    Decode(String),
}

impl From<reqwest::Error> for WechatPayError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use rand::rngs::OsRng;
    use rsa::{
        RsaPrivateKey,
        pkcs1v15::SigningKey,
        pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding},
        signature::{SignatureEncoding, Signer},
    };
    use sha2::Sha256;

    fn response_crypto() -> (WechatCrypto, SigningKey<Sha256>, String) {
        let merchant = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let platform = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let key_id = "PUB_KEY_ID_TEST".to_string();
        let config = WechatPayConfig {
            appid: "wx-test".to_string(),
            mchid: "1900000109".to_string(),
            merchant_serial_no: "merchant-serial".to_string(),
            merchant_private_key: merchant.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
            api_v3_key: "0123456789abcdef0123456789abcdef".to_string(),
            wechatpay_public_key_id: key_id.clone(),
            wechatpay_public_key: platform
                .to_public_key()
                .to_public_key_pem(LineEnding::LF)
                .unwrap(),
            previous_callback_keys: Vec::new(),
            notify_url: "https://example.com/notify".to_string(),
            timeout_minutes: 15,
        };
        (
            WechatCrypto::new(&config).unwrap(),
            SigningKey::new(platform),
            key_id,
        )
    }

    fn signed_response_headers(
        signer: &SigningKey<Sha256>,
        key_id: &str,
        timestamp: i64,
        body: &[u8],
    ) -> reqwest::header::HeaderMap {
        let nonce = "response-nonce";
        let body = std::str::from_utf8(body).unwrap();
        let message = format!("{timestamp}\n{nonce}\n{body}\n");
        let signature = STANDARD.encode(signer.sign(message.as_bytes()).to_bytes());
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "Wechatpay-Timestamp",
            timestamp.to_string().parse().unwrap(),
        );
        headers.insert("Wechatpay-Nonce", nonce.parse().unwrap());
        headers.insert("Wechatpay-Signature", signature.parse().unwrap());
        headers.insert("Wechatpay-Serial", key_id.parse().unwrap());
        headers
    }

    #[test]
    fn verifies_fresh_signed_api_response() {
        let (crypto, signer, key_id) = response_crypto();
        let body = br#"{"code_url":"weixin://wxpay/example"}"#;
        let headers = signed_response_headers(&signer, &key_id, Utc::now().timestamp(), body);
        verify_response(&crypto, &headers, body).unwrap();
    }

    #[test]
    fn rejects_stale_signed_api_response() {
        let (crypto, signer, key_id) = response_crypto();
        let body = br#"{"code_url":"weixin://wxpay/replayed"}"#;
        let headers = signed_response_headers(&signer, &key_id, Utc::now().timestamp() - 301, body);
        let error = verify_response(&crypto, &headers, body).unwrap_err();
        assert!(error.to_string().contains("five minute"));
    }

    #[test]
    fn rejects_unsigned_non_empty_api_response() {
        let (crypto, _, _) = response_crypto();
        let error = verify_response(
            &crypto,
            &reqwest::header::HeaderMap::new(),
            br#"{"code":"SIGN_ERROR"}"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("missing Wechatpay-Timestamp"));
    }

    #[test]
    fn unsigned_gateway_5xx_skips_verification_but_stays_strict_elsewhere() {
        let unsigned = reqwest::header::HeaderMap::new();
        // 完全无签名头的 5xx：来自网关/代理故障，跳过验签，
        // 后续归类为可重试的 Api 错误而非终局的 InvalidSignature
        assert!(!should_verify_response(StatusCode::BAD_GATEWAY, &unsigned));
        assert!(!should_verify_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &unsigned
        ));

        // 无签名头的 2xx/4xx：可能是伪造应答，必须仍走验签（并失败）
        assert!(should_verify_response(StatusCode::OK, &unsigned));
        assert!(should_verify_response(StatusCode::UNAUTHORIZED, &unsigned));

        // 携带任意一个签名头的 5xx：疑似微信业务后端应答，仍严格验签
        let mut partial = reqwest::header::HeaderMap::new();
        partial.insert("Wechatpay-Signature", "sig".parse().unwrap());
        assert!(should_verify_response(StatusCode::BAD_GATEWAY, &partial));
    }
}
