//! Runs a `Setup` in the full-screen UI: answers come from the input box,
//! the check shows as a turn that Esc cancels.

use std::io;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::Event as TermEvent;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use super::app::{Action, App};
use super::{FRAME, draw};
use crate::agent::Agent;
use crate::config::Config;
use crate::provider::{CheckError, OpenAiCompat};
use crate::setup::{Next, Setup};
use crate::tools::Ctx;

enum Checked {
    Done(Result<(), CheckError>),
    Cancelled,
    Quit,
}

/// Opens the setup and asks its first question.
pub fn open(app: &mut App, setup: Setup) {
    app.ask(&setup.prompt());
    app.setup = Some(setup);
}

/// Hands one answer to the setup; true when the user quit meanwhile.
pub async fn feed(
    term: &mut DefaultTerminal,
    app: &mut App,
    agent: &mut Agent<OpenAiCompat>,
    ctx: &mut Ctx,
    keys: &mut mpsc::UnboundedReceiver<TermEvent>,
    cfg: &mut Config,
    input: &str,
) -> io::Result<bool> {
    let Some(mut setup) = app.setup.take() else { return Ok(false) };
    let mut next = setup.answer(input);
    loop {
        match next {
            Ok(Next::Check) => {
                next = match check(term, app, keys, &setup).await? {
                    Checked::Done(r) => setup.checked(r),
                    Checked::Cancelled => {
                        app.say("check cancelled", true);
                        Ok(Next::Ask)
                    }
                    Checked::Quit => return Ok(true),
                }
            }
            Ok(Next::Ask) => break,
            Err(msg) => {
                app.say(&msg, true);
                break;
            }
            Ok(Next::Close) => {
                app.say("config unchanged", false);
                return Ok(false);
            }
            Ok(Next::Saved { msg, done }) => {
                *cfg = setup.config().clone();
                match setup.provider() {
                    Ok(p) => agent.set_provider(p),
                    Err(e) => app.say(&e, true),
                }
                ctx.run_allow = cfg.run_allow.clone();
                app.model = cfg.active().map(|p| p.model.clone()).unwrap_or_default();
                app.say(&msg, false);
                if done {
                    return Ok(false);
                }
                break;
            }
        }
    }
    open(app, setup);
    Ok(false)
}

/// The test request, animated like a turn.
async fn check(term: &mut DefaultTerminal, app: &mut App, keys: &mut mpsc::UnboundedReceiver<TermEvent>, setup: &Setup) -> io::Result<Checked> {
    app.start_turn();
    app.activity = format!("Checking {} with this key", setup.config().active().map(|p| p.model.as_str()).unwrap_or(""));
    let request = setup.check();
    tokio::pin!(request);
    let mut frame = tokio::time::interval(FRAME);
    frame.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let out = loop {
        tokio::select! {
            biased;
            Some(ev) = keys.recv() => match app.on_term(ev) {
                Action::Cancel => break Checked::Cancelled,
                Action::Quit => break Checked::Quit,
                Action::None | Action::Submit(_) => {}
            },
            _ = frame.tick() => draw(term, app)?,
            r = &mut request => break Checked::Done(r),
        }
    };
    app.end_turn();
    Ok(out)
}
