//! Windows has no kernel sandbox here: commands run with the user's rights,
//! so only no-shell, the allowlist, env clearing and approval apply. A job
//! object still kills each command's whole process tree, caps its CPU time
//! and suppresses crash dialogs; commands get a hidden console of their own.

use std::io;
use std::path::{Path, PathBuf};

use tokio::process::{Child, Command};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_TIME, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, GetExitCodeProcess, OpenProcess, OpenThread, PROCESS_QUERY_LIMITED_INFORMATION, ResumeThread,
    THREAD_SUSPEND_RESUME,
};

use crate::exec::Exec;

/// The only variables a child sees: API keys and tokens never leak.
pub const KEEP_ENV: &[&str] = &[
    "PATH", "PATHEXT", "SYSTEMROOT", "SYSTEMDRIVE", "WINDIR", "COMSPEC", "USERPROFILE", "USERNAME", "APPDATA", "LOCALAPPDATA",
    "PROGRAMDATA", "TEMP", "TMP", "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE",
];
/// CPU time per process, in 100 ns units.
const CPU_TIME: i64 = 900 * 10_000_000;
/// Tried in order. npm, pnpm and yarn are `.cmd` scripts, which Windows runs
/// through cmd.exe; Rust escapes their arguments and refuses unsafe ones.
const EXTENSIONS: &[&str] = &["exe", "cmd", "bat"];

/// Kills the whole job, so nothing a command spawned survives it.
pub struct Group(HANDLE);

// SAFETY: a job handle may be used and closed from any thread.
unsafe impl Send for Group {}
unsafe impl Sync for Group {}

impl Group {
    /// Puts the suspended child in a fresh job, then lets it run: it cannot
    /// spawn anything outside the job first.
    pub fn attach(child: &Child) -> Result<Self, String> {
        let (Some(process), Some(pid)) = (child.raw_handle(), child.id()) else { return Err("exited at once".into()) };
        // SAFETY: plain Win32 calls on live handles; Drop closes the job on every error path.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(format!("job object: {}", io::Error::last_os_error()));
            }
            let group = Self(job);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_PROCESS_TIME | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            info.BasicLimitInformation.PerProcessUserTimeLimit = CPU_TIME;
            let size = size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
            let limited = SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&raw const info).cast(), size) != 0;
            if !limited || AssignProcessToJobObject(job, process as HANDLE) == 0 {
                return Err(format!("job object: {}", io::Error::last_os_error()));
            }
            if !resume(pid) {
                return Err(format!("resume: {}", io::Error::last_os_error()));
            }
            Ok(group)
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        // SAFETY: the handle is ours; terminating an emptied job is harmless.
        unsafe {
            TerminateJobObject(self.0, 1);
            CloseHandle(self.0);
        }
    }
}

/// Started suspended, without a visible console, from an absolute path.
pub fn command(ex: &Exec, _root: &Path) -> Result<Command, String> {
    let program = resolve(&ex.program).ok_or_else(|| format!("{}: not found in PATH", ex.program))?;
    let mut c = super::plain(program, ex);
    c.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    Ok(c)
}

/// Looked up in PATH only: Windows would otherwise try the working
/// directory first, running a `cargo.exe` planted in the project.
fn resolve(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let dirs = std::env::split_paths(&path).filter(|d| d.is_absolute()).collect::<Vec<_>>();
    dirs.iter().find_map(|dir| EXTENSIONS.iter().map(|ext| dir.join(format!("{name}.{ext}"))).find(|p| p.is_file()))
}

/// Resumes the threads of a process started suspended.
fn resume(pid: u32) -> bool {
    // SAFETY: snapshot and thread handles are closed before returning.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut resumed = false;
        let mut more = Thread32First(snap, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !thread.is_null() {
                    resumed |= ResumeThread(thread) != u32::MAX;
                    CloseHandle(thread);
                }
            }
            more = Thread32Next(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        resumed
    }
}

pub fn alive(pid: u32) -> bool {
    // SAFETY: query-only handle, closed right away.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code = 0;
        let running = GetExitCodeProcess(process, &mut code) != 0 && code == STILL_ACTIVE as u32;
        CloseHandle(process);
        running
    }
}
