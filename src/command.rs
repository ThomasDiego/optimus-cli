use crate::agent::Agent;
use crate::permission::Mode;
use crate::provider::Provider;
use crate::tools::{Ctx, change};

pub enum Action {
    Exit,
    Say(String),
}

const COMMANDS: &[(&str, &str)] = &[
    ("undo", "annuler les dernières modifications"),
    ("mode", "ask · edit · auto"),
    ("clear", "vider la conversation"),
    ("exit", "quitter"),
];

const MODES: &[(&str, &str)] = &[
    ("ask", "tout confirmer"),
    ("edit", "éditions automatiques, commandes confirmées"),
    ("auto", "rien à confirmer"),
];

pub struct Suggestion {
    /// Full input once accepted, e.g. `/mode auto`.
    pub text: String,
    pub desc: &'static str,
    /// Accepting it should leave room for an argument.
    pub takes_arg: bool,
}

/// Completions for a `/command` being typed: prefix matches first, then
/// fuzzy (in-order letters) matches.
pub fn suggest(input: &str) -> Vec<Suggestion> {
    let Some(rest) = input.strip_prefix('/') else { return vec![] };
    if rest.contains('\n') {
        return vec![];
    }
    match rest.split_once(' ') {
        None => ranked(COMMANDS, rest)
            .map(|(name, desc)| Suggestion { text: format!("/{name}"), desc, takes_arg: name == "mode" })
            .collect(),
        Some(("mode", arg)) if !arg.contains(' ') => ranked(MODES, arg)
            .map(|(name, desc)| Suggestion { text: format!("/mode {name}"), desc, takes_arg: false })
            .collect(),
        _ => vec![],
    }
}

fn ranked<'a>(list: &'a [(&'static str, &'static str)], typed: &'a str) -> impl Iterator<Item = (&'static str, &'static str)> + 'a {
    let prefix = list.iter().filter(move |(n, _)| n.starts_with(typed));
    let fuzzy = list.iter().filter(move |(n, _)| !n.starts_with(typed) && subsequence(typed, n));
    prefix.chain(fuzzy).copied()
}

fn subsequence(needle: &str, hay: &str) -> bool {
    let mut hay = hay.chars();
    needle.chars().all(|c| hay.any(|h| h == c))
}

/// Handles `/commands`; `None` means the input is a prompt for the model.
pub fn handle<P: Provider>(input: &str, agent: &mut Agent<P>, ctx: &mut Ctx) -> Option<Action> {
    let rest = input.strip_prefix('/')?;
    let mut words = rest.split_whitespace();
    let name = words.next().unwrap_or("");
    // A leading absolute path ("/Users/…") is a prompt, not a command.
    if name.contains('/') {
        return None;
    }
    Some(match name {
        "exit" | "quit" => Action::Exit,
        "undo" => Action::Say(undo(agent, ctx)),
        "mode" => {
            let mode = match words.next() {
                None => agent.mode.get().next(),
                Some(m) => match Mode::parse(m) {
                    Some(m) => m,
                    None => return Some(Action::Say("modes : ask, edit, auto".into())),
                },
            };
            agent.mode.set(mode);
            Action::Say(format!("mode {}", mode.name()))
        }
        "clear" => {
            agent.clear();
            ctx.reads.clear();
            Action::Say("conversation vidée".into())
        }
        _ => Action::Say("/undo  /mode [ask|edit|auto]  /clear  /exit".into()),
    })
}

fn undo<P: Provider>(agent: &mut Agent<P>, ctx: &mut Ctx) -> String {
    match change::undo(ctx) {
        Ok(files) if files.is_empty() => "rien à annuler".into(),
        Ok(files) => {
            let list = files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
            agent.note(format!("[The user reverted your last changes to: {list}. Re-read before editing.]"));
            format!("annulé : {list}")
        }
        Err(e) => e,
    }
}

#[cfg(test)]
mod tests {
    use super::suggest;

    fn names(input: &str) -> Vec<String> {
        suggest(input).into_iter().map(|s| s.text).collect()
    }

    #[test]
    fn suggests_commands_and_mode_args() {
        assert_eq!(names("/"), ["/undo", "/mode", "/clear", "/exit"]);
        assert_eq!(names("/cl"), ["/clear"]);
        assert_eq!(names("/e"), ["/exit", "/mode", "/clear"]);
        assert_eq!(names("/mode a"), ["/mode ask", "/mode auto"]);
        assert_eq!(names("/mode ed"), ["/mode edit"]);
        assert!(names("/undo now").is_empty());
        assert!(names("hello").is_empty());
    }
}
