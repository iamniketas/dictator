//! LLM module - Ollama API client for text correction

use anyhow::Result;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use tracing::info;

/// Ollama API request
#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
}

/// Ollama API response
#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

/// Ollama client for text correction
pub struct OllamaClient {
    client: Client,
    base_url: String,
    model: String,
}

impl OllamaClient {
    /// Create new Ollama client
    pub fn new(base_url: &str, model: &str) -> Self {
        let client = Client::builder()
            .no_proxy() // Disable system proxy for localhost
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            base_url: base_url.to_string(),
            model: model.to_string(),
        }
    }

    /// Correct transcribed text using LLM
    pub fn correct_text(&self, raw_text: &str) -> Result<String> {
        if raw_text.trim().is_empty() {
            return Ok(String::new());
        }

        let prompt = format!(
            r#"Исправь ошибки в следующем тексте, полученном из голосовой транскрипции.
Исправь опечатки, пунктуацию и грамматику. Не меняй смысл текста.
Верни ТОЛЬКО исправленный текст, без объяснений.

Текст: {}"#,
            raw_text
        );

        info!("Sending to Ollama for correction...");

        let request = OllamaRequest {
            model: self.model.clone(),
            prompt,
            stream: false,
        };

        let url = format!("{}/api/generate", self.base_url);

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()?
            .json::<OllamaResponse>()?;

        let corrected = response.response.trim().to_string();
        info!("Text corrected: {} chars -> {} chars", raw_text.len(), corrected.len());

        Ok(corrected)
    }

    /// Check if Ollama server is available
    pub fn health_check(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        self.client.get(&url).send().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = OllamaClient::new("http://localhost:11434", "qwen3:30b");
        assert_eq!(client.base_url, "http://localhost:11434");
        assert_eq!(client.model, "qwen3:30b");
    }
}