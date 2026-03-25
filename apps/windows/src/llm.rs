// AGENT: kimi | TASK: task_036bea4a178e | TIMESTAMP: 2026-01-29T20:13:24.544516
// AUTO-GENERATED: Do not edit manually. Delegate changes via orchestrator.
// SOURCE: http://localhost:8000/task/task_036bea4a178e/report

use anyhow::Result;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};

/// Ollama API request
#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
}

/// Ollama API response
#[derive(Deserialize)]
#[allow(dead_code)]
struct OllamaResponse {
    response: String,
    done: Option<bool>,
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
            .no_proxy()
            .timeout(std::time::Duration::from_secs(120))
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

        debug!("Ollama request URL: {}", url);
        debug!("Ollama request model: {}", self.model);

        let response = self.client.post(&url).json(&request).send().map_err(|e| {
            error!("Failed to send request to Ollama: {}", e);
            anyhow::anyhow!("Ollama request failed: {}", e)
        })?;

        let status = response.status();
        debug!("Ollama response status: {}", status);

        if !status.is_success() {
            let error_text = response.text().unwrap_or_default();
            error!("Ollama returned error status {}: {}", status, error_text);
            return Err(anyhow::anyhow!(
                "Ollama API error: {} - {}",
                status,
                error_text
            ));
        }

        let ollama_response: OllamaResponse = response.json().map_err(|e| {
            error!("Failed to parse Ollama response: {}", e);
            anyhow::anyhow!("Failed to parse Ollama response: {}", e)
        })?;

        let corrected = ollama_response.response.trim().to_string();
        info!(
            "Text corrected: {} chars -> {} chars",
            raw_text.len(),
            corrected.len()
        );
        debug!("Corrected text: {}", corrected);

        Ok(corrected)
    }

    /// Check if Ollama server is available
    pub fn health_check(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        match self
            .client
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
        {
            Ok(response) => {
                let available = response.status().is_success();
                if available {
                    debug!("Ollama health check passed");
                } else {
                    error!(
                        "Ollama health check failed with status: {}",
                        response.status()
                    );
                }
                available
            }
            Err(e) => {
                error!("Ollama health check failed: {}", e);
                false
            }
        }
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
