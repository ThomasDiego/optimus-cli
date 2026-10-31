use std::io;
use std::path::Path;

use tokio::process::{Child, Command};

use crate::exec::Exec;

/// The only variables a child sees: API keys and tokens never leak.
pub const KEEP_ENV: &[&str] = &["PATH", "HOME", "USER", "LANG", "TMPDIR"];
/// Per-process limits: CPU seconds, largest file written.
const CPU_SECS: libc::rlim_t = 900;
const MAX_FILE: libc::rlim_t = 2 << 30;

/// Kills the whole process group, so nothing a command spawned survives it.
pub struct Group(libc::pid_t);

impl Group {
    pub fn attach(child: &Child) -> Result<Self, String> {
        // The child called setsid: its pid is its group id.
        child.id().map(|pid| Self(pid as libc::pid_t)).ok_or_else(|| "exited at once".into())
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        // SAFETY: plain syscall on a group we created; errors (already gone) are ignored.
        unsafe { libc::killpg(self.0, libc::SIGKILL) };
    }
}

/// The sandboxed command, started in its own session with resource limits.
pub fn command(ex: &Exec, root: &Path) -> Result<Command, String> {
    #[cfg(target_os = "macos")]
    let mut cmd = super::seatbelt::command(ex, root);
    #[cfg(target_os = "linux")]
    let mut cmd = super::landlock::command(ex, root)?;
    // No kernel sandbox elsewhere: only no-shell, env clearing and approval apply.
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let mut cmd = {
        let _ = root;
        super::plain(&ex.program, ex)
    };
    // SAFETY: only async-signal-safe setsid/setrlimit calls between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            // New session, hence new process group and no controlling
            // terminal: /dev/tty is unreachable, prompts read end-of-input.
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            for (res, v) in [(libc::RLIMIT_CPU, CPU_SECS), (libc::RLIMIT_FSIZE, MAX_FILE), (libc::RLIMIT_CORE, 0)] {
                libc::setrlimit(res, &libc::rlimit { rlim_cur: v, rlim_max: v });
            }
            Ok(())
        });
    }
    Ok(cmd)
}

pub fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else { return false };
    // SAFETY: signal 0 only checks existence; EPERM still means it exists.
    let found = unsafe { libc::kill(pid, 0) == 0 };
    found || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
