//! 支付宝API客户端
//
//! 封装支付宝开放平台API调用

use crate::config::AlipayConfig;
use crate::sign::{AlipaySigner, AlipayVerifier, sign_params, verify_params};
use chrono::{DateTime, FixedOffset, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;
use std::collections::HashMap;

/// 支付宝客户端
pub struct AlipayClient {
    config: AlipayConfig,
    signer: AlipaySigner,
    verifiers: Vec<AlipayVerifier>,
    http_client: Client,
}

impl AlipayClient {
    /// 创建新的支付宝客户端
    pub fn new(config: AlipayConfig) -> Result<Self, ClientError> {
        config.validate()?;

        let signer = AlipaySigner::from_pem(&config.private_key)
            .map_err(|e| ClientError::SignError(e.to_string()))?;

        let verifiers = std::iter::once(&config.alipay_public_key)
            .chain(config.previous_alipay_public_keys.iter())
            .map(|public_key| {
                AlipayVerifier::from_pem(public_key)
                    .map_err(|e| ClientError::SignError(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(ClientError::HttpError)?;

        Ok(Self {
            config,
            signer,
            verifiers,
            http_client,
        })
    }

    /// 获取公共请求参数
    fn common_params(&self, method: &str) -> Vec<(String, String)> {
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

        vec![
            ("app_id".to_string(), self.config.app_id.clone()),
            ("method".to_string(), method.to_string()),
            ("format".to_string(), "JSON".to_string()),
            ("charset".to_string(), self.config.charset.clone()),
            ("sign_type".to_string(), self.config.sign_type.clone()),
            ("timestamp".to_string(), timestamp),
            ("version".to_string(), self.config.version.clone()),
        ]
    }

    /// 构建请求参数并签名
    fn build_signed_params(
        &self,
        method: &str,
        biz_content: &str,
        extra_params: Vec<(String, String)>,
    ) -> Result<HashMap<String, String>, ClientError> {
        let mut params = self.common_params(method);
        params.push(("biz_content".to_string(), biz_content.to_string()));

        // 添加额外参数（如notify_url, return_url）
        for (k, v) in extra_params {
            params.push((k, v));
        }

        // 签名
        let sign = sign_params(&params, &self.signer)
            .map_err(|e| ClientError::SignError(e.to_string()))?;
        params.push(("sign".to_string(), sign));

        // 转换为HashMap
        Ok(params.into_iter().collect())
    }

    /// 发送API请求
    async fn send_request(
        &self,
        method: &str,
        biz_content: &str,
        extra_params: Vec<(String, String)>,
    ) -> Result<String, ClientError> {
        let params = self.build_signed_params(method, biz_content, extra_params)?;

        let response = self
            .http_client
            .post(self.config.gateway_url())
            .form(&params)
            .send()
            .await
            .map_err(ClientError::HttpError)?;

        response.text().await.map_err(ClientError::HttpError)
    }

    fn decode_signed_response<T: DeserializeOwned>(
        &self,
        text: &str,
        response_key: &str,
    ) -> Result<T, ClientError> {
        let envelope: HashMap<String, Box<RawValue>> = serde_json::from_str(text)
            .map_err(|e| ClientError::ParseError(format!("{e}: {text}")))?;
        let response = envelope
            .get(response_key)
            .ok_or_else(|| ClientError::ParseError(format!("missing {response_key}")))?;
        let sign_raw = envelope.get("sign").ok_or(ClientError::MissingSign)?;
        let signature: String = serde_json::from_str(sign_raw.get())
            .map_err(|e| ClientError::ParseError(format!("invalid response signature: {e}")))?;
        let verified = self
            .verifiers
            .iter()
            .try_fold(false, |verified, verifier| {
                if verified {
                    Ok(true)
                } else {
                    verifier
                        .verify(response.get(), &signature)
                        .map_err(|e| ClientError::SignError(e.to_string()))
                }
            })?;
        if !verified {
            return Err(ClientError::InvalidResponseSignature);
        }
        serde_json::from_str(response.get())
            .map_err(|e| ClientError::ParseError(format!("{e}: {}", response.get())))
    }

    /// 验证异步通知签名
    pub fn verify_notify(&self, params: &[(String, String)]) -> Result<bool, ClientError> {
        // 提取sign
        let sign = params
            .iter()
            .find(|(k, _)| k == "sign")
            .map(|(_, v)| v.as_str())
            .ok_or(ClientError::MissingSign)?;

        for verifier in &self.verifiers {
            match verify_params(params, sign, verifier) {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(error) => return Err(ClientError::SignError(error.to_string())),
            }
        }
        Ok(false)
    }

    /// 生成支付页面URL（电脑网站支付）
    pub fn page_pay_url(
        &self,
        out_trade_no: &str,
        total_amount: &str,
        subject: &str,
        body: Option<&str>,
        expires_at: DateTime<Utc>,
    ) -> Result<String, ClientError> {
        let biz_content = PagePayBizContent {
            out_trade_no: out_trade_no.to_string(),
            total_amount: total_amount.to_string(),
            subject: subject.to_string(),
            body: body.map(|s| s.to_string()),
            product_code: "FAST_INSTANT_TRADE_PAY".to_string(),
            time_expire: format_alipay_time(expires_at, true),
        };

        let biz_json = serde_json::to_string(&biz_content)
            .map_err(|e| ClientError::ParseError(e.to_string()))?;

        let mut extra_params = vec![("notify_url".to_string(), self.config.notify_url.clone())];

        if let Some(ref return_url) = self.config.return_url {
            extra_params.push(("return_url".to_string(), return_url.clone()));
        }

        let params = self.build_signed_params("alipay.trade.page.pay", &biz_json, extra_params)?;

        // 构建URL
        let query = urlencoding::encode_query(&params);
        Ok(format!("{}?{}", self.config.gateway_url(), query))
    }

    /// 手机网站支付URL
    pub fn wap_pay_url(
        &self,
        out_trade_no: &str,
        total_amount: &str,
        subject: &str,
        body: Option<&str>,
        expires_at: DateTime<Utc>,
    ) -> Result<String, ClientError> {
        let biz_content = WapPayBizContent {
            out_trade_no: out_trade_no.to_string(),
            total_amount: total_amount.to_string(),
            subject: subject.to_string(),
            body: body.map(|s| s.to_string()),
            product_code: "QUICK_WAP_WAY".to_string(),
            time_expire: format_alipay_time(expires_at, false),
        };

        let biz_json = serde_json::to_string(&biz_content)
            .map_err(|e| ClientError::ParseError(e.to_string()))?;

        let mut extra_params = vec![("notify_url".to_string(), self.config.notify_url.clone())];

        if let Some(ref return_url) = self.config.return_url {
            extra_params.push(("return_url".to_string(), return_url.clone()));
        }

        let params = self.build_signed_params("alipay.trade.wap.pay", &biz_json, extra_params)?;

        let query = urlencoding::encode_query(&params);
        Ok(format!("{}?{}", self.config.gateway_url(), query))
    }

    /// 查询订单状态
    pub async fn query_order(&self, out_trade_no: &str) -> Result<QueryResponse, ClientError> {
        let biz_content = QueryBizContent {
            out_trade_no: out_trade_no.to_string(),
        };

        let biz_json = serde_json::to_string(&biz_content)
            .map_err(|e| ClientError::ParseError(e.to_string()))?;

        let response = self
            .send_request("alipay.trade.query", &biz_json, vec![])
            .await?;
        self.decode_signed_response(&response, "alipay_trade_query_response")
    }

    /// 关闭订单
    pub async fn close_order(&self, out_trade_no: &str) -> Result<CloseResponse, ClientError> {
        let biz_content = CloseBizContent {
            out_trade_no: out_trade_no.to_string(),
        };

        let biz_json = serde_json::to_string(&biz_content)
            .map_err(|e| ClientError::ParseError(e.to_string()))?;

        let response = self
            .send_request("alipay.trade.close", &biz_json, vec![])
            .await?;
        self.decode_signed_response(&response, "alipay_trade_close_response")
    }

    /// 扫码支付（当面付）- 生成支付二维码
    ///
    /// 调用支付宝 alipay.trade.precreate 接口，生成支付二维码链接
    /// 用户使用支付宝扫码完成支付
    pub async fn precreate(
        &self,
        out_trade_no: &str,
        total_amount: &str,
        subject: &str,
        body: Option<&str>,
        timeout_minutes: i32,
    ) -> Result<PrecreateResponse, ClientError> {
        let biz_content = PrecreateBizContent {
            out_trade_no: out_trade_no.to_string(),
            total_amount: total_amount.to_string(),
            subject: subject.to_string(),
            body: body.map(|s| s.to_string()),
            timeout_express: format!("{timeout_minutes}m"),
        };

        let biz_json = serde_json::to_string(&biz_content)
            .map_err(|e| ClientError::ParseError(e.to_string()))?;

        let extra_params = vec![("notify_url".to_string(), self.config.notify_url.clone())];

        let response = self
            .send_request("alipay.trade.precreate", &biz_json, extra_params)
            .await?;
        self.decode_signed_response(&response, "alipay_trade_precreate_response")
    }

    /// 获取配置
    pub fn config(&self) -> &AlipayConfig {
        &self.config
    }
}

/// 电脑网站支付业务参数
#[derive(Debug, Serialize)]
struct PagePayBizContent {
    out_trade_no: String,
    total_amount: String,
    subject: String,
    body: Option<String>,
    product_code: String,
    time_expire: String,
}

/// 手机网站支付业务参数
#[derive(Debug, Serialize)]
struct WapPayBizContent {
    out_trade_no: String,
    total_amount: String,
    subject: String,
    body: Option<String>,
    product_code: String,
    time_expire: String,
}

/// 查询订单业务参数
#[derive(Debug, Serialize)]
struct QueryBizContent {
    out_trade_no: String,
}

/// 关闭订单业务参数
#[derive(Debug, Serialize)]
struct CloseBizContent {
    out_trade_no: String,
}

/// 扫码支付业务参数
#[derive(Debug, Serialize)]
struct PrecreateBizContent {
    out_trade_no: String,
    total_amount: String,
    subject: String,
    body: Option<String>,
    timeout_express: String,
}

fn format_alipay_time(expires_at: DateTime<Utc>, include_seconds: bool) -> String {
    let china_standard_time =
        FixedOffset::east_opt(8 * 60 * 60).expect("China standard time offset is valid");
    let expires_at = expires_at.with_timezone(&china_standard_time);
    if include_seconds {
        expires_at.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        expires_at.format("%Y-%m-%d %H:%M").to_string()
    }
}

/// 查询订单响应
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QueryResponse {
    /// 响应码
    pub code: String,
    /// 响应消息
    pub msg: String,
    /// 业务结果码
    #[serde(default)]
    pub sub_code: Option<String>,
    /// 业务结果描述
    #[serde(default)]
    pub sub_msg: Option<String>,
    /// 商户订单号
    #[serde(default)]
    pub out_trade_no: Option<String>,
    /// 支付宝交易号
    #[serde(default)]
    pub trade_no: Option<String>,
    /// 交易状态
    /// WAIT_BUYER_PAY: 交易创建，等待买家付款
    /// TRADE_CLOSED: 未付款交易超时关闭
    /// TRADE_SUCCESS: 交易支付成功
    /// TRADE_FINISHED: 交易结束
    #[serde(default)]
    pub trade_status: Option<String>,
    /// 交易金额
    #[serde(default)]
    pub total_amount: Option<String>,
    /// 买家支付宝账号
    #[serde(default)]
    pub buyer_logon_id: Option<String>,
    /// 买家支付宝用户ID
    #[serde(default)]
    pub buyer_user_id: Option<String>,
    /// 交易付款时间
    #[serde(default)]
    pub send_pay_date: Option<String>,
}

impl QueryResponse {
    /// 检查是否成功
    pub fn is_success(&self) -> bool {
        self.code == "10000"
    }

    /// 检查交易是否成功
    pub fn is_trade_success(&self) -> bool {
        self.trade_status.as_deref() == Some("TRADE_SUCCESS")
            || self.trade_status.as_deref() == Some("TRADE_FINISHED")
    }
}

/// 关闭订单响应
#[derive(Debug, Clone, Deserialize)]
pub struct CloseResponse {
    pub code: String,
    pub msg: String,
    #[serde(default)]
    pub sub_code: Option<String>,
    #[serde(default)]
    pub sub_msg: Option<String>,
}

impl CloseResponse {
    pub fn is_success(&self) -> bool {
        self.code == "10000"
    }
}

/// 扫码支付响应
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PrecreateResponse {
    /// 响应码
    pub code: String,
    /// 响应消息
    pub msg: String,
    /// 业务结果码
    #[serde(default)]
    pub sub_code: Option<String>,
    /// 业务结果描述
    #[serde(default)]
    pub sub_msg: Option<String>,
    /// 商户订单号
    #[serde(default)]
    pub out_trade_no: Option<String>,
    /// 支付宝交易号（支付成功后才有）
    #[serde(default)]
    pub trade_no: Option<String>,
    /// 支付二维码链接（重要：用户扫码支付的二维码内容）
    #[serde(default)]
    pub qr_code: Option<String>,
}

impl PrecreateResponse {
    /// 检查是否成功生成二维码
    pub fn is_success(&self) -> bool {
        self.code == "10000"
    }

    /// 获取二维码链接
    pub fn qr_code(&self) -> Option<&str> {
        self.qr_code.as_deref()
    }
}

/// 客户端错误
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("配置错误: {0}")]
    ConfigError(String),
    #[error("签名错误: {0}")]
    SignError(String),
    #[error("HTTP错误: {0}")]
    HttpError(#[from] reqwest::Error),
    #[error("解析错误: {0}")]
    ParseError(String),
    #[error("缺少签名参数")]
    MissingSign,
    #[error("支付宝响应签名验证失败")]
    InvalidResponseSignature,
}

impl From<crate::config::ConfigError> for ClientError {
    fn from(e: crate::config::ConfigError) -> Self {
        ClientError::ConfigError(e.to_string())
    }
}

/// URL编码辅助模块
mod urlencoding {
    use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

    pub fn encode_query(params: &std::collections::HashMap<String, String>) -> String {
        let mut pairs: Vec<_> = params
            .iter()
            .map(|(k, v)| {
                let encoded_key = utf8_percent_encode(k, NON_ALPHANUMERIC).to_string();
                let encoded_value = utf8_percent_encode(v, NON_ALPHANUMERIC).to_string();
                format!("{}={}", encoded_key, encoded_value)
            })
            .collect();
        pairs.sort();
        pairs.join("&")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::AlipaySigner;
    use rsa::{
        RsaPrivateKey,
        pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding},
        rand_core::OsRng,
    };

    fn signed_response_client() -> (AlipayClient, AlipaySigner) {
        let merchant = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let platform = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let config = AlipayConfig {
            app_id: "test-app".to_string(),
            private_key: merchant.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
            alipay_public_key: platform
                .to_public_key()
                .to_public_key_pem(LineEnding::LF)
                .unwrap(),
            notify_url: "https://example.com/alipay-notify".to_string(),
            ..AlipayConfig::default()
        };
        let signer =
            AlipaySigner::from_pem(platform.to_pkcs8_pem(LineEnding::LF).unwrap().as_str())
                .unwrap();
        (AlipayClient::new(config).unwrap(), signer)
    }

    #[test]
    fn test_query_response_is_success() {
        let response = QueryResponse {
            code: "10000".to_string(),
            msg: "Success".to_string(),
            sub_code: None,
            sub_msg: None,
            out_trade_no: Some("123".to_string()),
            trade_no: Some("456".to_string()),
            trade_status: Some("TRADE_SUCCESS".to_string()),
            total_amount: Some("100.00".to_string()),
            buyer_logon_id: None,
            buyer_user_id: None,
            send_pay_date: None,
        };

        assert!(response.is_success());
        assert!(response.is_trade_success());
    }

    #[test]
    fn verifies_raw_api_response_before_decoding() {
        let (client, platform) = signed_response_client();
        let raw = r#"{"code":"10000","msg":"Success","out_trade_no":"KC1","trade_status":"TRADE_SUCCESS","total_amount":"1.00"}"#;
        let signature = platform.sign(raw).unwrap();
        let envelope = format!(
            r#"{{"alipay_trade_query_response":{raw},"sign":{}}}"#,
            serde_json::to_string(&signature).unwrap()
        );
        let response: QueryResponse = client
            .decode_signed_response(&envelope, "alipay_trade_query_response")
            .unwrap();
        assert_eq!(response.out_trade_no.as_deref(), Some("KC1"));

        let tampered = envelope.replace("1.00", "9.00");
        assert!(matches!(
            client
                .decode_signed_response::<QueryResponse>(&tampered, "alipay_trade_query_response"),
            Err(ClientError::InvalidResponseSignature)
        ));
    }

    #[test]
    fn retained_public_key_verifies_delayed_response_after_rotation() {
        let merchant = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let current_platform = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let previous_platform = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let config = AlipayConfig {
            app_id: "test-app".to_string(),
            private_key: merchant.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
            alipay_public_key: current_platform
                .to_public_key()
                .to_public_key_pem(LineEnding::LF)
                .unwrap(),
            previous_alipay_public_keys: vec![
                previous_platform
                    .to_public_key()
                    .to_public_key_pem(LineEnding::LF)
                    .unwrap(),
            ],
            notify_url: "https://example.com/alipay-notify".to_string(),
            ..AlipayConfig::default()
        };
        let previous_signer = AlipaySigner::from_pem(
            previous_platform
                .to_pkcs8_pem(LineEnding::LF)
                .unwrap()
                .as_str(),
        )
        .unwrap();
        let client = AlipayClient::new(config).unwrap();
        let raw = r#"{"code":"10000","msg":"Success","out_trade_no":"KC-OLD"}"#;
        let signature = previous_signer.sign(raw).unwrap();
        let envelope = format!(
            r#"{{"alipay_trade_query_response":{raw},"sign":{}}}"#,
            serde_json::to_string(&signature).unwrap()
        );

        let response: QueryResponse = client
            .decode_signed_response(&envelope, "alipay_trade_query_response")
            .unwrap();
        assert_eq!(response.out_trade_no.as_deref(), Some("KC-OLD"));

        let mut notify_params = vec![
            ("app_id".to_string(), "test-app".to_string()),
            ("out_trade_no".to_string(), "KC-OLD".to_string()),
            ("trade_status".to_string(), "TRADE_SUCCESS".to_string()),
        ];
        let notify_signature = sign_params(&notify_params, &previous_signer).unwrap();
        notify_params.push(("sign".to_string(), notify_signature));
        assert!(client.verify_notify(&notify_params).unwrap());
    }

    #[test]
    fn all_payment_scenes_serialize_provider_expiration() {
        let expires_at = DateTime::parse_from_rfc3339("2026-07-27T04:05:06Z")
            .unwrap()
            .with_timezone(&Utc);
        let page = PagePayBizContent {
            out_trade_no: "page-order".to_string(),
            total_amount: "1.00".to_string(),
            subject: "recharge".to_string(),
            body: None,
            product_code: "FAST_INSTANT_TRADE_PAY".to_string(),
            time_expire: format_alipay_time(expires_at, true),
        };
        let wap = WapPayBizContent {
            out_trade_no: "wap-order".to_string(),
            total_amount: "1.00".to_string(),
            subject: "recharge".to_string(),
            body: None,
            product_code: "QUICK_WAP_WAY".to_string(),
            time_expire: format_alipay_time(expires_at, false),
        };
        let qr = PrecreateBizContent {
            out_trade_no: "qr-order".to_string(),
            total_amount: "1.00".to_string(),
            subject: "recharge".to_string(),
            body: None,
            timeout_express: "30m".to_string(),
        };

        assert_eq!(
            serde_json::to_value(page).unwrap()["time_expire"],
            "2026-07-27 12:05:06"
        );
        assert_eq!(
            serde_json::to_value(wap).unwrap()["time_expire"],
            "2026-07-27 12:05"
        );
        assert_eq!(serde_json::to_value(qr).unwrap()["timeout_express"], "30m");
    }
}
