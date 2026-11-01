use std::path::Path;
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
const SECRET_EXCLUDES: &[&str] = &[
    ":(exclude,glob)**/.env",
    ":(exclude,glob)**/.env.*",
    ":(exclude,glob)**/*.pem",
    ":(exclude,glob)**/*.key",
    ":(exclude,glob)**/*.p12",
    ":(exclude,glob)**/*.pfx",
    ":(exclude,glob)**/id_rsa*",
    ":(exclude,glob)**/id_ed25519*",
    ":(exclude,glob)**/id_ecdsa*",
    ":(exclude,glob)**/id_dsa*",
    ":(exclude,glob)**/.npmrc",
    ":(exclude,glob)**/.pypirc",
    ":(exclude,glob)**/.netrc",
    ":(exclude,glob)**/.git-credentials",
    ":(exclude,glob)**/auth.json",
];

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
    Init,
    Stash,
    Uncommit,
    Revert,
    #[serde(rename = "cherry_pick")]
    CherryPick,
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
    #[serde(default)]
    pop: bool,
}

impl Tool for Git {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "git",
            description: "Local git, no network; writes return the new status. diff/show: stat + patch. commit takes only what add staged. restore discards working changes. uncommit: undo the last commit, its changes stay staged. revert/cherry_pick rev: on conflict, fix, add, commit.",
            schema: json!({
                "type": "object",
                "properties": {
                    "op": {"type": "string", "enum": ["status", "diff", "log", "show", "add", "commit", "branch", "switch", "restore", "init", "stash", "uncommit", "revert", "cherry_pick"]},
                    "paths": {"type": "array", "items": {"type": "string"}},
                    "rev": {"type": "string", "description": "commit (show: default HEAD); log: branch or a..b"},
                    "message": {"type": "string"},
                    "name": {"type": "string", "description": "branch: create it (none: list); switch: go to it, create=true for a new one"},
                    "n": {"type": "integer", "description": "log entries, default 10"},
                    "staged": {"type": "boolean", "description": "diff/restore the index"},
                    "all": {"type": "boolean", "description": "add every change (secrets excluded)"},
                    "stat": {"type": "boolean", "description": "diff: stat only"},
                    "create": {"type": "boolean"},
                    "pop": {"type": "boolean", "description": "stash: restore the latest"}
                },
                "required": ["op"]
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        // A parent repository is out of the sandbox's reach anyway.
        match (matches!(a.op, Op::Init), ctx.ws.dir.is_dir(".git")) {
            (false, false) => return Err("not a git repository; init creates one".into()),
            (true, true) => return Err("already a git repository".into()),
            _ => {}
        }
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
                if let Some(rev) = a.rev.as_deref() {
                    push(safe_name(rev)?);
                }
                Review::None
            }
            Op::Show => {
                let rev = safe_name(a.rev.as_deref().unwrap_or("HEAD"))?;
                ["show", "--no-ext-diff", "--no-textconv"].iter().for_each(|s| push(s));
                if paths.is_empty() {
                    // Stat then patch: without it `--stat` alone hides the diff,
                    // and the model would retry through a shell to see it.
                    push("--patch-with-stat");
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
            Op::Init => {
                ["init", "-q"].iter().for_each(|s| push(s));
                Review::Write
            }
            Op::Stash if a.pop => {
                ["stash", "pop"].iter().for_each(|s| push(s));
                Review::Write
            }
            Op::Stash => {
                ["stash", "push"].iter().for_each(|s| push(s));
                if let Some(msg) = a.message.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
                    ["-m", msg].iter().for_each(|s| push(s));
                }
                Review::Write
            }
            // Soft only: the commit goes, its changes stay staged.
            Op::Uncommit => {
                ["reset", "--soft", "HEAD~1"].iter().for_each(|s| push(s));
                Review::Write
            }
            // No editor can open: commands have no tty.
            Op::Revert | Op::CherryPick => {
                let rev = safe_name(a.rev.as_deref().ok_or("needs rev")?)?;
                let op = if matches!(a.op, Op::Revert) { "revert" } else { "cherry-pick" };
                [op, "--no-edit", rev].iter().for_each(|s| push(s));
                Review::Write
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

/// Branch and changed files, read-only and hardened like the tool. The agent
/// shows it up front and after every git write, so the model never spends a
/// step asking where it stands.
pub async fn status(root: &Path) -> String {
    super::capped(read_only(root, &["status", "--short", "--branch"]).await.lines().map(String::from).collect(), 15)
}

/// Appended to a git write's result so the model has nothing to check: the
/// new status, plus the patch of the commit a revert or cherry_pick made.
pub async fn after_write(root: &Path, args: &[String], ok: bool) -> String {
    let mut out = format!("[git status]\n{}", status(root).await);
    if ok && matches!(args.first().map(String::as_str), Some("revert" | "cherry-pick")) {
        let patch = read_only(root, &["show", "--no-ext-diff", "--no-textconv", "--format=", "--patch-with-stat", "HEAD"]).await;
        out.push_str(&format!("\n[new commit]\n{}", crate::exec::clip(&patch)));
    }
    out
}

/// Stdout of a hardened, read-only git command.
async fn read_only(root: &Path, args: &[&str]) -> String {
    let args = args.iter().map(|s| s.to_string()).collect();
    let ex = Exec { review: Review::None, env: ENV, hidden_args: HARDENING, ..Exec::new("git".into(), args, root.to_path_buf(), TIMEOUT) };
    let out = crate::exec::run(&ex, root).await.unwrap_or_default();
    out.split_once("[stdout]\n").map_or("", |(_, s)| s.split("\n[stderr]").next().unwrap_or("")).trim().to_string()
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
        assert!(plan(&mut c, json!({"op": "status"})).unwrap_err().starts_with("not a git repository"));
        assert_eq!(plan(&mut c, json!({"op": "init"})).unwrap(), ("init -q".into(), true));
        std::fs::create_dir(c.ws.root.join(".git")).unwrap();
        assert_eq!(plan(&mut c, json!({"op": "init"})).unwrap_err(), "already a git repository");
        assert_eq!(plan(&mut c, json!({"op": "uncommit"})).unwrap(), ("reset --soft HEAD~1".into(), true));
        assert_eq!(plan(&mut c, json!({"op": "stash", "message": "wip"})).unwrap().0, "stash push -m wip");
        assert_eq!(plan(&mut c, json!({"op": "status"})).unwrap(), ("status --short --branch".into(), false));
        assert_eq!(plan(&mut c, json!({"op": "diff"})).unwrap().0, "diff --no-ext-diff --no-textconv --patch-with-stat");
        assert_eq!(plan(&mut c, json!({"op": "diff", "stat": true})).unwrap().0, "diff --no-ext-diff --no-textconv --stat");
        assert_eq!(plan(&mut c, json!({"op": "diff", "paths": ["a.rs"]})).unwrap().0, "diff --no-ext-diff --no-textconv --patch-with-stat -- a.rs");
        assert_eq!(plan(&mut c, json!({"op": "commit", "message": "--amend"})).unwrap(), ("commit -m --amend".into(), true));
        assert!(plan(&mut c, json!({"op": "add", "all": true})).unwrap().0.contains(":(exclude,glob)**/.env"));
        assert_eq!(plan(&mut c, json!({"op": "show", "rev": "HEAD"})).unwrap().0, "show --no-ext-diff --no-textconv --patch-with-stat HEAD");
        assert!(plan(&mut c, json!({"op": "show", "rev": "--output=/tmp/x"})).is_err());
        assert_eq!(plan(&mut c, json!({"op": "log", "rev": "main..hotfix"})).unwrap().0, "log --oneline -n 10 main..hotfix");
        assert!(plan(&mut c, json!({"op": "log", "rev": "--output=/tmp/x"})).is_err());
        assert!(plan(&mut c, json!({"op": "switch", "name": "a;rm"})).is_err());
        assert!(plan(&mut c, json!({"op": "add", "paths": [".env"]})).is_err());
        assert!(plan(&mut c, json!({"op": "add", "paths": ["../x"]})).is_err());
        assert!(plan(&mut c, json!({"op": "push"})).is_err());
        assert!(plan(&mut c, json!({"op": "commit", "message": " "})).is_err());
    }

    /// Real git through the sandbox: reads leave `.git` untouched, writes work,
    /// and a planted hook never runs.
    #[cfg(unix)]
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
        std::fs::write(root.join("b.rs"), "").unwrap();
        let st = super::status(&root).await;
        assert!(st.starts_with("## ") && st.ends_with("?? b.rs"), "{st}");
        std::fs::remove_file(root.join("b.rs")).unwrap();

        // Committed on the wrong branch: uncommit keeps the work, stash parks it.
        assert!(run(json!({"op": "uncommit"})).await.starts_with("exit 0"));
        assert!(!run(json!({"op": "log"})).await.contains("add a"));
        assert!(run(json!({"op": "status"})).await.contains("A  a.rs"));
        assert!(run(json!({"op": "stash"})).await.starts_with("exit 0"));
        assert!(!root.join("a.rs").exists());
        assert!(run(json!({"op": "stash", "pop": true})).await.starts_with("exit 0"));
        assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), "a\n");

        // Commit, revert it, then bring it back with cherry_pick: no editor hangs.
        assert!(run(json!({"op": "commit", "message": "add a"})).await.starts_with("exit 0"));
        assert!(run(json!({"op": "revert", "rev": "HEAD"})).await.starts_with("exit 0"));
        assert!(!root.join("a.rs").exists());
        // The model sees what the revert did without reading the files.
        let revert = vec!["revert".to_string()];
        let after = super::after_write(&root, &revert, true).await;
        assert!(after.starts_with("[git status]\n## ") && after.contains("[new commit]") && after.contains("-a"), "{after}");
        assert!(!super::after_write(&root, &revert, false).await.contains("[new commit]"));
        assert!(run(json!({"op": "cherry_pick", "rev": "HEAD~1"})).await.starts_with("exit 0"));
        assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), "a\n");
    }

    #[tokio::test]
    async fn init_creates_a_repository_in_the_sandbox() {
        let mut c = ctx("git-init", &[("a.rs", "")]);
        let tool = all().into_iter().find(|t| t.def().name == "git").unwrap();
        let Outcome::Exec(ex) = tool.run(&mut c, r#"{"op":"init"}"#).unwrap() else { unreachable!() };
        assert!(crate::exec::run(&ex, &c.ws.root).await.unwrap().starts_with("exit 0"));
        assert!(c.ws.root.join(".git/HEAD").exists());
    }
}
