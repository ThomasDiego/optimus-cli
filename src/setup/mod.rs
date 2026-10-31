//! Guided configuration: the first run, then /config. A UI shows `prompt()`,
//! passes each answer to `answer()` and runs `check()` when asked: nothing is
//! saved before a real request proved the key and the model work together.

mod presets;

use std::path::PathBuf;

pub use self::presets::ready;
use self::presets::{PRESETS, Preset, default_only, local};
use crate::config::{self, Config, ProviderConfig};
use crate::keys;
use crate::provider::{CheckError, OpenAiCompat};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Step {
    Menu,
    Provider,
    BaseUrl,
    Key,
    Model,
    Programs,
}

#[derive(Debug)]
pub enum Next {
    /// Show `prompt()` and wait for the answer.
    Ask,
    /// Run `check()`, then hand its result to `checked()`.
    Check,
    /// Saved: the caller reloads the provider from `config()`.
    Saved { msg: String, done: bool },
    /// Left without changes.
    Close,
}

pub struct Setup {
    dir: PathBuf,
    cfg: Config,
    /// Typed but not saved yet: saved only once the check passed.
    key: Option<String>,
    step: Step,
    first_run: bool,
    /// Provider, key and model in a row, as opposed to a single menu entry.
    full: bool,
}

impl Setup {
    pub fn first_run(dir: PathBuf, cfg: Config) -> Self {
        Self::new(dir, cfg, Step::Provider, true)
    }

    pub fn menu(dir: PathBuf, cfg: Config) -> Self {
        Self::new(dir, cfg, Step::Menu, false)
    }

    fn new(dir: PathBuf, mut cfg: Config, step: Step, first_run: bool) -> Self {
        if cfg.active().is_err() {
            let default = Config::default();
            cfg.provider = default.provider.clone();
            cfg.providers.extend(default.providers);
        }
        Self { dir, cfg, key: None, step, first_run, full: first_run }
    }

    /// Only /config can be left: a first run has nothing to fall back on.
    pub fn closable(&self) -> bool {
        !self.first_run
    }

    pub fn secret(&self) -> bool {
        self.step == Step::Key
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    fn active(&self) -> &ProviderConfig {
        &self.cfg.providers[&self.cfg.provider]
    }

    fn preset(&self) -> Option<&'static Preset> {
        PRESETS.iter().find(|p| p.id == self.cfg.provider)
    }

    fn current_key(&self) -> Option<String> {
        self.key.clone().or_else(|| keys::get(&self.dir, &self.active().key_env).map(|k| k.value))
    }

    pub fn prompt(&self) -> String {
        let p = self.active();
        let keep = |v: &str| if v.is_empty() { String::new() } else { format!(" (Enter keeps {v})") };
        match self.step {
            Step::Menu => {
                let key = match keys::get(&self.dir, &p.key_env) {
                    Some(k) if k.from_env => format!("{} from ${}", keys::mask(&k.value), p.key_env),
                    Some(k) => keys::mask(&k.value),
                    None => "none".into(),
                };
                let provider = self.preset().map_or(self.cfg.provider.as_str(), |p| p.label);
                let programs = self.cfg.run_allow.join(" ");
                format!("Configuration\n  1 provider  {provider}\n  2 key       {key}\n  3 model     {}\n  4 programs  {programs}\nChoose 1-4, or Enter to close.", p.model)
            }
            Step::Provider => {
                let list: String = PRESETS.iter().enumerate().map(|(i, p)| format!("\n  {} {}", i + 1, p.label)).collect();
                let current = self.preset().map_or(self.cfg.provider.as_str(), |p| p.label);
                format!("Provider:{list}\nChoose 1-{}{}", PRESETS.len(), keep(current))
            }
            Step::BaseUrl => format!("Base URL, e.g. http://localhost:11434/v1 for Ollama{}", keep(&p.base_url)),
            Step::Key => {
                let from = self.preset().filter(|p| !p.keys_url.is_empty()).map_or(String::new(), |p| format!(", from {}", p.keys_url));
                let hint = match self.current_key() {
                    Some(k) => keep(&keys::mask(&k)),
                    None if local(p) => " (Enter for none)".into(),
                    None => String::new(),
                };
                format!("API key{from}{hint}")
            }
            Step::Model => {
                let see = if self.cfg.provider == "openrouter" { ", see https://openrouter.ai/models" } else { "" };
                format!("Model{see}{}", keep(&p.model))
            }
            Step::Programs => format!("Programs `run` may start, separated by spaces{}", keep(&self.cfg.run_allow.join(" "))),
        }
    }

    pub fn answer(&mut self, input: &str) -> Result<Next, String> {
        let input = input.trim();
        match self.step {
            Step::Menu => {
                self.full = input == "1";
                self.step = match input {
                    "" => return Ok(Next::Close),
                    "1" => Step::Provider,
                    "2" => Step::Key,
                    "3" => Step::Model,
                    "4" => Step::Programs,
                    _ => return Err("choose 1-4, or press Enter to close".into()),
                };
                Ok(Next::Ask)
            }
            Step::Provider => {
                let chosen = match input {
                    "" => None,
                    n => Some(n.parse::<usize>().ok().and_then(|i| PRESETS.get(i.wrapping_sub(1))).ok_or(format!("choose 1-{}", PRESETS.len()))?),
                };
                if let Some(preset) = chosen.filter(|p| p.id != self.cfg.provider) {
                    self.cfg.provider = preset.id.into();
                    self.key = None;
                    self.cfg.providers.entry(preset.id.into()).or_insert_with(|| preset.config());
                }
                // Custom providers, and any provider unknown here, say where they live.
                let known = self.preset().is_some_and(|p| p.id != "custom");
                self.step = if known && !self.active().base_url.is_empty() { Step::Key } else { Step::BaseUrl };
                Ok(Next::Ask)
            }
            Step::BaseUrl => {
                let url = if input.is_empty() { self.active().base_url.clone() } else { input.trim_end_matches('/').to_string() };
                config::check_url(&url)?;
                self.provider_mut().base_url = url;
                self.step = Step::Key;
                Ok(Next::Ask)
            }
            Step::Key => {
                if !input.is_empty() {
                    if input.len() < 8 || input.contains(char::is_whitespace) {
                        return Err("that does not look like an API key".into());
                    }
                    self.key = Some(input.to_string());
                } else if self.current_key().is_none() && !local(self.active()) {
                    return Err("a key is required for this provider".into());
                }
                if self.full {
                    self.step = Step::Model;
                    return Ok(Next::Ask);
                }
                Ok(Next::Check)
            }
            Step::Model => {
                let model = if input.is_empty() { self.active().model.clone() } else { input.to_string() };
                if model.is_empty() || model.contains(char::is_whitespace) {
                    return Err("type a model name, without spaces".into());
                }
                if model != self.active().model {
                    let only = default_only(&self.cfg.provider, &model);
                    let p = self.provider_mut();
                    (p.model, p.only) = (model, only);
                }
                Ok(Next::Check)
            }
            Step::Programs => {
                if !input.is_empty() {
                    self.cfg.run_allow = config::parse_programs(input)?;
                }
                self.save("Programs saved.".into())
            }
        }
    }

    fn provider_mut(&mut self) -> &mut ProviderConfig {
        self.cfg.providers.get_mut(&self.cfg.provider).expect("active provider exists")
    }

    /// The provider for the saved config.
    pub fn provider(&self) -> Result<OpenAiCompat, String> {
        ready(&self.dir, &self.cfg)
    }

    pub async fn check(&self) -> Result<(), CheckError> {
        let p = self.active();
        OpenAiCompat::new(&p.base_url, self.current_key().unwrap_or_default(), p.model.clone(), p.only.clone()).check().await
    }

    /// Saves after a passing check; otherwise goes back to the likely culprit.
    pub fn checked(&mut self, result: Result<(), CheckError>) -> Result<Next, String> {
        match result {
            Ok(()) => {
                if let Some(key) = self.key.take() {
                    keys::save(&self.dir, &self.active().key_env, &key).map_err(|e| format!("could not save the key: {e}"))?;
                }
                self.save(format!("Saved: {} answers with this key.", self.active().model))
            }
            Err(e) => {
                self.step = match e.status {
                    401 | 403 => Step::Key,
                    0 if self.preset().is_none_or(|p| p.id == "custom") => Step::BaseUrl,
                    0 => Step::Key,
                    _ => Step::Model,
                };
                let hint = match self.step {
                    Step::Key => "check the key",
                    Step::BaseUrl => "check the URL and your connection",
                    _ => "check the model name",
                };
                Err(format!("{}; {hint}", e.message))
            }
        }
    }

    fn save(&mut self, msg: String) -> Result<Next, String> {
        config::save(&self.dir, &self.cfg).map_err(|e| format!("could not save the config: {e}"))?;
        self.step = Step::Menu;
        Ok(Next::Saved { msg, done: self.first_run })
    }
}

#[cfg(test)]
mod tests;
