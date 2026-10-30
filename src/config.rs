use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

/// Used when no config file exists. Keys are never stored here, only env var names.
const DEFAULT: &str = r#"
provider = "openrouter"

[providers.openrouter]
kind = "openai_compat"
base_url = "https://openrouter.ai/api/v1"
key_env = "OPENROUTER_API_KEY"
model = "deepseek/deepseek-v4.1-flash-1017"
only = ["deepseek"]
"#;

#[derive(Deserialize)]
pub struct Config {
    pub provider: String,
    pub providers: HashMap<String, ProviderConfig>,
    /// Programs the `run` tool may start, by bare name.
    #[serde(default = "default_run_allow")]
    pub run_allow: Vec<String>,
}

fn default_run_allow() -> Vec<String> {
    ["cargo", "rustc", "rustfmt", "npm", "pnpm", "yarn", "bun", "node", "deno", "tsc", "python3", "pytest", "go", "make"]
        .map(String::from)
        .to_vec()
}

#[derive(Deserialize)]
pub struct ProviderConfig {
    pub kind: String,
    pub base_url: String,
    pub key_env: String,
    pub model: String,
    /// OpenRouter: restrict routing to these upstream providers, no fallback.
    #[serde(default)]
    pub only: Vec<String>,
}

impl Config {
    pub fn active(&self) -> Result<&ProviderConfig> {
        self.providers
            .get(&self.provider)
            .with_context(|| format!("provider \"{}\" is not in the config", self.provider))
    }
}

pub fn load() -> Result<Config> {
    let path = path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT.to_string(),
        Err(e) => return Err(e).with_context(|| path.display().to_string()),
    };
    toml::from_str(&text).with_context(|| path.display().to_string())
}

fn path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("ai").join("config.toml")
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_parses() {
        let cfg: super::Config = toml::from_str(super::DEFAULT).unwrap();
        assert_eq!(cfg.active().unwrap().only, ["deepseek"]);
    }
}
