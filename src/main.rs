mod agent;
mod command;
mod config;
mod exec;
mod message;
mod permission;
mod provider;
mod repl;
mod sandbox;
mod tools;
mod ui;

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc;

use crate::agent::Agent;
use crate::permission::Mode;
use crate::provider::OpenAiCompat;
use crate::sandbox::Workspace;
use crate::tools::Ctx;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cfg = config::load()?;
    let p = cfg.active()?;
    if p.kind != "openai_compat" {
        bail!("type de provider inconnu : {}", p.kind);
    }
    let key = std::env::var(&p.key_env).with_context(|| format!("variable {} absente", p.key_env))?;
    let provider = OpenAiCompat::new(&p.base_url, key, p.model.clone(), p.only.clone());

    let ws = Workspace::open(std::env::current_dir()?)?;
    let header = format!("{} · {}", p.model, ws.root.display());
    let mut ctx = Ctx::new(ws, cfg.run_allow.clone());
    let (tx, mut events) = mpsc::unbounded_channel();
    let mut agent = Agent::new(provider, tools::all(), tx, Mode::Ask);
    repl::run(&mut agent, &mut ctx, &mut events, &header).await
}
