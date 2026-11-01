use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::process::Child;

use crate::os::{self, Group};
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
    /// Keeps running after the call returns (servers, watchers); no timeout.
    pub background: bool,
}

impl Exec {
    pub fn new(program: String, args: Vec<String>, dir: PathBuf, timeout: Duration) -> Self {
        Self { program, args, dir, timeout, net: false, review: Review::Exec, git: false, env: &[], hidden_args: &[], background: false }
    }

    pub fn display(&self) -> String {
        let quote = |a: &String| if a.contains(char::is_whitespace) { format!("{a:?}") } else { a.clone() };
        let cmd = std::iter::once(&self.program).chain(&self.args).map(quote).collect::<Vec<_>>().join(" ");
        if self.net { format!("{cmd} [network]") } else { cmd }
    }
}

const HEAD: usize = 15;
const TAIL: usize = 45;

/// Runs without a shell, with a cleared environment, resource limits and a timeout.
pub async fn run(ex: &Exec, root: &Path) -> Result<String, String> {
    let start = Instant::now();
    let (child, _group) = spawn(ex, root)?;
    // On timeout, cancel or exit, `_group` kills every remaining process.
    let out = match tokio::time::timeout(ex.timeout, child.wait_with_output()).await {
        Ok(r) => r.map_err(|e| format!("{}: {e}", ex.program))?,
        Err(_) => return Err(format!("timeout after {}s, process killed", ex.timeout.as_secs())),
    };
    Ok(warned(format_output(out.status.code(), start.elapsed(), &out.stdout, &out.stderr)))
}

/// Prefixes output with the reason commands run unsandboxed, if they do.
pub fn warned(out: String) -> String {
    match os::warning() {
        Some(w) => format!("[{w}]\n{out}"),
        None => out,
    }
}

/// Starts the sandboxed command in its own session, output piped.
/// Dropping the `Group` kills it and everything it spawned.
pub fn spawn(ex: &Exec, root: &Path) -> Result<(Child, Group), String> {
    let mut cmd = os::command(ex, root)?;
    cmd.current_dir(&ex.dir)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for k in os::KEEP_ENV {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    cmd.envs(ex.env.iter().copied());
    let child = cmd.spawn().map_err(|e| format!("{}: {e}", ex.program))?;
    let group = Group::attach(&child).map_err(|e| format!("{}: {e}", ex.program))?;
    Ok((child, group))
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
pub fn clip(text: &str) -> String {
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

    #[cfg(unix)]
    #[tokio::test]
    async fn background_children_die_with_the_command() {
        let root = crate::sandbox::tests::temp("exec-group").canonicalize().unwrap();
        let out = run(&exec("/bin/sh", &["-c", "sleep 30 >/dev/null 2>&1 & echo $!"], &root, 5), &root).await.unwrap();
        let pid: libc::pid_t = out.lines().last().unwrap().trim().parse().unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        // SAFETY: signal 0 only checks whether the process exists.
        assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "background child survived");
    }

    #[cfg(unix)]
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

    /// Prompts get end-of-input instead of hanging on, or typing into, the
    /// user's terminal.
    #[cfg(unix)]
    #[tokio::test]
    async fn no_terminal_to_prompt_on() {
        let root = crate::sandbox::tests::temp("exec-tty").canonicalize().unwrap();
        let out = run(&exec("/bin/sh", &["-c", "read x; echo got=$?; : < /dev/tty && echo TTY"], &root, 5), &root).await.unwrap();
        assert!(out.contains("got=1") && !out.contains("TTY"), "{out}");
    }

    #[cfg(unix)]
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
        std::fs::create_dir(root.join("certs")).unwrap();
        for f in ["certs/server.key", ".npmrc", "id_ed25519", "auth.json"] {
            std::fs::write(root.join(f), "SECRET").unwrap();
            let out = run(&exec("/bin/cat", &[f], &root, 5), &root).await.unwrap();
            assert!(!out.contains("SECRET"), "{f}: {out}");
        }
        let ca = run(&exec("/bin/cat", &["/private/etc/ssl/cert.pem"], &root, 5), &root).await.unwrap();
        assert!(ca.starts_with("exit 0"), "system CA unreadable: {ca}");

        let home_list = run(&exec("/bin/ls", &[home.to_str().unwrap()], &root, 5), &root).await.unwrap();
        assert!(!home_list.starts_with("exit 0"), "HOME readable: {home_list}");

        let curl = ["-sS", "-m", "5", "-o", "/dev/null", "https://example.com"];
        let offline = run(&exec("/usr/bin/curl", &curl, &root, 10), &root).await.unwrap();
        assert!(!offline.starts_with("exit 0"), "network reachable: {offline}");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn landlock_confines_writes_reads_and_sockets() {
        if os::warning().is_some() && std::env::var_os("CI").is_none() {
            return eprintln!("skipped: no Landlock on this kernel");
        }
        let root = crate::sandbox::tests::temp("exec-ll").canonicalize().unwrap();
        let inside = root.join("in.txt");
        let ok = run(&exec("/usr/bin/touch", &[inside.to_str().unwrap()], &root, 5), &root).await.unwrap();
        assert!(ok.starts_with("exit 0") && inside.exists(), "{ok}");

        let home = os::home().unwrap();
        let outside = home.join(format!("ai-test-escape-{}", std::process::id()));
        let denied = run(&exec("/usr/bin/touch", &[outside.to_str().unwrap()], &root, 5), &root).await.unwrap();
        assert!(!denied.starts_with("exit 0") && !outside.exists(), "{denied}");
        let home_list = run(&exec("/bin/ls", &[home.to_str().unwrap()], &root, 5), &root).await.unwrap();
        assert!(!home_list.starts_with("exit 0"), "HOME readable: {home_list}");

        let curl = ["-sS", "-m", "5", "-o", "/dev/null", "https://example.com"];
        let offline = run(&exec("/usr/bin/curl", &curl, &root, 10), &root).await.unwrap();
        assert!(!offline.starts_with("exit 0"), "network reachable: {offline}");
        let py = |code: &str| exec("python3", &["-c", code], &root, 10);
        for (code, what) in [("socket.socket(socket.AF_UNIX)", "unix socket"), ("socket.socket(type=socket.SOCK_DGRAM)", "UDP")] {
            let out = run(&py(&format!("import socket; {code}")), &root).await.unwrap();
            assert!(!out.starts_with("exit 0"), "{what} allowed: {out}");
        }
        let listen = run(&py("import socket; s = socket.socket(); s.bind(('127.0.0.1', 0)); s.listen()"), &root).await.unwrap();
        assert!(listen.starts_with("exit 0"), "servers cannot listen: {listen}");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn job_kills_the_tree_and_env_is_cleared() {
        let root = os::plain_path(crate::sandbox::tests::temp("exec-win").canonicalize().unwrap());
        let out = run(&exec("cmd", &["/c", "set"], &root, 10), &root).await.unwrap();
        assert!(out.starts_with("exit 0") && !out.contains("CARGO_PKG_NAME"), "{out}");

        // The grandchild keeps no handle on our pipes: only the job can end it.
        let spawn = "import subprocess as s; print(s.Popen(['ping', '-n', '30', '127.0.0.1'], stdout=s.DEVNULL, stderr=s.DEVNULL).pid)";
        let out = run(&exec("python", &["-c", spawn], &root, 30), &root).await.unwrap();
        let pid: u32 = out.lines().last().unwrap().trim().parse().unwrap_or_else(|_| panic!("{out}"));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!os::alive(pid), "background child survived");

        let err = run(&exec("ping", &["-n", "10", "127.0.0.1"], &root, 1), &root).await.unwrap_err();
        assert!(err.starts_with("timeout"), "{err}");
    }
}
