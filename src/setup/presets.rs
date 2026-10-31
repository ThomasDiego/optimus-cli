//! Known providers, and turning a config into a ready one.

use std::path::Path;

use crate::config::{Config, ProviderConfig};
use crate::keys;
use crate::provider::OpenAiCompat;

pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub base_url: &'static str,
    pub key_env: &'static str,
    pub model: &'static str,
    pub keys_url: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "openrouter",
        label: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        key_env: "OPENROUTER_API_KEY",
        model: "deepseek/deepseek-v4.1-flash-1017",
        keys_url: "https://openrouter.ai/keys",
    },
    Preset {
        id: "openai",
        label: "OpenAI",
        base_url: "https://api.openai.com/v1",
        key_env: "OPENAI_API_KEY",
        model: "",
        keys_url: "https://platform.openai.com/api-keys",
    },
    Preset {
        id: "deepseek",
        label: "DeepSeek",
        base_url: "https://api.deepseek.com",
        key_env: "DEEPSEEK_API_KEY",
        model: "deepseek-chat",
        keys_url: "https://platform.deepseek.com/api_keys",
    },
    Preset { id: "custom", label: "Other OpenAI-compatible API (Ollama, Mistral…)", base_url: "", key_env: "AI_API_KEY", model: "", keys_url: "" },
];

impl Preset {
    pub fn config(&self) -> ProviderConfig {
        ProviderConfig {
            kind: "openai_compat".into(),
            base_url: self.base_url.into(),
            key_env: self.key_env.into(),
            model: self.model.into(),
            only: default_only(self.id, self.model),
        }
    }
}

/// A ready provider, or why the config cannot run yet.
pub fn ready(dir: &Path, cfg: &Config) -> Result<OpenAiCompat, String> {
    cfg.validate()?;
    let p = cfg.active().map_err(|e| e.to_string())?;
    let key = match keys::get(dir, &p.key_env) {
        Some(k) => k.value,
        None if local(p) => String::new(),
        None => return Err(format!("no API key for {} yet", cfg.provider)),
    };
    Ok(OpenAiCompat::new(&p.base_url, key, p.model.clone(), p.only.clone()))
}

/// Local servers (Ollama, LM Studio) usually need no key.
pub fn local(p: &ProviderConfig) -> bool {
    p.base_url.starts_with("http://")
}

/// OpenRouter keeps DeepSeek models on DeepSeek itself, without fallback.
pub fn default_only(provider: &str, model: &str) -> Vec<String> {
    if provider == "openrouter" && model.starts_with("deepseek/") { vec!["deepseek".into()] } else { vec![] }
}
