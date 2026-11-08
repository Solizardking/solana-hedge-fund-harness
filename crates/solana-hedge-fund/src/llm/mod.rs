//! Grok (xAI) narrative layer. Advisory only — it never changes a desk
//! score, mirroring sol_fund/llm/grok.py. When XAI_API_KEY is unset the
//! client ABSTAINS: `analyze` returns None and `status` reports
//! "not_configured". An unconfigured Grok must never be dressed up as an
//! opinion.

use serde_json::{json, Value};

pub struct GrokClient {
    api_key: Option<String>,
    pub model: String,
}

impl GrokClient {
    pub fn from_env() -> Self {
        let api_key = std::env::var("XAI_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty());
        let model = std::env::var("GROK_MODEL").unwrap_or_else(|_| "grok-4.6".to_string());
        Self { api_key, model }
    }

    pub fn status(&self) -> &'static str {
        if self.api_key.is_some() {
            "configured"
        } else {
            "not_configured"
        }
    }

    /// Ask Grok for a narrative read on a snapshot summary. Returns None when
    /// unconfigured or when the call fails — callers must treat None as
    /// abstention, not as a neutral opinion.
    pub async fn analyze(&self, context: &str) -> Option<String> {
        let key = self.api_key.as_ref()?;
        let body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": "You are Grok advising a Solana perps+spot fund. Be concise, cite the numbers given, and never recommend exceeding the stated risk limits."},
                {"role": "user", "content": context}
            ]
        });
        let v: Value = reqwest::Client::new()
            .post("https://api.x.ai/v1/chat/completions")
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        v.get("choices")?
            .get(0)?
            .get("message")?
            .get("content")?
            .as_str()
            .map(|s| s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abstains_without_key() {
        // Tests run without XAI_API_KEY in env; the client must abstain.
        if std::env::var("XAI_API_KEY").is_ok() {
            return; // a configured dev shell is allowed to differ
        }
        let grok = GrokClient::from_env();
        assert_eq!(grok.status(), "not_configured");
        let out = futures::executor::block_on(grok.analyze("SOL at $100"));
        assert!(out.is_none());
    }
}
