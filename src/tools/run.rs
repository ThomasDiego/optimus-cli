use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use super::{Ctx, Outcome, Tool, parse};
use crate::exec::Exec;
use crate::message::ToolDef;

const DEFAULT_TIMEOUT: u64 = 120;
const MAX_TIMEOUT: u64 = 600;

/// Subcommands that publish, authenticate, install globally or fetch and run
/// remote code: refused even though the program itself is allowed.
const DENIED_SUBCOMMANDS: &[(&str, &[&str])] = &[
    ("cargo", &["publish", "login", "logout", "owner", "yank", "install", "uninstall"]),
    ("npm", &["publish", "unpublish", "login", "logout", "adduser", "token", "owner", "access", "exec", "x"]),
    ("pnpm", &["publish", "login", "logout", "exec", "dlx"]),
    ("yarn", &["publish", "login", "logout", "npm", "dlx"]),
    ("bun", &["publish", "x"]),
    ("deno", &["publish", "install", "eval"]),
    ("go", &["install"]),
];

/// Inline code bypasses the reviewable write_file step.
const INLINE_CODE_FLAGS: &[(&str, &[&str])] = &[
    ("python3", &["-c"]),
    ("node", &["-e", "--eval", "-p", "--print"]),
    ("bun", &["-e", "--eval"]),
];

/// Shell habits and the tool that does the job: a direct pointer saves the
/// model from guessing (or scripting around the refusal).
const SHELL_TOOLS: &[(&str, &str)] = &[
    ("ls tree", "list_dir"),
    ("cat head tail less", "read_file"),
    ("grep rg ag", "grep"),
    ("wc", "grep with mode=count, or read_file (its footer gives the line count)"),
    ("find fd", "glob"),
    ("rm rmdir", "delete_file"),
    ("mv", "move_file"),
    ("cp touch mkdir echo tee sed awk", "write_file or edit_file (parent dirs are created)"),
    ("curl wget", "web_fetch (reads pages only; nothing can be sent)"),
    ("git", "git"),
    ("kill pkill ps", "process"),
];

pub struct Run;

#[derive(Deserialize)]
struct Args {
    cmd: String,
    #[serde(default)]
    args: Vec<String>,
    dir: Option<String>,
    timeout: Option<u64>,
    #[serde(default)]
    net: bool,
    #[serde(default)]
    background: bool,
}

fn denied(cmd: &str, args: &[String]) -> Option<String> {
    let lookup = |table: &[(&str, &'static [&'static str])]| table.iter().find(|(p, _)| *p == cmd).map(|(_, l)| *l).unwrap_or(&[]);
    let sub = args.iter().find(|a| !a.starts_with('-') && !a.starts_with('+'));
    if let Some(sub) = sub.filter(|s| lookup(DENIED_SUBCOMMANDS).contains(&s.as_str())) {
        return Some(format!("{cmd} {sub} is not allowed (publishing, credentials or remote code)"));
    }
    let inline = lookup(INLINE_CODE_FLAGS);
    if args.iter().any(|a| inline.iter().any(|f| a == f || (f.len() == 2 && a.starts_with(f)))) {
        return Some("inline code is not allowed".into());
    }
    None
}

impl Tool for Run {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "run",
            description: "Run an allowed program (cargo, npm, pytest…). No shell: no pipes, globs or &&. Offline unless net; cargo then uses its local cache, try that first. Returns exit code, output and changed files.",
            schema: json!({
                "type": "object",
                "properties": {
                    "cmd": {"type": "string", "description": "program name only"},
                    "args": {"type": "array", "items": {"type": "string"}},
                    "dir": {"type": "string", "description": "working dir, default root"},
                    "timeout": {"type": "integer", "description": "seconds, default 120"},
                    "net": {"type": "boolean", "description": "network access (downloads); user must approve"},
                    "background": {"type": "boolean", "description": "for dev servers, watchers and long jobs (minutes): returns the job id and early output (a server printing its URL is up, no need to probe it); follow with process output+wait"}
                },
                "required": ["cmd"]
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        if a.cmd.contains(char::is_whitespace) {
            return Err("cmd is the program name only; put arguments in args".into());
        }
        if let Some((_, tool)) = SHELL_TOOLS.iter().find(|(cmd, _)| cmd.split(' ').any(|c| c == a.cmd)) {
            return Err(format!("no shell commands; use {tool}"));
        }
        if a.cmd.contains('/') {
            return Err("programs run by name only; run build outputs through their tool (cargo run, npm run…)".into());
        }
        if !ctx.run_allow.contains(&a.cmd) {
            return Err(format!("{} not allowed; allowed: {}", a.cmd, ctx.run_allow.join(", ")));
        }
        if let Some(reason) = denied(&a.cmd, &a.args) {
            return Err(reason);
        }
        let dir = ctx.ws.walk_base(a.dir.as_deref())?;
        let timeout = Duration::from_secs(a.timeout.unwrap_or(DEFAULT_TIMEOUT).clamp(1, MAX_TIMEOUT));
        // Without network, cargo resolves from its local cache at once instead
        // of retrying the blocked registry for seconds.
        let env: &'static [_] = if a.net { &[] } else { &[("CARGO_NET_OFFLINE", "true")] };
        Ok(Outcome::Exec(Exec { net: a.net, env, background: a.background, ..Exec::new(a.cmd, a.args, dir, timeout) }))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::tools::tests::{call, ctx};

    #[test]
    fn validates_program_and_dir() {
        let mut c = ctx("run", &[]);
        assert_eq!(call(&mut c, "run", json!({"cmd": "cargo", "args": ["test"]})).unwrap(), r#"exec cargo ["test"]"#);
        assert!(call(&mut c, "run", json!({"cmd": "cargo test"})).unwrap_err().contains("put arguments in args"));
        assert!(call(&mut c, "run", json!({"cmd": "bash"})).unwrap_err().contains("not allowed; allowed: cargo"));
        assert_eq!(call(&mut c, "run", json!({"cmd": "ls"})).unwrap_err(), "no shell commands; use list_dir");
        assert!(call(&mut c, "run", json!({"cmd": "curl"})).unwrap_err().contains("nothing can be sent"));
        assert!(call(&mut c, "run", json!({"cmd": "/usr/bin/cargo"})).unwrap_err().contains("by name only"));
        assert!(call(&mut c, "run", json!({"cmd": "./target/debug/demo"})).unwrap_err().contains("cargo run"));
        assert!(call(&mut c, "run", json!({"cmd": "cargo", "dir": ".."})).is_err());
    }

    #[test]
    fn cargo_is_offline_without_net() {
        let mut c = ctx("run-offline", &[]);
        let tool = crate::tools::all().into_iter().find(|t| t.def().name == "run").unwrap();
        let env = |c: &mut crate::tools::Ctx, net: bool| match tool.run(c, &json!({"cmd": "cargo", "net": net}).to_string()).unwrap() {
            crate::tools::Outcome::Exec(ex) => ex.env,
            _ => unreachable!(),
        };
        assert_eq!(env(&mut c, false), [("CARGO_NET_OFFLINE", "true")]);
        assert!(env(&mut c, true).is_empty());
    }

    #[test]
    fn refuses_dangerous_subcommands_and_inline_code() {
        let mut c = ctx("run-deny", &[]);
        c.run_allow = ["cargo", "npm", "python3", "node"].map(String::from).to_vec();
        let run = |c: &mut crate::tools::Ctx, cmd: &str, args: &[&str]| call(c, "run", json!({"cmd": cmd, "args": args}));
        assert!(run(&mut c, "cargo", &["publish"]).unwrap_err().contains("cargo publish is not allowed"));
        assert!(run(&mut c, "cargo", &["+nightly", "install", "x"]).is_err());
        assert!(run(&mut c, "npm", &["exec", "evil"]).is_err());
        assert!(run(&mut c, "python3", &["-c", "print(1)"]).unwrap_err().contains("inline code"));
        assert!(run(&mut c, "python3", &["-cprint(1)"]).is_err());
        assert!(run(&mut c, "node", &["--eval", "1"]).is_err());
        assert!(run(&mut c, "cargo", &["test", "--", "--nocapture"]).is_ok());
        assert!(run(&mut c, "python3", &["script.py", "-c", "conf"]).is_err(), "conservative on -c anywhere");
        assert!(run(&mut c, "npm", &["run", "build"]).is_ok());
    }
}
