//! Generic HTTP ASR sidecar transcription backend.
//!
//! This backend keeps the Rust daemon independent from Python/PyTorch by
//! sending WAV audio to a local HTTP sidecar.

use async_trait::async_trait;
use reqwest::multipart;
use serde::Deserialize;
use tracing::{debug, warn};

use super::{TranscriptionBackend, TranscriptionConfig};

/// Keep a guardrail so a runaway recording does not create an unbounded
/// multipart request.
const MAX_FILE_SIZE: usize = 1024 * 1024 * 1024;

/// Generic HTTP ASR sidecar transcription backend.
pub struct AsrSidecarBackend {
    client: reqwest::Client,
    url: String,
    api_key: Option<String>,
}

impl AsrSidecarBackend {
    /// Create a new sidecar backend with the transcription URL and optional API key.
    pub fn new(url: String, api_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            url,
            api_key: api_key
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty()),
        }
    }
}

/// Response from the ASR sidecar.
#[derive(Debug, Deserialize)]
pub struct AsrSidecarResponse {
    /// Plain text transcript. Sidecars may also return richer diarized output,
    /// but whisrs currently consumes the flattened text for typing.
    pub text: String,
}

#[derive(Debug, Deserialize)]
struct AsrSidecarErrorResponse {
    error: Option<String>,
    detail: Option<serde_json::Value>,
}

impl AsrSidecarErrorResponse {
    fn message(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        match &self.detail {
            Some(serde_json::Value::String(detail)) => detail.clone(),
            Some(detail) => detail.to_string(),
            None => "unknown sidecar error".to_string(),
        }
    }
}

#[async_trait]
impl TranscriptionBackend for AsrSidecarBackend {
    async fn transcribe(
        &self,
        audio: &[u8],
        config: &TranscriptionConfig,
    ) -> anyhow::Result<String> {
        if audio.len() > MAX_FILE_SIZE {
            anyhow::bail!(
                "audio file too large ({} bytes, max {} bytes / 1GB)",
                audio.len(),
                MAX_FILE_SIZE
            );
        }

        if audio.is_empty() {
            anyhow::bail!("cannot transcribe empty audio");
        }

        if self.url.trim().is_empty() {
            anyhow::bail!("no ASR sidecar URL configured");
        }

        debug!(
            "sending {} bytes to ASR sidecar (model={}, language={})",
            audio.len(),
            config.model,
            config.language
        );

        let file_part = multipart::Part::bytes(audio.to_vec())
            .file_name("audio.wav")
            .mime_str("audio/wav")?;

        let mut form = multipart::Form::new()
            .part("file", file_part)
            .text("model", config.model.clone());

        if config.language != "auto" {
            form = form.text("language", config.language.clone());
        }
        if let Some(prompt) = &config.prompt {
            form = form.text("hotwords", prompt.clone());
        }

        // Some OpenAI-compatible endpoints 307-redirect when the URL has a trailing
        // slash, which can downgrade http→https and cause reqwest to abort multipart
        // POSTs. Trim trailing slashes so both forms work without a redirect.
        let effective_url = self.url.trim_end_matches('/');
        if effective_url != self.url {
            debug!("trimmed trailing slashes from ASR sidecar URL: {effective_url}");
        }
        let mut request = self.client.post(effective_url);
        if let Some(key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {key}"));
        }
        let response = request.multipart(form).send().await?;
        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            if let Ok(err_resp) = serde_json::from_str::<AsrSidecarErrorResponse>(&body) {
                anyhow::bail!(
                    "ASR sidecar error ({}): {}",
                    status.as_u16(),
                    err_resp.message()
                );
            }
            anyhow::bail!("ASR sidecar error ({}): {}", status.as_u16(), body);
        }

        let parsed: AsrSidecarResponse = serde_json::from_str(&body)?;
        let text = parsed.text.trim().to_string();

        if text.is_empty() {
            warn!("ASR sidecar returned empty transcription");
        }

        Ok(text)
    }

    // Uses the default transcribe_stream (collect + transcribe). Model-specific
    // streaming behavior belongs in the sidecar process.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn transcribe_rejects_empty_audio() {
        let backend = AsrSidecarBackend::new("http://127.0.0.1:8765/transcribe".to_string(), None);
        let config = TranscriptionConfig {
            language: "en".to_string(),
            model: "test-asr-model".to_string(),
            prompt: None,
        };
        let err = backend.transcribe(&[], &config).await.unwrap_err();
        assert!(err.to_string().contains("empty audio"));
    }

    #[tokio::test]
    async fn transcribe_rejects_missing_url() {
        let backend = AsrSidecarBackend::new(String::new(), None);
        let config = TranscriptionConfig {
            language: "en".to_string(),
            model: "test-asr-model".to_string(),
            prompt: None,
        };
        let err = backend.transcribe(&[1, 2, 3], &config).await.unwrap_err();
        assert!(err.to_string().contains("sidecar URL"));
    }

    #[test]
    fn empty_api_key_is_normalized_to_none() {
        let backend = AsrSidecarBackend::new(
            "http://127.0.0.1:8765/transcribe".to_string(),
            Some(String::new()),
        );
        assert!(backend.api_key.is_none());
    }

    #[test]
    fn api_key_is_stored_when_present() {
        let backend = AsrSidecarBackend::new(
            "http://127.0.0.1:8765/transcribe".to_string(),
            Some("sk-test-key".to_string()),
        );
        assert_eq!(backend.api_key.as_deref(), Some("sk-test-key"));
    }

    #[test]
    fn whitespace_only_api_key_is_normalized_to_none() {
        let backend = AsrSidecarBackend::new(
            "http://127.0.0.1:8765/transcribe".to_string(),
            Some("   ".to_string()),
        );
        assert!(backend.api_key.is_none());
    }

    #[test]
    fn api_key_whitespace_is_trimmed() {
        let backend = AsrSidecarBackend::new(
            "http://127.0.0.1:8765/transcribe".to_string(),
            Some("  sk-test-key  ".to_string()),
        );
        assert_eq!(backend.api_key.as_deref(), Some("sk-test-key"));
    }

    #[test]
    fn parse_asr_sidecar_response() {
        let body = r#"{"text": "Hello world"}"#;
        let parsed: AsrSidecarResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.text, "Hello world");
    }

    #[test]
    fn parse_asr_sidecar_error() {
        let body = r#"{"error": "model failed to load"}"#;
        let parsed: AsrSidecarErrorResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.message(), "model failed to load");
    }

    #[test]
    fn parse_fastapi_error_detail() {
        let body = r#"{"detail": "request asked for wrong model"}"#;
        let parsed: AsrSidecarErrorResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.message(), "request asked for wrong model");
    }
}
