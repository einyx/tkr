use super::cosine::normalize;

pub enum Backend {
    Ollama(OllamaBackend),
    ExactMatch,
}

pub struct OllamaBackend {
    url: String,
    model: String,
}

impl Backend {
    pub fn new(ollama_url: &str, ollama_model: &str) -> Self {
        if Self::ollama_available(ollama_url) {
            Backend::Ollama(OllamaBackend { url: ollama_url.to_string(), model: ollama_model.to_string() })
        } else {
            Backend::ExactMatch
        }
    }

    fn ollama_available(url: &str) -> bool {
        ureq::get(&format!("{url}/api/tags"))
            .timeout(std::time::Duration::from_millis(300))
            .call().is_ok()
    }

    pub fn embed(&self, text: &str) -> Option<Vec<f32>> {
        match self {
            Backend::Ollama(b) => b.embed(text),
            Backend::ExactMatch => None,
        }
    }
}

impl OllamaBackend {
    fn embed(&self, text: &str) -> Option<Vec<f32>> {
        let body = serde_json::json!({ "model": self.model, "prompt": text });
        let resp = ureq::post(&format!("{}/api/embeddings", self.url))
            .timeout(std::time::Duration::from_secs(5))
            .send_json(body).ok()?;
        let json: serde_json::Value = resp.into_json().ok()?;
        let arr = json["embedding"].as_array()?;
        let mut v: Vec<f32> = arr.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect();
        normalize(&mut v);
        Some(v)
    }
}
