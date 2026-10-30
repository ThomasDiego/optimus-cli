use std::io::{self, BufRead, Write};
use std::path::Path;

use anyhow::Result;

use crate::agent::Agent;
use crate::message::ToolCall;
use crate::provider::Provider;
use crate::tools::{Ctx, truncate};
use crate::ui::Ui;

const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const RESET: &str = "\x1b[0m";

/// Plain line-based terminal UI; replaced by the ratatui view later.
struct Term;

impl Ui for Term {
    fn text(&mut self, chunk: &str) {
        print!("{chunk}");
        let _ = io::stdout().flush();
    }

    fn tool_call(&mut self, call: &ToolCall) {
        println!("\n{DIM}› {} {}{RESET}", call.name, truncate(&call.args, 100));
    }

    fn tool_result(&mut self, ok: bool, output: &str) {
        let first = truncate(output.lines().next().unwrap_or(""), 100);
        let color = if ok { DIM } else { RED };
        println!("{color}  {first}{RESET}");
    }

    fn approve(&mut self, path: &Path, diff: &str) -> bool {
        println!("{}", path.display());
        for line in diff.lines() {
            let color = match line.as_bytes().first() {
                Some(b'+') => GREEN,
                Some(b'-') => RED,
                Some(b'@') => DIM,
                _ => "",
            };
            println!("{color}{line}{RESET}");
        }
        print!("Appliquer ? [o/N] ");
        let _ = io::stdout().flush();
        read_line().is_some_and(|l| matches!(l.trim(), "o" | "O" | "y" | "Y"))
    }
}

fn read_line() -> Option<String> {
    let mut line = String::new();
    match io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

pub async fn run<P: Provider>(agent: &mut Agent<P>, ctx: &mut Ctx, header: &str) -> Result<()> {
    println!("{DIM}{header}{RESET}");
    let mut ui = Term;
    loop {
        print!("\n> ");
        io::stdout().flush()?;
        let Some(line) = read_line() else { break };
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        if input == "/exit" {
            break;
        }
        match agent.turn(input.to_string(), ctx, &mut ui).await {
            Ok(u) => println!("\n{DIM}↑{} ↓{} cache {} ${:.4}{RESET}", u.input, u.output, u.cached, u.cost),
            Err(e) => eprintln!("\n{RED}{e:#}{RESET}"),
        }
    }
    Ok(())
}
