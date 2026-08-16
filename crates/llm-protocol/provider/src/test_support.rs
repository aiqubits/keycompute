//! Test-only transport fixtures for protocol adapters.

use crate::{ByteStream, GetBinaryResponse, HttpTransport};
use async_trait::async_trait;
use keycompute_types::{KeyComputeError, Result};
use std::{sync::Mutex, time::Duration};

pub type RecordedRequest = (String, Vec<(String, String)>);

/// A deterministic transport for tests that exercise `GET /models` requests.
#[derive(Debug)]
pub struct RecordingGetTransport {
    body: Vec<u8>,
    requests: Mutex<Vec<RecordedRequest>>,
}

impl RecordingGetTransport {
    pub fn new(body: impl Into<Vec<u8>>) -> Self {
        Self {
            body: body.into(),
            requests: Mutex::new(Vec::new()),
        }
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("request lock poisoned").clone()
    }
}

#[async_trait]
impl HttpTransport for RecordingGetTransport {
    async fn post_json(
        &self,
        _url: &str,
        _headers: Vec<(String, String)>,
        _body: String,
    ) -> Result<String> {
        Err(KeyComputeError::ProviderError(
            "RecordingGetTransport only supports GET requests".into(),
        ))
    }

    async fn post_stream(
        &self,
        _url: &str,
        _headers: Vec<(String, String)>,
        _body: String,
    ) -> Result<ByteStream> {
        Err(KeyComputeError::ProviderError(
            "RecordingGetTransport only supports GET requests".into(),
        ))
    }

    fn request_timeout(&self) -> Duration {
        Duration::from_secs(1)
    }

    fn stream_timeout(&self) -> Duration {
        Duration::from_secs(1)
    }

    async fn get_binary(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
    ) -> Result<GetBinaryResponse> {
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push((url.to_string(), headers));
        Ok(GetBinaryResponse {
            body: self.body.clone(),
            content_type: Some("application/json".into()),
        })
    }
}
