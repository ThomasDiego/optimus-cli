use std::io::{self, Write};

use anyhow::Result;
use tokio::sync::mpsc;

use crate::agent::Agent;
use crate::provider::Provider;
use crate::tools::{Ctx, truncate};
use crate::ui::{Approval, Event, brief};

/// One turn without UI: the answer goes to stdout (pipeable), progress to
/// stderr. Nobody can approve, so anything the mode would ask for is denied.
pub async fn run<P: Provider>(
    agent: &mut Agent<P>,
    ctx: &mut Ctx,
    events: &mut mpsc::UnboundedReceiver<Event>,
    prompt: String,
) -> Result<()> {
    let result = {
        let turn = agent.turn(prompt, ctx);
        tokio::pin!(turn);
        loop {
            tokio::select! {
                biased;
                Some(ev) = events.recv() => show(ev),
                r = &mut turn => break r,
            }
        }
    };
    while let Ok(ev) = events.try_recv() {
        show(ev);
    }
    println!();
    let u = result?;
    eprintln!("↑{} ↓{} cache {} ${:.4}", u.input, u.output, u.cached, u.cost);
    Ok(())
}

fn show(ev: Event) {
    match ev {
        Event::Text(t) => {
            print!("{t}");
            let _ = io::stdout().flush();
        }
        Event::Usage(_) => {}
        Event::ToolCall(call) => eprintln!("› {}", brief(&call)),
        Event::ToolResult { ok, output } => {
            let first = truncate(output.lines().next().unwrap_or(""), 120).replace('\t', "  ");
            eprintln!("  {}{first}", if ok { "" } else { "✗ " });
        }
        Event::Diff { path, .. } => eprintln!("  modifié : {}", path.display()),
        Event::Approve { req, reply } => {
            let _ = reply.send(false);
            let what = match req {
                Approval::Write { path, .. } => format!("écriture de {}", path.display()),
                Approval::Exec { cmd } => format!("exécution de {cmd}"),
            };
            eprintln!("  ✗ {what} refusée (--mode edit ou auto pour l'autoriser)");
        }
    }
}
