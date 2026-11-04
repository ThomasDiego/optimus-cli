mod app;
mod clipboard;
mod input;
mod markdown;
mod select;
mod setup;
mod transcript;
mod view;

use std::io::{self, Write, stdout};
use std::time::Duration;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event as TermEvent, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use self::app::{Action, App};
use crate::agent::Agent;
use crate::command::{self, Action as Cmd};
use crate::config::{self, Config};
use crate::provider::OpenAiCompat;
use crate::setup::Setup;
use crate::tools::Ctx;
use crate::ui::Event;

/// ~30 fps, only while a turn runs; idle means zero wakeups.
const FRAME: Duration = Duration::from_millis(33);

/// What the session starts from.
pub struct Start {
    pub cfg: Config,
    /// Why the config cannot run yet: the setup opens first.
    pub problem: Option<String>,
    pub notice: Option<String>,
}
/// Mouse buttons, wheel and motion while a button is held (SGR encoding):
/// unlike full capture, a pointer that is only moving produces no events.
/// Dragging selects text, since capturing the mouse disables the terminal's own.
const MOUSE_ON: &str = "\x1b[?1002h\x1b[?1006h";
const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1002l";

pub enum Error {
    /// The terminal could not be set up; nothing was drawn yet.
    Init(io::Error),
    Session(anyhow::Error),
}

pub async fn run(
    agent: &mut Agent<OpenAiCompat>,
    ctx: &mut Ctx,
    events: &mut mpsc::UnboundedReceiver<Event>,
    start: Start,
) -> Result<(), Error> {
    // Alternate screen: the UI owns the whole terminal, like 100dvh × 100%.
    let mut term = match ratatui::try_init() {
        Ok(t) => t,
        Err(e) => {
            let _ = ratatui::try_restore();
            return Err(Error::Init(e));
        }
    };
    // Paste arrives as one event instead of keystrokes (no accidental submit);
    // the enhancement flags let terminals report Shift+Enter.
    let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
    let _ = execute!(stdout(), EnableBracketedPaste, PushKeyboardEnhancementFlags(flags));
    let _ = write!(stdout(), "{MOUSE_ON}").and_then(|_| stdout().flush());
    let result = session(&mut term, agent, ctx, events, start).await;
    let _ = write!(stdout(), "{MOUSE_OFF}");
    let _ = execute!(stdout(), PopKeyboardEnhancementFlags, DisableBracketedPaste);
    ratatui::try_restore().map_err(|e| Error::Session(e.into()))?;
    result.map_err(Error::Session)
}

async fn session(
    term: &mut DefaultTerminal,
    agent: &mut Agent<OpenAiCompat>,
    ctx: &mut Ctx,
    events: &mut mpsc::UnboundedReceiver<Event>,
    start: Start,
) -> Result<()> {
    let Start { mut cfg, problem, notice } = start;
    let model = cfg.active().map(|p| p.model.clone()).unwrap_or_default();
    let mut app = App::new(model, agent.mode.clone());
    let mut keys = term_events();
    if let Some(n) = notice {
        app.say(&n, true);
    }
    if let Some(why) = problem {
        app.say("Welcome to optimus. Let's connect a model provider; /config changes it later.", false);
        app.say(&why, false);
        setup::open(&mut app, Setup::first_run(config::dir(), cfg.clone()));
    }
    loop {
        app.jobs = ctx.jobs.running();
        draw(term, &mut app)?;
        // While a Ctrl+C is armed, wake when it expires to clear its hint.
        let ev = match app.quit_until.filter(|_| app.quit_armed()) {
            Some(until) => tokio::select! {
                ev = keys.recv() => ev,
                _ = tokio::time::sleep_until(until.into()) => continue,
            },
            None => keys.recv().await,
        };
        let Some(ev) = ev else { return Ok(()) };
        let input = match app.on_term(ev) {
            Action::Submit(s) => s,
            Action::Quit => return Ok(()),
            Action::Cancel if app.setup.as_ref().is_some_and(Setup::closable) => {
                app.setup = None;
                app.say("config unchanged", false);
                continue;
            }
            Action::None | Action::Cancel => continue,
        };
        if app.setup.is_some() {
            if !input.is_empty() {
                // An API key never appears on screen.
                app.echo(if app.secret() { "••••••••" } else { &input });
            }
            if setup::feed(term, &mut app, agent, ctx, &mut keys, &mut cfg, &input).await? {
                return Ok(());
            }
            continue;
        }
        app.echo(&input);
        match command::handle(&input, agent, ctx) {
            Some(Cmd::Config) => {
                setup::open(&mut app, Setup::menu(config::dir(), cfg.clone()));
                continue;
            }
            Some(Cmd::Exit) => return Ok(()),
            Some(Cmd::Clear) => {
                app.clear();
                continue;
            }
            Some(Cmd::Say(s)) => {
                app.say(&s, false);
                app.end_turn();
                continue;
            }
            None => {}
        }

        app.start_turn();
        let mut quit = false;
        let outcome = {
            let turn = agent.turn(input, ctx);
            tokio::pin!(turn);
            let mut frame = tokio::time::interval(FRAME);
            frame.set_missed_tick_behavior(MissedTickBehavior::Skip);
            // Redraw only when something visible changed: new content, or the
            // animation reaching its next step.
            let (mut dirty, mut shown) = (true, usize::MAX);
            loop {
                tokio::select! {
                    biased;
                    Some(ev) = keys.recv() => match app.on_term(ev) {
                        Action::Cancel => break None,
                        Action::Quit => {
                            quit = true;
                            break None;
                        }
                        Action::None | Action::Submit(_) => draw(term, &mut app)?,
                    },
                    _ = frame.tick() => {
                        let step = view::anim_step(&app);
                        if app.reveal() || dirty || step != shown {
                            draw(term, &mut app)?;
                            (dirty, shown) = (false, step);
                        }
                    }
                    Some(ev) = events.recv() => {
                        app.on_agent(ev);
                        dirty = true;
                    }
                    r = &mut turn => break Some(r),
                }
            }
        };
        // The turn future is dropped here: a cancelled command is killed.
        while let Ok(ev) = events.try_recv() {
            app.on_agent(ev);
        }
        match outcome {
            Some(Ok(_)) => {}
            Some(Err(e)) => app.say(&format!("{e:#}"), true),
            None => {
                agent.heal();
                app.say("interrupted", true);
            }
        }
        app.end_turn();
        if quit {
            draw(term, &mut app)?;
            return Ok(());
        }
    }
}

/// Ratatui diffs against the previous frame: only changed cells are written.
pub(super) fn draw(term: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    term.draw(|f| view::render(f, app))?;
    if let Some(text) = app.copied.take().filter(|t| !t.is_empty()) {
        clipboard::copy(&text);
    }
    Ok(())
}

/// Blocking reads on a dedicated thread: no polling, no CPU while idle.
fn term_events() -> mpsc::UnboundedReceiver<TermEvent> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if tx.send(ev).is_err() {
                break;
            }
        }
    });
    rx
}
