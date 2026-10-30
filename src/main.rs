mod agent;
mod config;
mod message;
mod provider;
mod repl;
mod sandbox;
mod tools;
mod ui;

use anyhow::{Context, Result, bail};

use crate::agent::Agent;
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
    let mut ctx = Ctx::new(ws);
    let mut agent = Agent::new(provider, tools::all());
    repl::run(&mut agent, &mut ctx, &header).await
}
