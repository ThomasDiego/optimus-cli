//! API keys typed in /config, kept out of config.toml in `<config dir>/ai/.env`:
//! readable by the user only, and named like the secret files every tool and
//! sandbox already refuses. The environment variable is the fallback.

use std::io;
use std::path::Path;

const FILE: &str = ".env";

pub struct Key {
    pub value: String,
    pub from_env: bool,
}

/// The saved key wins: it is the one the user last set in /config.
pub fn get(dir: &Path, name: &str) -> Option<Key> {
    let saved = read(dir).into_iter().find(|(n, _)| n == name).map(|(_, value)| Key { value, from_env: false });
    saved.or_else(|| {
        let value = std::env::var(name).ok()?.trim().to_string();
        (!value.is_empty()).then_some(Key { value, from_env: true })
    })
}

pub fn save(dir: &Path, name: &str, value: &str) -> io::Result<()> {
    let mut all = read(dir);
    all.retain(|(n, _)| n != name);
    all.push((name.to_string(), value.to_string()));
    let text: String = all.iter().map(|(n, v)| format!("{n}={v}\n")).collect();
    std::fs::create_dir_all(dir)?;
    crate::config::write_file(&dir.join(FILE), text.as_bytes(), true)
}

fn read(dir: &Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(dir.join(FILE)) else { return vec![] };
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
        .filter(|(n, v)| !n.is_empty() && !v.is_empty())
        .collect()
}

/// `sk-or-…f3a9`: enough to recognise a key, useless to anyone else.
pub fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < 16 {
        return "•".repeat(8);
    }
    let (head, tail): (String, String) = (chars[..6].iter().collect(), chars[chars.len() - 4..].iter().collect());
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_privately_and_masks() {
        let dir = crate::sandbox::tests::temp("keys");
        save(&dir, "A_KEY", "sk-or-v1-0123456789abcdef").unwrap();
        save(&dir, "B_KEY", "other-key-value").unwrap();
        save(&dir, "A_KEY", "sk-or-v1-replaced-0000ffff").unwrap();
        assert_eq!(get(&dir, "A_KEY").unwrap().value, "sk-or-v1-replaced-0000ffff");
        assert_eq!(get(&dir, "B_KEY").unwrap().value, "other-key-value");
        assert!(get(&dir, "AI_TEST_UNSET_KEY_NAME").is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(FILE)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // The file tools refuse it like any project secret.
        assert!(crate::sandbox::is_secret(&dir.join(FILE)));
        assert_eq!(mask("sk-or-v1-replaced-0000ffff"), "sk-or-…ffff");
        assert_eq!(mask("short"), "••••••••");
    }
}
