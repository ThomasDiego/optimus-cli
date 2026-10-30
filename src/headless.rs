use std::io::{self, Write};

use anyhow::Result;
use tokio::sync::mpsc;

use crate::agent::Agent;
use crate::message::{kilo, money};
use crate::provider::Provider;
use crate::tools::{Ctx, truncate};
use crate::ui::{Approval, Event, brief, stat, summary};

/// One turn without UI: the answer goes to stdout (pipeable), progress to
/// stderr. Nobody can approve, so anything the mode would ask for is denied.
pub async fn run<P: Provider>(
    agent: &mut Agent<P>,
    ctx: &mut Ctx,
    events: &mut mpsc::UnboundedReceiver<Event>,
    prompt: String,
) -> Result<()> {
    let mut flow = Flow::Start;
    let result = {
        let turn = agent.turn(prompt, ctx);
        tokio::pin!(turn);
        loop {
            tokio::select! {
                biased;
                Some(ev) = events.recv() => show(ev, &mut flow),
                r = &mut turn => break r,
            }
        }
    };
    while let Ok(ev) = events.try_recv() {
        show(ev, &mut flow);
    }
    println!();
    let u = result?;
    eprintln!("↑{} ↓{} cache {} {}", kilo(u.input), kilo(u.output), kilo(u.cached), money(u.cost));
    Ok(())
}

/// Where stdout stands, so texts from separate steps do not run together.
#[derive(PartialEq)]
enum Flow {
    Start,
    Text,
    Gap,
}

fn show(ev: Event, flow: &mut Flow) {
    match ev {
        Event::Text(t) => {
            let sep = if *flow == Flow::Gap { "\n\n" } else { "" };
            print!("{sep}{t}");
            let _ = io::stdout().flush();
            *flow = Flow::Text;
        }
        Event::Usage(_) | Event::Drafting { .. } => {}
        Event::ToolCall(call) => {
            if *flow == Flow::Text {
                *flow = Flow::Gap;
            }
            eprintln!("› {}", brief(&call));
        }
        Event::ToolResult { ok, output } => {
            // ✗ marks tool errors only (the eval counts them); exit codes stay plain.
            let (first, _) = summary(ok, &output);
            eprintln!("  {}{}", if ok { "" } else { "✗ " }, truncate(&first, 120));
        }
        Event::Diff { diff, .. } => {
            // Moves and deletions have no line diff: their result line says it all.
            let (add, del) = stat(&diff);
            if (add, del) != (0, 0) {
                eprintln!("  +{add} -{del}");
            }
        }
        Event::Approve { req, reply } => {
            let _ = reply.send(false);
            let (what, hint) = match req {
                Approval::Write { label, .. } => (format!("write {label}"), "needs --mode edit or auto"),
                Approval::Exec { cmd, net: false } => (format!("run {cmd}"), "needs --mode auto"),
                Approval::Exec { cmd, net: true } => (format!("run {cmd}"), "network needs interactive approval"),
                Approval::Fetch { url } => (format!("fetch {url}"), "network needs interactive approval"),
            };
            eprintln!("  ✗ denied: {what} ({hint})");
        }
    }
}
