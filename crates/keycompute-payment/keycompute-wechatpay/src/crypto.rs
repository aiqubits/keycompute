use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use base64::{Engine, engine::general_purpose::STANDARD};
use rsa::{
    RsaPrivateKey, RsaPublicKey,
    pkcs1v15::{Signature, SigningKey, VerifyingKey},
    pkcs8::{DecodePrivateKey, DecodePublicKey},
    signature::{SignatureEncoding, Signer, Verifier},
};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::{WechatPayConfig, WechatPayError};

#[derive(Debug, Clone)]
pub struct NotifyHeaders {
    pub timestamp: String,
    pub nonce: String,
    pub signature: String,
    pub serial: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WechatPayNotify {
    pub id: String,
    pub event_type: String,
    pub resource: EncryptedResource,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EncryptedResource {
    pub algorithm: String,
    pub ciphertext: String,
    pub nonce: String,
    #[serde(default)]
    pub associated_data: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VerifiedNotify {
    pub appid: String,
    pub mchid: String,
    pub out_trade_no: String,
    pub transaction_id: String,
    pub trade_state: String,
    pub trade_type: String,
    pub amount: NotifyAmount,
    pub success_time: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NotifyAmount {
    pub total: i64,
    pub payer_total: Option<i64>,
    pub currency: String,
    pub payer_currency: Option<String>,
}

pub(crate) struct WechatCrypto {
    signer: SigningKey<Sha256>,
    callback_keys: Vec<CallbackCrypto>,
}

struct CallbackCrypto {
    verifier: VerifyingKey<Sha256>,
    api_v3_key: [u8; 32],
    public_key_id: String,
}

impl WechatCrypto {
    pub(crate) fn new(config: &WechatPayConfig) -> Result<Self, WechatPayError> {
        let private_key = RsaPrivateKey::from_pkcs8_pem(&config.merchant_private_key)
            .map_err(|e| WechatPayError::Crypto(format!("invalid merchant private key: {e}")))?;
        let callback_keys = std::iter::once((
            config.wechatpay_public_key_id.as_str(),
            config.wechatpay_public_key.as_str(),
            config.api_v3_key.as_str(),
        ))
        .chain(config.previous_callback_keys.iter().map(|key| {
            (
                key.public_key_id.as_str(),
                key.public_key.as_str(),
                key.api_v3_key.as_str(),
            )
        }))
        .map(|(public_key_id, public_key, api_v3_key)| {
            let public_key = RsaPublicKey::from_public_key_pem(public_key).map_err(|e| {
                WechatPayError::Crypto(format!("invalid WeChat Pay public key: {e}"))
            })?;
            let api_v3_key: [u8; 32] = api_v3_key
                .as_bytes()
                .try_into()
                .map_err(|_| WechatPayError::Crypto("API v3 key must be 32 bytes".to_string()))?;
            Ok(CallbackCrypto {
                verifier: VerifyingKey::new(public_key),
                api_v3_key,
                public_key_id: public_key_id.to_string(),
            })
        })
        .collect::<Result<Vec<_>, WechatPayError>>()?;

        Ok(Self {
            signer: SigningKey::new(private_key),
            callback_keys,
        })
    }

    pub(crate) fn sign(&self, message: &str) -> String {
        STANDARD.encode(self.signer.sign(message.as_bytes()).to_bytes())
    }

    pub(crate) fn verify(
        &self,
        serial: &str,
        message: &str,
        signature: &str,
    ) -> Result<(), WechatPayError> {
        let callback_key = self
            .callback_keys
            .iter()
            .find(|key| key.public_key_id == serial)
            .ok_or_else(|| {
                WechatPayError::InvalidSignature(format!("unexpected WeChat Pay key id {serial}"))
            })?;
        let bytes = STANDARD
            .decode(signature)
            .map_err(|e| WechatPayError::InvalidSignature(e.to_string()))?;
        let signature = Signature::try_from(bytes.as_slice())
            .map_err(|e| WechatPayError::InvalidSignature(e.to_string()))?;
        callback_key
            .verifier
            .verify(message.as_bytes(), &signature)
            .map_err(|_| WechatPayError::InvalidSignature("signature mismatch".to_string()))
    }

    pub(crate) fn verify_notify(
        &self,
        headers: &NotifyHeaders,
        raw_body: &[u8],
    ) -> Result<WechatPayNotify, WechatPayError> {
        let timestamp: i64 = headers
            .timestamp
            .parse()
            .map_err(|_| WechatPayError::InvalidSignature("invalid timestamp".to_string()))?;
        let now = chrono::Utc::now().timestamp();
        if now.abs_diff(timestamp) > 300 {
            return Err(WechatPayError::InvalidSignature(
                "callback timestamp outside five minute window".to_string(),
            ));
        }
        let body =
            std::str::from_utf8(raw_body).map_err(|e| WechatPayError::Decode(e.to_string()))?;
        let message = format!("{}\n{}\n{}\n", headers.timestamp, headers.nonce, body);
        self.verify(&headers.serial, &message, &headers.signature)?;
        serde_json::from_slice(raw_body).map_err(|e| WechatPayError::Decode(e.to_string()))
    }

    pub(crate) fn decrypt_notify(
        &self,
        resource: &EncryptedResource,
    ) -> Result<VerifiedNotify, WechatPayError> {
        if resource.algorithm != "AEAD_AES_256_GCM" {
            return Err(WechatPayError::Decode(format!(
                "unsupported algorithm {}",
                resource.algorithm
            )));
        }
        if resource.nonce.len() != 12 {
            return Err(WechatPayError::Decode(
                "callback nonce must contain exactly 12 bytes".to_string(),
            ));
        }
        let ciphertext = STANDARD
            .decode(&resource.ciphertext)
            .map_err(|e| WechatPayError::Decode(e.to_string()))?;
        for callback_key in &self.callback_keys {
            let cipher = Aes256Gcm::new_from_slice(&callback_key.api_v3_key)
                .map_err(|e| WechatPayError::Crypto(e.to_string()))?;
            if let Ok(plaintext) = cipher.decrypt(
                Nonce::from_slice(resource.nonce.as_bytes()),
                aes_gcm::aead::Payload {
                    msg: &ciphertext,
                    aad: resource.associated_data.as_bytes(),
                },
            ) {
                return serde_json::from_slice(&plaintext)
                    .map_err(|e| WechatPayError::Decode(e.to_string()));
            }
        }
        Err(WechatPayError::Decode(
            "callback decryption failed".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::Payload;
    use rand::rngs::OsRng;
    use rsa::{
        pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding},
        signature::{SignatureEncoding, Signer},
    };

    fn config_and_platform_signer() -> (WechatPayConfig, SigningKey<Sha256>) {
        let merchant = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let platform = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let config = WechatPayConfig {
            appid: "wx-test".to_string(),
            mchid: "1900000109".to_string(),
            merchant_serial_no: "merchant-serial".to_string(),
            merchant_private_key: merchant.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
            api_v3_key: "0123456789abcdef0123456789abcdef".to_string(),
            wechatpay_public_key_id: "PUB_KEY_ID_TEST".to_string(),
            wechatpay_public_key: platform
                .to_public_key()
                .to_public_key_pem(LineEnding::LF)
                .unwrap(),
            previous_callback_keys: Vec::new(),
            notify_url: "https://example.com/api/v1/payments/notify/wechatpay".to_string(),
            timeout_minutes: 15,
        };
        (config, SigningKey::new(platform))
    }

    #[test]
    fn verifies_and_decrypts_callback() {
        let (config, platform_signer) = config_and_platform_signer();
        let crypto = WechatCrypto::new(&config).unwrap();
        let plaintext = serde_json::json!({
            "appid": config.appid,
            "mchid": config.mchid,
            "out_trade_no": "KC20260101000000ABCD1234",
            "transaction_id": "42000000000001",
            "trade_state": "SUCCESS",
            "trade_type": "NATIVE",
            "amount": {"total": 100, "currency": "CNY"},
            "success_time": "2026-01-01T00:00:00+08:00"
        })
        .to_string();
        let nonce = "0123456789ab";
        let associated_data = "transaction";
        let cipher = Aes256Gcm::new_from_slice(config.api_v3_key.as_bytes()).unwrap();
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(nonce.as_bytes()),
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: associated_data.as_bytes(),
                },
            )
            .unwrap();
        let body = serde_json::json!({
            "id": "notify-id",
            "event_type": "TRANSACTION.SUCCESS",
            "resource": {
                "algorithm": "AEAD_AES_256_GCM",
                "ciphertext": STANDARD.encode(ciphertext),
                "nonce": nonce,
                "associated_data": associated_data
            }
        })
        .to_string();
        let timestamp = chrono::Utc::now().timestamp().to_string();
        let callback_nonce = "callback-nonce";
        let message = format!("{timestamp}\n{callback_nonce}\n{body}\n");
        let signature = STANDARD.encode(platform_signer.sign(message.as_bytes()).to_bytes());
        let headers = NotifyHeaders {
            timestamp,
            nonce: callback_nonce.to_string(),
            signature,
            serial: config.wechatpay_public_key_id,
        };
        let envelope = crypto.verify_notify(&headers, body.as_bytes()).unwrap();
        let notify = crypto.decrypt_notify(&envelope.resource).unwrap();
        assert_eq!(notify.trade_state, "SUCCESS");
        assert_eq!(notify.amount.total, 100);
    }

    #[test]
    fn rejects_expired_callback_timestamp() {
        let (config, _) = config_and_platform_signer();
        let crypto = WechatCrypto::new(&config).unwrap();
        let headers = NotifyHeaders {
            timestamp: (chrono::Utc::now().timestamp() - 301).to_string(),
            nonce: "nonce".to_string(),
            signature: "invalid".to_string(),
            serial: config.wechatpay_public_key_id,
        };
        let error = crypto.verify_notify(&headers, b"{}").unwrap_err();
        assert!(error.to_string().contains("five minute"));
    }

    #[test]
    fn rejects_extreme_callback_timestamp_without_overflow() {
        let (config, _) = config_and_platform_signer();
        let crypto = WechatCrypto::new(&config).unwrap();
        let headers = NotifyHeaders {
            timestamp: i64::MIN.to_string(),
            nonce: "nonce".to_string(),
            signature: "invalid".to_string(),
            serial: config.wechatpay_public_key_id,
        };

        let error = crypto.verify_notify(&headers, b"{}").unwrap_err();
        assert!(error.to_string().contains("five minute"));
    }

    #[test]
    fn rejects_invalid_resource_nonce_lengths_without_panicking() {
        let (config, _) = config_and_platform_signer();
        let crypto = WechatCrypto::new(&config).unwrap();

        for nonce in ["", "12345678901", "1234567890123"] {
            let resource = EncryptedResource {
                algorithm: "AEAD_AES_256_GCM".to_string(),
                ciphertext: STANDARD.encode([0_u8; 16]),
                nonce: nonce.to_string(),
                associated_data: String::new(),
            };
            let error = crypto.decrypt_notify(&resource).unwrap_err();
            assert!(error.to_string().contains("exactly 12 bytes"));
        }
    }

    #[test]
    fn retained_callback_keys_verify_and_decrypt_delayed_notifications() {
        let (mut config, _) = config_and_platform_signer();
        let previous_platform = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let previous_signer = SigningKey::<Sha256>::new(previous_platform.clone());
        let previous_api_key = "abcdef0123456789abcdef0123456789";
        config.previous_callback_keys = vec![crate::WechatPayCallbackKey {
            public_key_id: "OLD_KEY_ID".to_string(),
            public_key: previous_platform
                .to_public_key()
                .to_public_key_pem(LineEnding::LF)
                .unwrap(),
            api_v3_key: previous_api_key.to_string(),
        }];
        let crypto = WechatCrypto::new(&config).unwrap();
        let plaintext = serde_json::json!({
            "appid": config.appid,
            "mchid": config.mchid,
            "out_trade_no": "KC-ROTATED",
            "transaction_id": "WX-OLD",
            "trade_state": "SUCCESS",
            "trade_type": "NATIVE",
            "amount": {"total": 100, "currency": "CNY"}
        })
        .to_string();
        let resource_nonce = "0123456789ab";
        let cipher = Aes256Gcm::new_from_slice(previous_api_key.as_bytes()).unwrap();
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(resource_nonce.as_bytes()),
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: b"transaction",
                },
            )
            .unwrap();
        let body = serde_json::json!({
            "id": "old-notification",
            "event_type": "TRANSACTION.SUCCESS",
            "resource": {
                "algorithm": "AEAD_AES_256_GCM",
                "ciphertext": STANDARD.encode(ciphertext),
                "nonce": resource_nonce,
                "associated_data": "transaction"
            }
        })
        .to_string();
        let timestamp = chrono::Utc::now().timestamp().to_string();
        let nonce = "callback-nonce";
        let message = format!("{timestamp}\n{nonce}\n{body}\n");
        let headers = NotifyHeaders {
            timestamp,
            nonce: nonce.to_string(),
            signature: STANDARD.encode(previous_signer.sign(message.as_bytes()).to_bytes()),
            serial: "OLD_KEY_ID".to_string(),
        };

        let envelope = crypto.verify_notify(&headers, body.as_bytes()).unwrap();
        let notify = crypto.decrypt_notify(&envelope.resource).unwrap();
        assert_eq!(notify.out_trade_no, "KC-ROTATED");
        assert_eq!(notify.transaction_id, "WX-OLD");
    }
}
