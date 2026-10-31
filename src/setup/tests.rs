use super::*;

fn ask(s: &mut Setup, input: &str) -> Next {
    s.answer(input).unwrap_or_else(|e| panic!("{input:?} refused: {e}"))
}

fn err(status: u16) -> Result<(), CheckError> {
    Err(CheckError { status, message: format!("{status}: nope") })
}

/// First run: provider, key, model, a failing check that sends the user back
/// to the key, then a passing one that saves.
#[test]
fn first_run_saves_only_after_a_passing_check() {
    let dir = crate::sandbox::tests::temp("setup-first");
    // A name no environment sets: only the saved key counts.
    const NAME: &str = "AI_TEST_FIRST_RUN_KEY";
    let mut cfg = Config::default();
    cfg.providers.get_mut("openrouter").unwrap().key_env = NAME.into();
    let mut s = Setup::first_run(dir.clone(), cfg);
    assert!(s.prompt().contains("1 OpenRouter") && !s.closable());
    assert!(matches!(ask(&mut s, "1"), Next::Ask));
    assert!(s.secret() && s.prompt().contains("https://openrouter.ai/keys"));
    assert!(s.answer("").unwrap_err().contains("key is required"));
    assert!(s.answer("short").is_err());
    assert!(matches!(ask(&mut s, "sk-or-v1-0000000000000000bad"), Next::Ask));
    assert!(!s.secret() && s.prompt().contains("Enter keeps deepseek/"));
    assert!(matches!(ask(&mut s, ""), Next::Check));

    let msg = s.checked(err(401)).unwrap_err();
    assert!(msg.contains("401") && msg.contains("check the key"), "{msg}");
    assert!(!dir.join("config.toml").exists() && keys::get(&dir, NAME).is_none(), "saved before a passing check");
    assert!(s.secret() && s.prompt().contains("sk-or-…0bad"));
    ask(&mut s, "sk-or-v1-0000000000000000good");
    ask(&mut s, "");
    match s.checked(Ok(())).unwrap() {
        Next::Saved { msg, done } => assert!(done && msg.contains("deepseek/"), "{msg}"),
        _ => panic!("not saved"),
    }
    assert_eq!(keys::get(&dir, NAME).unwrap().value, "sk-or-v1-0000000000000000good");
    let saved = config::load(&dir).unwrap();
    assert!(ready(&dir, &saved).is_ok());
    assert!(!std::fs::read_to_string(dir.join("config.toml")).unwrap().contains("good"), "key in config.toml");
}

/// /config: a model change is checked before saving, a wrong model goes
/// back to the model step, and programs refuse shells.
#[test]
fn menu_changes_one_thing_at_a_time() {
    let dir = crate::sandbox::tests::temp("setup-menu");
    keys::save(&dir, "OPENROUTER_API_KEY", "sk-or-v1-0000000000000000abcd").unwrap();
    let mut s = Setup::menu(dir.clone(), Config::default());
    assert!(s.closable() && s.prompt().contains("2 key       sk-or-…abcd"));
    assert!(s.answer("9").is_err());
    ask(&mut s, "3");
    assert!(matches!(ask(&mut s, "anthropic/claude-x"), Next::Check));
    assert!(s.config().active().unwrap().only.is_empty(), "DeepSeek-only routing kept for another vendor");
    assert!(s.checked(err(404)).unwrap_err().contains("check the model"));
    assert!(matches!(ask(&mut s, "deepseek/deepseek-chat"), Next::Check));
    assert_eq!(s.config().active().unwrap().only, ["deepseek"]);
    assert!(matches!(s.checked(Ok(())).unwrap(), Next::Saved { done: false, .. }));

    ask(&mut s, "4");
    assert!(s.answer("cargo bash").unwrap_err().contains("shells"));
    assert!(s.answer("cargo ../evil").is_err());
    assert!(matches!(ask(&mut s, "cargo, python  cargo"), Next::Saved { .. }));
    assert_eq!(config::load(&dir).unwrap().run_allow, ["cargo", "python"]);
    assert!(matches!(ask(&mut s, ""), Next::Close));
}

/// A local server needs a URL but no key; a remote one must use HTTPS.
#[test]
fn custom_provider_needs_a_safe_url() {
    let dir = crate::sandbox::tests::temp("setup-custom");
    let mut s = Setup::first_run(dir.clone(), Config::default());
    ask(&mut s, "4");
    assert!(s.prompt().starts_with("Base URL"));
    assert!(s.answer("http://api.example.com/v1").unwrap_err().contains("https://"));
    ask(&mut s, "http://localhost:11434/v1/");
    assert!(s.prompt().contains("Enter for none"));
    ask(&mut s, "");
    assert!(s.answer("").is_err(), "no default model for a custom API");
    ask(&mut s, "qwen3");
    assert!(s.checked(err(0)).unwrap_err().contains("check the URL"));
    ask(&mut s, "");
    ask(&mut s, "");
    ask(&mut s, "");
    assert!(matches!(s.checked(Ok(())).unwrap(), Next::Saved { done: true, .. }));
    let cfg = config::load(&dir).unwrap();
    assert_eq!(cfg.active().unwrap().base_url, "http://localhost:11434/v1");
    assert!(ready(&dir, &cfg).is_ok(), "a local server runs without a key");
}
