use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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
const FILE: &str = "config.toml";
/// They would bring back the shell `run` refuses to be.
const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "dash", "ksh", "csh", "tcsh", "nu", "cmd", "powershell", "pwsh", "env", "sudo", "doas", "xargs",
];

#[derive(Clone, Deserialize, Serialize)]
pub struct Config {
    pub provider: String,
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Programs the `run` tool may start, by bare name.
    #[serde(default = "default_run_allow")]
    pub run_allow: Vec<String>,
}

fn default_run_allow() -> Vec<String> {
    ["cargo", "rustc", "rustfmt", "npm", "pnpm", "yarn", "bun", "node", "deno", "tsc", "python3", "pytest", "go", "make", "php", "composer", "laravel"]
        .map(String::from)
        .to_vec()
}

#[derive(Clone, Deserialize, Serialize)]
pub struct ProviderConfig {
    pub kind: String,
    pub base_url: String,
    pub key_env: String,
    pub model: String,
    /// OpenRouter: restrict routing to these upstream providers, no fallback.
    #[serde(default)]
    pub only: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        toml::from_str(DEFAULT).expect("default config parses")
    }
}

impl Config {
    pub fn active(&self) -> Result<&ProviderConfig> {
        self.providers
            .get(&self.provider)
            .with_context(|| format!("provider \"{}\" is not in the config", self.provider))
    }

    /// Refuses what would break the security model or fail on the first request.
    pub fn validate(&self) -> Result<(), String> {
        let p = self.active().map_err(|e| e.to_string())?;
        if p.kind != "openai_compat" {
            return Err(format!("unknown provider kind \"{}\"", p.kind));
        }
        check_url(&p.base_url)?;
        let env_name = |c: char| c.is_ascii_alphanumeric() || c == '_';
        if p.key_env.is_empty() || p.key_env.starts_with(|c: char| c.is_ascii_digit()) || !p.key_env.chars().all(env_name) {
            return Err(format!("key_env {:?}: not an environment variable name", p.key_env));
        }
        if p.model.is_empty() || p.model.contains(char::is_whitespace) {
            return Err("model: missing or contains spaces".into());
        }
        Ok(())
    }

    /// Drops `run_allow` entries that are not plain program names; says which.
    pub fn sanitize(&mut self) -> Option<String> {
        let mut dropped = vec![];
        self.run_allow.retain(|name| check_program(name).map_err(|e| dropped.push(e)).is_ok());
        (!dropped.is_empty()).then(|| format!("run_allow: ignored {}", dropped.join("; ")))
    }
}

/// HTTPS only, except a local server: the key never travels in clear.
pub fn check_url(url: &str) -> Result<(), String> {
    let local = ["http://localhost", "http://127.0.0.1", "http://[::1]"]
        .iter()
        .any(|p| url.strip_prefix(p).is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/'])));
    let https = url.strip_prefix("https://").is_some_and(|rest| !rest.is_empty());
    if (https || local) && !url.contains(char::is_whitespace) {
        Ok(())
    } else {
        Err(format!("{url:?}: use https:// (http:// only for localhost), so the key never travels in clear"))
    }
}

/// A bare program name, and never a shell.
pub fn check_program(name: &str) -> Result<(), String> {
    let plain = |c: char| c.is_ascii_alphanumeric() || "-_.+".contains(c);
    if name.is_empty() || name.starts_with('.') || !name.chars().all(plain) {
        return Err(format!("{name}: not a program name"));
    }
    let lower = name.to_ascii_lowercase();
    if SHELLS.contains(&lower.strip_suffix(".exe").unwrap_or(&lower)) {
        return Err(format!("{name}: shells are never allowed"));
    }
    Ok(())
}

/// `cargo, npm  node` as a list without duplicates, every name checked.
pub fn parse_programs(input: &str) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = vec![];
    for name in input.split([' ', ',']).filter(|n| !n.is_empty()) {
        check_program(name)?;
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

/// Where config.toml and the key file live.
pub fn dir() -> PathBuf {
    crate::os::config_dir().join("ai")
}

/// The defaults when there is no file yet.
pub fn load(dir: &Path) -> Result<Config, String> {
    let path = dir.join(FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {}", path.display(), e.message())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn save(dir: &Path, cfg: &Config) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(FILE);
    // A file that no longer parses is set aside rather than lost.
    if std::fs::read_to_string(&path).is_ok_and(|t| toml::from_str::<Config>(&t).is_err()) {
        std::fs::copy(&path, dir.join("config.toml.bak"))?;
    }
    let text = format!("# Managed by ai: change it with /config.\n{}", toml::to_string(cfg).map_err(io::Error::other)?);
    write_file(&path, text.as_bytes(), false)
}

/// Written next to `path`, then renamed over it: never half a file.
/// `private` files are readable by their owner only.
pub fn write_file(path: &Path, data: &[u8], private: bool) -> io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = opts.open(&tmp)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_parses_and_validates() {
        let cfg = Config::default();
        assert_eq!(cfg.active().unwrap().only, ["deepseek"]);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn refuses_clear_text_urls_bad_names_and_shells() {
        for ok in ["https://openrouter.ai/api/v1", "http://localhost:11434/v1", "http://127.0.0.1", "http://[::1]:8080/v1"] {
            assert!(check_url(ok).is_ok(), "{ok}");
        }
        for bad in ["http://api.example.com/v1", "http://localhost.evil.com", "https://", "ftp://x", "openrouter.ai"] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
        let mut cfg = Config::default();
        cfg.providers.get_mut("openrouter").unwrap().key_env = "MY KEY".into();
        assert!(cfg.validate().unwrap_err().contains("key_env"));
        cfg.provider = "nope".into();
        assert!(cfg.validate().is_err());

        let mut cfg = Config { run_allow: ["cargo", "bash", "CMD.EXE", "../x", "python3.12"].map(String::from).to_vec(), ..Config::default() };
        let note = cfg.sanitize().unwrap();
        assert_eq!(cfg.run_allow, ["cargo", "python3.12"]);
        assert!(note.contains("bash: shells") && note.contains("CMD.EXE") && note.contains("../x"), "{note}");
    }

    #[test]
    fn saves_round_trip_and_keeps_a_broken_file() {
        let dir = crate::sandbox::tests::temp("config-save");
        std::fs::write(dir.join(FILE), "provider = [").unwrap();
        assert!(load(&dir).is_err());
        let mut cfg = Config::default();
        cfg.run_allow = vec!["cargo".into()];
        save(&dir, &cfg).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("config.toml.bak")).unwrap(), "provider = [");
        let back = load(&dir).unwrap();
        assert_eq!(back.run_allow, cfg.run_allow);
        assert_eq!(back.active().unwrap().model, cfg.active().unwrap().model);
    }
}
