mod agent;
mod cli;
mod command;
mod config;
mod exec;
mod headless;
mod jobs;
mod keys;
mod local;
mod message;
mod os;
mod permission;
mod provider;
mod repl;
mod sandbox;
mod setup;
mod tools;
mod track;
mod tui;
mod ui;
mod web;

use std::io::{self, IsTerminal};

use anyhow::{Result, anyhow};
use tokio::sync::mpsc;

use crate::agent::Agent;
use crate::config::Config;
use crate::provider::OpenAiCompat;
use crate::sandbox::Workspace;
use crate::tools::Ctx;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = cli::parse().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2);
    });
    let dir = config::dir();
    let (mut cfg, problem) = match config::load(&dir) {
        Ok(cfg) => (cfg, None),
        Err(e) => (Config::default(), Some(e)),
    };
    let notice = cfg.sanitize();
    let model = cfg.active().map(|p| p.model.clone()).unwrap_or_default();
    // Without a working config, the full-screen UI runs the setup before any
    // request; the other UIs cannot ask, so they stop with the reason.
    let (provider, problem) = match problem.map_or_else(|| setup::ready(&dir, &cfg), Err) {
        Ok(p) => (p, None),
        Err(e) => (OpenAiCompat::new("https://unconfigured.invalid", String::new(), String::new(), vec![]), Some(e)),
    };
    let unusable = |e: &str| anyhow!("{e}: run `ai` in a terminal to set it up");

    let ws = Workspace::open(std::env::current_dir()?)?;
    let header = format!("{model} · {}", ws.root.display());
    let mut ctx = Ctx::new(ws, cfg.run_allow.clone());
    let (tx, mut events) = mpsc::unbounded_channel();
    let mut agent = Agent::new(provider, tools::all(), tx, args.mode);
    if let Some(prompt) = args.prompt {
        if let Some(e) = &problem {
            return Err(unusable(e));
        }
        if let Some(n) = &notice {
            eprintln!("{n}");
        }
        return headless::run(&mut agent, &mut ctx, &mut events, prompt).await;
    }
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        let start = tui::Start { cfg, problem: problem.clone(), notice: notice.clone() };
        match tui::run(&mut agent, &mut ctx, &mut events, start).await {
            // Terminals that cannot report the cursor get the line UI instead.
            Err(tui::Error::Init(e)) => eprintln!("TUI unavailable ({e}), using line mode"),
            Err(tui::Error::Session(e)) => return Err(e),
            Ok(()) => return Ok(()),
        }
    }
    if let Some(e) = &problem {
        return Err(unusable(e));
    }
    repl::run(&mut agent, &mut ctx, &mut events, &header).await
}
