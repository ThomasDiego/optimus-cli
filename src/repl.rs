use std::io::{self, BufRead, Write};

use anyhow::Result;
use tokio::sync::mpsc;

use crate::agent::Agent;
use crate::command::{self, Action};
use crate::message::{kilo, money};
use crate::provider::Provider;
use crate::tools::{Ctx, truncate};
use crate::ui::{Approval, Event, brief, stat, summary};

const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const RESET: &str = "\x1b[0m";

/// Line-based UI, used when stdin or stdout is not a terminal.
pub async fn run<P: Provider>(
    agent: &mut Agent<P>,
    ctx: &mut Ctx,
    events: &mut mpsc::UnboundedReceiver<Event>,
    header: &str,
) -> Result<()> {
    println!("{DIM}{header}{RESET}");
    let mut lines = stdin_lines();
    loop {
        print!("\n> ");
        io::stdout().flush()?;
        let Some(line) = lines.recv().await else { break };
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        match command::handle(input, agent, ctx) {
            Some(Action::Exit) => break,
            Some(Action::Clear) => {
                println!("{DIM}conversation cleared{RESET}");
                continue;
            }
            Some(Action::Say(s)) => {
                println!("{DIM}{s}{RESET}");
                continue;
            }
            None => {}
        }
        let result = {
            let turn = agent.turn(input.to_string(), ctx);
            tokio::pin!(turn);
            loop {
                tokio::select! {
                    biased;
                    Some(ev) = events.recv() => show(ev, &mut lines).await,
                    r = &mut turn => break r,
                }
            }
        };
        while let Ok(ev) = events.try_recv() {
            show(ev, &mut lines).await;
        }
        match result {
            Ok(u) => println!("\n{DIM}↑{} ↓{} cache {} {}{RESET}", kilo(u.input), kilo(u.output), kilo(u.cached), money(u.cost)),
            Err(e) => eprintln!("\n{RED}{e:#}{RESET}"),
        }
    }
    Ok(())
}

async fn show(ev: Event, lines: &mut mpsc::UnboundedReceiver<String>) {
    match ev {
        Event::Text(t) => {
            print!("{t}");
            let _ = io::stdout().flush();
        }
        Event::Usage(_) | Event::Drafting { .. } => {}
        Event::ToolCall(call) => println!("\n{DIM}› {}{RESET}", brief(&call)),
        Event::ToolResult { ok, output } => {
            let (first, failed) = summary(ok, &output);
            println!("{}  {}{RESET}", if failed { RED } else { DIM }, truncate(&first, 100));
        }
        Event::Diff { diff, .. } => {
            let (add, del) = stat(&diff);
            println!("  {GREEN}+{add}{RESET} {RED}-{del}{RESET}");
        }
        Event::Approve { req, reply } => {
            match &req {
                Approval::Write { diff, .. } => print_diff(diff),
                Approval::Exec { cmd, .. } => println!("run {cmd}"),
                Approval::Fetch { url } => println!("fetch {url}"),
            }
            print!("Allow? [y/N] ");
            let _ = io::stdout().flush();
            let yes = lines.recv().await.is_some_and(|l| matches!(l.trim(), "y" | "Y"));
            let _ = reply.send(yes);
        }
    }
}

fn print_diff(diff: &str) {
    for line in diff.lines() {
        let color = match line.as_bytes().first() {
            Some(b'+') => GREEN,
            Some(b'-') => RED,
            Some(b'@') => DIM,
            _ => "",
        };
        println!("{color}{line}{RESET}");
    }
}

/// Reads stdin on a blocking thread so the event loop stays responsive.
fn stdin_lines() -> mpsc::UnboundedReceiver<String> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}
