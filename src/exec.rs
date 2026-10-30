use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::process::Command;

use crate::tools::truncate;

/// A program launch prepared by the `run` tool.
pub struct Exec {
    pub program: String,
    pub args: Vec<String>,
    pub dir: PathBuf,
    pub timeout: Duration,
}

impl Exec {
    pub fn display(&self) -> String {
        let quote = |a: &String| if a.contains(char::is_whitespace) { format!("{a:?}") } else { a.clone() };
        std::iter::once(&self.program).chain(&self.args).map(quote).collect::<Vec<_>>().join(" ")
    }
}

/// The only variables a child sees: API keys and tokens never leak.
const KEEP_ENV: &[&str] = &["PATH", "HOME", "USER", "LANG", "TMPDIR"];
const HEAD: usize = 15;
const TAIL: usize = 45;

/// Kernel sandbox: writes only in the project, temp dirs and package caches;
/// no reads of credentials or `.env` files. `.git` stays read-only.
#[cfg(target_os = "macos")]
const PROFILE: &str = r#"(version 1)
(allow default)
(deny file-write*)
(allow file-write*
  (subpath (param "ROOT"))
  (subpath (param "TMP"))
  (subpath "/private/tmp")
  (subpath "/dev")
  (subpath (string-append (param "HOME") "/.cargo/registry"))
  (subpath (string-append (param "HOME") "/.cargo/git"))
  (subpath (string-append (param "HOME") "/.npm"))
  (subpath (string-append (param "HOME") "/.cache"))
  (subpath (string-append (param "HOME") "/Library/Caches")))
(deny file-write* (subpath (string-append (param "ROOT") "/.git")))
(deny file-read*
  (subpath (string-append (param "HOME") "/.ssh"))
  (subpath (string-append (param "HOME") "/.aws"))
  (subpath (string-append (param "HOME") "/.gnupg"))
  (subpath (string-append (param "HOME") "/.config"))
  (literal (string-append (param "HOME") "/.netrc"))
  (regex #"/\.env$")
  (regex #"/\.env\.[^/]*$"))
(allow file-read* (regex #"/\.env\.(example|sample|template)$"))"#;

/// Runs without a shell, with a cleared environment and a timeout.
pub async fn run(ex: &Exec, root: &Path) -> Result<String, String> {
    let mut cmd = command(ex, root);
    cmd.current_dir(&ex.dir)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for k in KEEP_ENV {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    let start = Instant::now();
    let child = cmd.spawn().map_err(|e| format!("{}: {e}", ex.program))?;
    // On timeout or cancel the child is dropped, which kills it.
    let out = match tokio::time::timeout(ex.timeout, child.wait_with_output()).await {
        Ok(r) => r.map_err(|e| format!("{}: {e}", ex.program))?,
        Err(_) => return Err(format!("timeout after {}s, process killed", ex.timeout.as_secs())),
    };
    Ok(format_output(out.status.code(), start.elapsed(), &out.stdout, &out.stderr))
}

#[cfg(target_os = "macos")]
fn command(ex: &Exec, root: &Path) -> Command {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/var/empty".into());
    let tmp = std::env::temp_dir().canonicalize().unwrap_or_else(|_| "/private/tmp".into());
    // Per-user temp is .../T; its parent also holds the per-user cache (C).
    let tmp = if tmp.ends_with("T") { tmp.parent().map(Path::to_path_buf).unwrap_or(tmp) } else { tmp };
    let mut c = Command::new("/usr/bin/sandbox-exec");
    for (k, v) in [("ROOT", root), ("HOME", home.as_path()), ("TMP", tmp.as_path())] {
        let mut def = OsString::from(format!("{k}="));
        def.push(v);
        c.arg("-D").arg(def);
    }
    c.arg("-p").arg(PROFILE).arg(&ex.program).args(&ex.args);
    c
}

/// No kernel sandbox yet outside macOS: only no-shell, env clearing and approval apply.
#[cfg(not(target_os = "macos"))]
fn command(ex: &Exec, _root: &Path) -> Command {
    let mut c = Command::new(&ex.program);
    c.args(&ex.args);
    c
}

fn format_output(code: Option<i32>, took: Duration, stdout: &[u8], stderr: &[u8]) -> String {
    let status = code.map_or("killed".to_string(), |c| format!("exit {c}"));
    let mut out = format!("{status} {:.1}s", took.as_secs_f32());
    for (name, bytes) in [("stdout", stdout), ("stderr", stderr)] {
        let text = String::from_utf8_lossy(bytes);
        if !text.trim().is_empty() {
            out.push_str(&format!("\n[{name}]\n{}", clip(&text)));
        }
    }
    out
}

/// Keeps the head (first errors) and the tail (summary), drops the middle.
fn clip(text: &str) -> String {
    let lines: Vec<String> = text.trim_end().lines().map(|l| truncate(l, 300).into_owned()).collect();
    if lines.len() <= HEAD + TAIL {
        return lines.join("\n");
    }
    let cut = lines.len() - HEAD - TAIL;
    format!("{}\n[{cut} lines cut]\n{}", lines[..HEAD].join("\n"), lines[lines.len() - TAIL..].join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec(program: &str, args: &[&str], dir: &Path, secs: u64) -> Exec {
        let args = args.iter().map(|s| s.to_string()).collect();
        Exec { program: program.into(), args, dir: dir.into(), timeout: Duration::from_secs(secs) }
    }

    #[tokio::test]
    async fn env_is_cleared_and_output_compact() {
        let root = crate::sandbox::tests::temp("exec-env").canonicalize().unwrap();
        let out = run(&exec("/usr/bin/env", &[], &root, 5), &root).await.unwrap();
        assert!(out.starts_with("exit 0"));
        assert!(!out.contains("CARGO_PKG_NAME"), "env leaked: {out}");
        let long: String = (1..=100).map(|i| format!("{i}\n")).collect();
        let clipped = clip(&long);
        assert!(clipped.contains("15\n[40 lines cut]\n56"));
    }

    #[tokio::test]
    async fn timeout_kills() {
        let root = crate::sandbox::tests::temp("exec-timeout").canonicalize().unwrap();
        let err = run(&exec("/bin/sleep", &["5"], &root, 1), &root).await.unwrap_err();
        assert!(err.starts_with("timeout"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn seatbelt_confines_writes_and_secret_reads() {
        let root = crate::sandbox::tests::temp("exec-sb").canonicalize().unwrap();
        std::fs::write(root.join(".env"), "KEY=1").unwrap();
        let inside = root.join("in.txt");
        let ok = run(&exec("/usr/bin/touch", &[inside.to_str().unwrap()], &root, 5), &root).await.unwrap();
        assert!(ok.starts_with("exit 0") && inside.exists());

        // The project lives in temp during tests, so escape towards HOME instead.
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let outside = home.join(format!("ai-test-escape-{}", std::process::id()));
        let denied = run(&exec("/usr/bin/touch", &[outside.to_str().unwrap()], &root, 5), &root).await.unwrap();
        assert!(!denied.starts_with("exit 0") && !outside.exists(), "{denied}");

        let env = run(&exec("/bin/cat", &[".env"], &root, 5), &root).await.unwrap();
        assert!(!env.contains("KEY=1"), "{env}");
    }
}
