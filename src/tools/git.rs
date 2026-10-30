use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use super::{Ctx, Outcome, Tool, parse};
use crate::exec::{Exec, Review};
use crate::message::ToolDef;

const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_LOG: u32 = 50;

/// Never configurable by the model: no hooks, no external programs, no transport.
const HARDENING: &[&str] = &[
    "-c", "core.hooksPath=/dev/null",
    "-c", "core.fsmonitor=false",
    "-c", "core.pager=cat",
    "-c", "credential.helper=",
    "-c", "protocol.allow=never",
    "-c", "diff.external=",
    "-c", "color.ui=never",
    "--no-pager",
];
const ENV: &[(&str, &str)] = &[("GIT_TERMINAL_PROMPT", "0"), ("GIT_OPTIONAL_LOCKS", "0")];
/// `add all` never stages secrets, even when they are not ignored.
const SECRET_EXCLUDES: &[&str] =
    &[":(exclude,glob)**/.env", ":(exclude,glob)**/.env.*", ":(exclude,glob)**/*.pem", ":(exclude,glob)**/*.key"];

pub struct Git;

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Op {
    Status,
    Diff,
    Log,
    Show,
    Add,
    Commit,
    Branch,
    Switch,
    Restore,
}

#[derive(Deserialize)]
struct Args {
    op: Op,
    #[serde(default)]
    paths: Vec<String>,
    rev: Option<String>,
    message: Option<String>,
    name: Option<String>,
    n: Option<u32>,
    #[serde(default)]
    staged: bool,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    stat: bool,
    #[serde(default)]
    create: bool,
}

impl Tool for Git {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "git",
            description: "Local git, no network. diff gives stat + patch. commit takes only what add staged. restore discards working changes.",
            schema: json!({
                "type": "object",
                "properties": {
                    "op": {"type": "string", "enum": ["status", "diff", "log", "show", "add", "commit", "branch", "switch", "restore"]},
                    "paths": {"type": "array", "items": {"type": "string"}},
                    "rev": {"type": "string", "description": "show: commit, default HEAD"},
                    "message": {"type": "string", "description": "commit message"},
                    "name": {"type": "string", "description": "branch name; branch without name lists"},
                    "n": {"type": "integer", "description": "log entries, default 10"},
                    "staged": {"type": "boolean", "description": "diff/restore the index"},
                    "all": {"type": "boolean", "description": "add every change (secrets excluded)"},
                    "stat": {"type": "boolean", "description": "diff: stat only, for large changes"},
                    "create": {"type": "boolean", "description": "switch: create the branch"}
                },
                "required": ["op"]
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        // Pathspecs go to git as text, so `..` is refused here rather than at I/O.
        let paths = a
            .paths
            .iter()
            .map(|p| match ctx.ws.rel_write(p)? {
                r if r.components().any(|c| matches!(c, std::path::Component::ParentDir)) => Err(format!("{p}: .. not allowed")),
                r => Ok(r.display().to_string()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut argv: Vec<String> = vec![];
        let mut push = |s: &str| argv.push(s.to_string());
        let review = match a.op {
            Op::Status => {
                ["status", "--short", "--branch"].iter().for_each(|s| push(s));
                Review::None
            }
            Op::Diff => {
                ["diff", "--no-ext-diff", "--no-textconv"].iter().for_each(|s| push(s));
                if a.staged {
                    push("--cached");
                }
                // Stat first: it survives output clipping when the patch is long.
                push(if a.stat { "--stat" } else { "--patch-with-stat" });
                Review::None
            }
            Op::Log => {
                let n = a.n.unwrap_or(10).clamp(1, MAX_LOG).to_string();
                ["log", "--oneline", "-n", &n].iter().for_each(|s| push(s));
                Review::None
            }
            Op::Show => {
                let rev = safe_name(a.rev.as_deref().unwrap_or("HEAD"))?;
                ["show", "--no-ext-diff", "--no-textconv"].iter().for_each(|s| push(s));
                if paths.is_empty() {
                    push("--stat");
                }
                push(rev);
                Review::None
            }
            Op::Add if a.all => {
                ["add", "-A", "--", "."].iter().chain(SECRET_EXCLUDES).for_each(|s| push(s));
                Review::Write
            }
            Op::Add if paths.is_empty() => return Err("add needs paths or all=true".into()),
            Op::Add => {
                push("add");
                Review::Write
            }
            Op::Commit => {
                let msg = a.message.as_deref().map(str::trim).filter(|m| !m.is_empty()).ok_or("commit needs a message")?;
                ["commit", "-m", msg].iter().for_each(|s| push(s));
                Review::Write
            }
            Op::Branch => match &a.name {
                None => {
                    ["branch", "--list"].iter().for_each(|s| push(s));
                    Review::None
                }
                Some(name) => {
                    ["branch", safe_name(name)?].iter().for_each(|s| push(s));
                    Review::Write
                }
            },
            Op::Switch => {
                let name = safe_name(a.name.as_deref().ok_or("switch needs a name")?)?;
                push("switch");
                if a.create {
                    push("-c");
                }
                push(name);
                Review::Write
            }
            Op::Restore if paths.is_empty() => return Err("restore needs paths".into()),
            Op::Restore => {
                push("restore");
                if a.staged {
                    push("--staged");
                }
                Review::Exec
            }
        };
        let takes_paths = matches!(a.op, Op::Diff | Op::Log | Op::Show | Op::Add | Op::Restore) && !(matches!(a.op, Op::Add) && a.all);
        if takes_paths && !paths.is_empty() {
            argv.push("--".into());
            argv.extend(paths);
        }
        let exec = Exec::new("git".into(), argv, ctx.ws.root.clone(), TIMEOUT);
        Ok(Outcome::Exec(Exec { review, git: review != Review::None, env: ENV, hidden_args: HARDENING, ..exec }))
    }
}

/// Branch or revision: no option injection, no odd characters.
fn safe_name(s: &str) -> Result<&str, String> {
    let ok = !s.is_empty()
        && s.len() <= 200
        && !s.starts_with('-')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "._/@{}~^-".contains(c));
    if ok { Ok(s) } else { Err(format!("invalid name or revision: {s}")) }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::tools::tests::ctx;
    use crate::tools::{Outcome, all};

    fn plan(c: &mut crate::tools::Ctx, args: serde_json::Value) -> Result<(String, bool), String> {
        let tool = all().into_iter().find(|t| t.def().name == "git").unwrap();
        match tool.run(c, &args.to_string())? {
            Outcome::Exec(ex) => Ok((ex.args.join(" "), ex.git)),
            _ => unreachable!(),
        }
    }

    #[test]
    fn builds_fixed_commands_and_rejects_injection() {
        let mut c = ctx("git", &[("a.rs", "")]);
        assert_eq!(plan(&mut c, json!({"op": "status"})).unwrap(), ("status --short --branch".into(), false));
        assert_eq!(plan(&mut c, json!({"op": "diff"})).unwrap().0, "diff --no-ext-diff --no-textconv --patch-with-stat");
        assert_eq!(plan(&mut c, json!({"op": "diff", "stat": true})).unwrap().0, "diff --no-ext-diff --no-textconv --stat");
        assert_eq!(plan(&mut c, json!({"op": "diff", "paths": ["a.rs"]})).unwrap().0, "diff --no-ext-diff --no-textconv --patch-with-stat -- a.rs");
        assert_eq!(plan(&mut c, json!({"op": "commit", "message": "--amend"})).unwrap(), ("commit -m --amend".into(), true));
        assert!(plan(&mut c, json!({"op": "add", "all": true})).unwrap().0.contains(":(exclude,glob)**/.env"));
        assert!(plan(&mut c, json!({"op": "show", "rev": "--output=/tmp/x"})).is_err());
        assert!(plan(&mut c, json!({"op": "switch", "name": "a;rm"})).is_err());
        assert!(plan(&mut c, json!({"op": "add", "paths": [".env"]})).is_err());
        assert!(plan(&mut c, json!({"op": "add", "paths": ["../x"]})).is_err());
        assert!(plan(&mut c, json!({"op": "push"})).is_err());
        assert!(plan(&mut c, json!({"op": "commit", "message": " "})).is_err());
    }

    /// Real git through the sandbox: reads leave `.git` untouched, writes work,
    /// and a planted hook never runs.
    #[tokio::test]
    async fn runs_sandboxed_without_hooks() {
        let mut c = ctx("git-run", &[("a.rs", "a\n")]);
        let root = c.ws.root.clone();
        let git = |args: &[&str]| std::process::Command::new("git").args(args).current_dir(&root).output().unwrap();
        git(&["init", "-q"]);
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init"]);
        std::fs::write(root.join(".git/hooks/pre-commit"), format!("#!/bin/sh\ntouch {}/HOOK_RAN\n", root.display())).unwrap();
        git(&["config", "user.name", "t"]);
        git(&["config", "user.email", "t@t"]);
        std::fs::set_permissions(root.join(".git/hooks/pre-commit"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

        let tool = all().into_iter().find(|t| t.def().name == "git").unwrap();
        let mut run = async |args: serde_json::Value| match tool.run(&mut c, &args.to_string()).unwrap() {
            Outcome::Exec(ex) => crate::exec::run(&ex, &root).await.unwrap(),
            _ => unreachable!(),
        };
        assert!(run(json!({"op": "status"})).await.contains("?? a.rs"));
        assert!(run(json!({"op": "add", "paths": ["a.rs"]})).await.starts_with("exit 0"));
        assert!(run(json!({"op": "commit", "message": "add a"})).await.starts_with("exit 0"));
        assert!(run(json!({"op": "log"})).await.contains("add a"));
        assert!(!root.join("HOOK_RAN").exists(), "hook ran");
    }
}
