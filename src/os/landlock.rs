//! Linux kernel sandbox (Landlock), the counterpart of seatbelt.sb: reads
//! limited to the system, toolchains and the project, writes to the project
//! and temp, package caches only with network approval, no TCP connection
//! without it, no signal to processes outside the sandbox. Landlock grants
//! whole trees and cannot carve exceptions out of them: unlike on macOS, the
//! project's secret files, `.git` and the trash stay reachable to programs.

use std::fs::OpenOptions;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tokio::process::Command;

use super::seccomp;
use crate::exec::Exec;

// Rights from include/uapi/linux/landlock.h.
const EXECUTE: u64 = 1 << 0;
const WRITE_FILE: u64 = 1 << 1;
const READ_FILE: u64 = 1 << 2;
const READ_DIR: u64 = 1 << 3;
const TRUNCATE: u64 = 1 << 14;
const IOCTL_DEV: u64 = 1 << 15;
const READ: u64 = EXECUTE | READ_FILE | READ_DIR;
/// Everything the running kernel handles: masked by `handled`.
const ALL: u64 = u64::MAX;
/// The only rights a rule on a file (not a directory) may carry.
const FILE_RIGHTS: u64 = EXECUTE | WRITE_FILE | READ_FILE | TRUNCATE | IOCTL_DEV;
const CONNECT_TCP: u64 = 1 << 1;
const SCOPE_SIGNAL: u64 = 1 << 1;
const RULE_PATH_BENEATH: libc::c_int = 1;
const CREATE_RULESET_VERSION: libc::c_uint = 1;

/// Readable system trees; missing ones are skipped.
const SYSTEM: &[&str] = &[
    "/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32", "/etc", "/opt", "/nix", "/snap", "/proc", "/sys",
    "/run/systemd/resolve", "/run/NetworkManager", "/run/resolvconf", "/run/current-system", "/home/linuxbrew/.linuxbrew",
];
/// Toolchains under HOME. Cargo credentials stay unreadable.
const HOME_READ: &[&str] = &[
    ".cargo/bin", ".cargo/registry", ".cargo/git", ".cargo/config.toml", ".cargo/config", ".rustup", ".npm", ".nvm",
    ".bun", ".deno", ".pyenv", "go", ".local/bin", ".local/lib", ".local/share/pnpm", ".gitconfig", ".config/git",
];
/// Writable only when downloading, so offline commands cannot poison caches
/// shared with other projects.
const HOME_CACHES: &[&str] = &[".cargo/registry", ".cargo/git", ".npm", ".cache"];

#[repr(C)]
struct RulesetAttr {
    handled_fs: u64,
    handled_net: u64,
    scoped: u64,
}

#[repr(C, packed)]
struct PathBeneath {
    allowed: u64,
    parent_fd: RawFd,
}

/// Landlock version of the running kernel, 0 without Landlock.
pub fn abi() -> i64 {
    static ABI: OnceLock<i64> = OnceLock::new();
    // SAFETY: the version query reads no memory.
    *ABI.get_or_init(|| {
        let v = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, std::ptr::null::<RulesetAttr>(), 0usize, CREATE_RULESET_VERSION) };
        (v as i64).max(0)
    })
}

/// The program, confined once forked: rules are built here, the child only
/// enforces them.
pub fn command(ex: &Exec, root: &Path) -> Result<Command, String> {
    let mut cmd = super::plain(&ex.program, ex);
    let ruleset = ruleset(root, ex.net).map_err(|e| format!("sandbox: {e}"))?;
    let filter = seccomp::filter(ex.net, abi() >= 4);
    // SAFETY: only async-signal-safe prctl/syscall calls between fork and exec; nothing allocates.
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            if let Some(fd) = &ruleset
                && libc::syscall(libc::SYS_landlock_restrict_self, fd.as_raw_fd(), 0u32) != 0
            {
                return Err(io::Error::last_os_error());
            }
            filter.as_deref().map_or(Ok(()), seccomp::install)
        });
    }
    Ok(cmd)
}

fn ruleset(root: &Path, net: bool) -> io::Result<Option<OwnedFd>> {
    let abi = abi();
    if abi < 1 {
        return Ok(None);
    }
    let handled: u64 = match abi {
        1 => (1 << 13) - 1,
        2 => (1 << 14) - 1,
        3 | 4 => (1 << 15) - 1,
        _ => (1 << 16) - 1,
    };
    let attr = RulesetAttr {
        handled_fs: handled,
        // Binding stays free: dev servers may listen.
        handled_net: if abi >= 4 && !net { CONNECT_TCP } else { 0 },
        scoped: if abi >= 6 { SCOPE_SIGNAL } else { 0 },
    };
    // SAFETY: attr outlives the call; the returned fd is owned below.
    let fd = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, &raw const attr, size_of::<RulesetAttr>(), 0u32) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh descriptor nobody else owns.
    let fd = unsafe { OwnedFd::from_raw_fd(fd as RawFd) };
    for (path, access) in rules(root, net) {
        add(&fd, &path, access & handled)?;
    }
    Ok(Some(fd))
}

fn rules(root: &Path, net: bool) -> Vec<(PathBuf, u64)> {
    let mut rules: Vec<(PathBuf, u64)> = SYSTEM.iter().map(|p| (p.into(), READ)).collect();
    rules.push(("/dev".into(), READ | WRITE_FILE | IOCTL_DEV));
    for p in [Path::new("/tmp"), Path::new("/dev/shm"), &std::env::temp_dir(), root] {
        rules.push((p.into(), ALL));
    }
    if let Some(home) = super::home() {
        rules.extend(HOME_READ.iter().map(|p| (home.join(p), READ)));
        if net {
            rules.extend(HOME_CACHES.iter().map(|p| (home.join(p), ALL)));
        }
    }
    rules
}

fn add(ruleset: &OwnedFd, path: &Path, access: u64) -> io::Result<()> {
    let Ok(file) = OpenOptions::new().read(true).custom_flags(libc::O_PATH).open(path) else {
        return Ok(());
    };
    let allowed = if file.metadata()?.is_dir() { access } else { access & FILE_RIGHTS };
    let attr = PathBeneath { allowed, parent_fd: file.as_raw_fd() };
    // SAFETY: both descriptors are open and attr outlives the call.
    let r = unsafe { libc::syscall(libc::SYS_landlock_add_rule, ruleset.as_raw_fd(), RULE_PATH_BENEATH, &raw const attr, 0u32) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}
