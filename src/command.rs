use crate::agent::Agent;
use crate::permission::Mode;
use crate::provider::Provider;
use crate::tools::{Ctx, change};

pub enum Action {
    Exit,
    Say(String),
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
