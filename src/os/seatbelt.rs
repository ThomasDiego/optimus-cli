use std::ffi::OsString;
use std::path::Path;

use tokio::process::Command;

use crate::exec::Exec;

/// Kernel sandbox, see the profile for the exact rules.
const PROFILE: &str = include_str!("../seatbelt.sb");

pub fn command(ex: &Exec, root: &Path) -> Command {
    let home = super::home().unwrap_or_else(|| "/var/empty".into());
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
