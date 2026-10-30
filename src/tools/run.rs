use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use super::{Ctx, Outcome, Tool, parse};
use crate::exec::Exec;
use crate::message::ToolDef;

const DEFAULT_TIMEOUT: u64 = 120;
const MAX_TIMEOUT: u64 = 600;

pub struct Run;

#[derive(Deserialize)]
struct Args {
    cmd: String,
    #[serde(default)]
    args: Vec<String>,
    dir: Option<String>,
    timeout: Option<u64>,
}

impl Tool for Run {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "run",
            description: "Run an allowed program (cargo, npm, pytest…). No shell: no pipes, globs or &&. Returns exit code and output.",
            schema: json!({
                "type": "object",
                "properties": {
                    "cmd": {"type": "string", "description": "program name only"},
                    "args": {"type": "array", "items": {"type": "string"}},
                    "dir": {"type": "string", "description": "working dir, default root"},
                    "timeout": {"type": "integer", "description": "seconds, default 120"}
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
        if a.cmd.contains('/') || !ctx.run_allow.contains(&a.cmd) {
            return Err(format!("{} not allowed; allowed: {}", a.cmd, ctx.run_allow.join(", ")));
        }
        let dir = ctx.ws.walk_base(a.dir.as_deref())?;
        let timeout = Duration::from_secs(a.timeout.unwrap_or(DEFAULT_TIMEOUT).clamp(1, MAX_TIMEOUT));
        Ok(Outcome::Exec(Exec { program: a.cmd, args: a.args, dir, timeout }))
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
        assert!(call(&mut c, "run", json!({"cmd": "/usr/bin/cargo"})).is_err());
        assert!(call(&mut c, "run", json!({"cmd": "cargo", "dir": ".."})).is_err());
    }
}
