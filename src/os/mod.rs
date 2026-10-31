//! What differs between operating systems: how a command is confined, how
//! its whole process tree is killed, and where the user's files live.

#[cfg(target_os = "linux")]
mod landlock;
#[cfg(target_os = "macos")]
mod seatbelt;
#[cfg(target_os = "linux")]
mod seccomp;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use std::path::PathBuf;

use tokio::process::Command;

use crate::exec::Exec;
#[cfg(unix)]
pub use unix::{Group, KEEP_ENV, alive, command};
#[cfg(windows)]
pub use windows::{Group, KEEP_ENV, alive, command};

/// Why commands run without a kernel sandbox on this machine, if they do.
pub fn warning() -> Option<&'static str> {
    #[cfg(target_os = "linux")]
    if landlock::abi() < 1 {
        return Some("no kernel sandbox: Landlock is unavailable, programs can read and write outside the project");
    }
    None
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// `$XDG_CONFIG_HOME`, else `~/.config`, or `%APPDATA%` on Windows.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        return dir.into();
    }
    if cfg!(windows) {
        return std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    }
    home().unwrap_or_default().join(".config")
}

/// Canonical Windows paths start with `\\?\`, which cmd.exe and many tools
/// refuse as a working directory: dropped for plain drive paths.
pub fn plain_path(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(rest) = p.to_str().and_then(|s| s.strip_prefix(r"\\?\")).filter(|r| r.as_bytes().get(1) == Some(&b':')) {
        return PathBuf::from(rest);
    }
    p
}

/// Project paths use `/` everywhere, as the model writes them; Windows
/// accepts it as a separator.
pub fn slashes(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(s) = p.to_str().filter(|s| s.contains('\\')) {
        return PathBuf::from(s.replace('\\', "/"));
    }
    p
}

/// The program itself, without any sandbox.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn plain(program: impl AsRef<std::ffi::OsStr>, ex: &Exec) -> Command {
    let mut c = Command::new(program);
    c.args(ex.hidden_args).args(&ex.args);
    c
}
