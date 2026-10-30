use std::io::Read;

use crate::permission::Mode;

const USAGE: &str = "usage : ai [-p <prompt>|-] [--mode ask|edit|auto]

  sans -p   interface interactive
  -p        un tour sans interface ; réponse sur stdout, progression sur stderr
            (- lit le prompt sur stdin)
  --mode    permissions ; en -p, ask = lecture seule (rien ne peut être confirmé)";

pub struct Args {
    /// Headless prompt; `None` starts the interactive UI.
    pub prompt: Option<String>,
    pub mode: Mode,
}

pub fn parse() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut out = Args { prompt: None, mode: Mode::Ask };
    while let Some(a) = args.next() {
        match a.as_str() {
            "-p" | "--print" => {
                let p = args.next().ok_or_else(|| format!("-p attend un prompt\n\n{USAGE}"))?;
                out.prompt = Some(if p == "-" { stdin()? } else { p });
            }
            "--mode" => {
                out.mode = args
                    .next()
                    .and_then(|m| Mode::parse(&m))
                    .ok_or_else(|| format!("--mode attend ask, edit ou auto\n\n{USAGE}"))?;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("argument inconnu : {other}\n\n{USAGE}")),
        }
    }
    if out.prompt.as_deref().is_some_and(|p| p.trim().is_empty()) {
        return Err("prompt vide".into());
    }
    Ok(out)
}

fn stdin() -> Result<String, String> {
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).map_err(|e| format!("stdin : {e}"))?;
    Ok(s)
}
