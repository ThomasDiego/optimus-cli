use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::process::Command;

use crate::tools::truncate;

/// Which permission a launch needs, applied by the agent before running it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Review {
    /// Read-only (e.g. `git status`): never asked.
    None,
    /// Local, reversible state change (e.g. `git commit`): asked like a file write.
    Write,
    /// Arbitrary code or destructive: asked like a command.
    Exec,
}

/// A program launch prepared by a tool.
pub struct Exec {
    pub program: String,
    pub args: Vec<String>,
    pub dir: PathBuf,
    pub timeout: Duration,
    /// Network access, granted only after explicit user approval.
    pub net: bool,
    pub review: Review,
    /// Lets the sandbox write `.git` (git tool only).
    pub git: bool,
    /// Forced variables and leading arguments, not shown in approvals.
    pub env: &'static [(&'static str, &'static str)],
    pub hidden_args: &'static [&'static str],
}

impl Exec {
    pub fn new(program: String, args: Vec<String>, dir: PathBuf, timeout: Duration) -> Self {
        Self { program, args, dir, timeout, net: false, review: Review::Exec, git: false, env: &[], hidden_args: &[] }
    }

    pub fn display(&self) -> String {
        let quote = |a: &String| if a.contains(char::is_whitespace) { format!("{a:?}") } else { a.clone() };
        let cmd = std::iter::once(&self.program).chain(&self.args).map(quote).collect::<Vec<_>>().join(" ");
        if self.net { format!("{cmd} [réseau]") } else { cmd }
    }
}

/// The only variables a child sees: API keys and tokens never leak.
const KEEP_ENV: &[&str] = &["PATH", "HOME", "USER", "LANG", "TMPDIR"];
const HEAD: usize = 15;
const TAIL: usize = 45;
/// Per-process limits: CPU seconds, largest file written.
const CPU_SECS: libc::rlim_t = 900;
const MAX_FILE: libc::rlim_t = 2 << 30;

/// Kernel sandbox, see the profile for the exact rules.
#[cfg(target_os = "macos")]
const PROFILE: &str = include_str!("seatbelt.sb");

/// Kills the whole process group, so nothing a command spawned survives it.
struct Group(libc::pid_t);

impl Drop for Group {
    fn drop(&mut self) {
        // SAFETY: plain syscall on a group we created; errors (already gone) are ignored.
        unsafe { libc::killpg(self.0, libc::SIGKILL) };
    }
}

/// Runs without a shell, with a cleared environment, resource limits and a timeout.
pub async fn run(ex: &Exec, root: &Path) -> Result<String, String> {
    let mut cmd = command(ex, root);
    cmd.current_dir(&ex.dir)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    for k in KEEP_ENV {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    cmd.envs(ex.env.iter().copied());
    // SAFETY: only async-signal-safe setrlimit calls between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            for (res, v) in [(libc::RLIMIT_CPU, CPU_SECS), (libc::RLIMIT_FSIZE, MAX_FILE), (libc::RLIMIT_CORE, 0)] {
                libc::setrlimit(res, &libc::rlimit { rlim_cur: v, rlim_max: v });
            }
            Ok(())
        });
    }
    let start = Instant::now();
    let child = cmd.spawn().map_err(|e| format!("{}: {e}", ex.program))?;
    let _group = child.id().map(|pid| Group(pid as libc::pid_t));
    // On timeout, cancel or exit, `_group` kills every remaining process.
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
    let flag = |on: bool| Path::new(if on { "1" } else { "0" });
    let mut c = Command::new("/usr/bin/sandbox-exec");
    let params = [("ROOT", root), ("HOME", home.as_path()), ("TMP", tmp.as_path()), ("NET", flag(ex.net)), ("GIT", flag(ex.git))];
    for (k, v) in params {
        let mut def = OsString::from(format!("{k}="));
        def.push(v);
        c.arg("-D").arg(def);
    }
    c.arg("-p").arg(PROFILE).arg(&ex.program).args(ex.hidden_args).args(&ex.args);
    c
}

/// No kernel sandbox yet outside macOS: only no-shell, env clearing and approval apply.
#[cfg(not(target_os = "macos"))]
fn command(ex: &Exec, _root: &Path) -> Command {
    let mut c = Command::new(&ex.program);
    c.args(ex.hidden_args).args(&ex.args);
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
        Exec::new(program.into(), args, dir.into(), Duration::from_secs(secs))
    }

    #[tokio::test]
    async fn background_children_die_with_the_command() {
        let root = crate::sandbox::tests::temp("exec-group").canonicalize().unwrap();
        let out = run(&exec("/bin/sh", &["-c", "sleep 30 >/dev/null 2>&1 & echo $!"], &root, 5), &root).await.unwrap();
        let pid: libc::pid_t = out.lines().last().unwrap().trim().parse().unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        // SAFETY: signal 0 only checks whether the process exists.
        assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "background child survived");
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

        let home_list = run(&exec("/bin/ls", &[home.to_str().unwrap()], &root, 5), &root).await.unwrap();
        assert!(!home_list.starts_with("exit 0"), "HOME readable: {home_list}");

        let curl = ["-sS", "-m", "5", "-o", "/dev/null", "https://example.com"];
        let offline = run(&exec("/usr/bin/curl", &curl, &root, 10), &root).await.unwrap();
        assert!(!offline.starts_with("exit 0"), "network reachable: {offline}");
    }
}
