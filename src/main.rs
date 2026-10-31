mod agent;
mod cli;
mod command;
mod config;
mod exec;
mod headless;
mod jobs;
mod local;
mod message;
mod os;
mod permission;
mod provider;
mod repl;
mod sandbox;
mod tools;
mod track;
mod tui;
mod ui;
mod web;

use std::io::{self, IsTerminal};

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc;

use crate::agent::Agent;
use crate::provider::OpenAiCompat;
use crate::sandbox::Workspace;
use crate::tools::Ctx;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = cli::parse().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2);
    });
    let cfg = config::load()?;
    let p = cfg.active()?;
    if p.kind != "openai_compat" {
        bail!("unknown provider kind: {}", p.kind);
    }
    let key = std::env::var(&p.key_env).with_context(|| format!("{} is not set", p.key_env))?;
    let provider = OpenAiCompat::new(&p.base_url, key, p.model.clone(), p.only.clone());

    let ws = Workspace::open(std::env::current_dir()?)?;
    let header = format!("{} · {}", p.model, ws.root.display());
    let mut ctx = Ctx::new(ws, cfg.run_allow.clone());
    let (tx, mut events) = mpsc::unbounded_channel();
    let mut agent = Agent::new(provider, tools::all(), tx, args.mode);
    if let Some(prompt) = args.prompt {
        return headless::run(&mut agent, &mut ctx, &mut events, prompt).await;
    }
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        match tui::run(&mut agent, &mut ctx, &mut events, p.model.clone()).await {
            // Terminals that cannot report the cursor get the line UI instead.
            Err(tui::Error::Init(e)) => eprintln!("TUI unavailable ({e}), using line mode"),
            Err(tui::Error::Session(e)) => return Err(e),
            Ok(()) => return Ok(()),
        }
    }
    repl::run(&mut agent, &mut ctx, &mut events, &header).await
}
