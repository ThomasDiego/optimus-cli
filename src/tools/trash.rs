//! Deleted directories are moved here rather than removed: one rename, even
//! for node_modules, and /undo renames them back. Each session owns
//! `.ai-trash/<pid>/`, purged on exit; a crashed session's is swept at the
//! next start. Tools and the kernel sandbox both keep the model out of it.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cap_std::fs::Dir;

use crate::sandbox::TRASH;

/// Counting stops here: past it the label says "N+".
pub const MAX_COUNT: usize = 100_000;

/// A fresh location for the `i`-th directory of a batch.
pub fn slot(i: usize) -> PathBuf {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    Path::new(TRASH).join(std::process::id().to_string()).join(format!("{now}-{i}"))
}

pub fn park(dir: &Dir, rel: &Path, slot: &Path) -> io::Result<()> {
    if let Some(parent) = slot.parent() {
        dir.create_dir_all(parent)?;
    }
    // Hides the trash from git, which would otherwise list it as untracked.
    let ignore = Path::new(TRASH).join(".gitignore");
    if !dir.try_exists(&ignore)? {
        dir.write(&ignore, "*\n")?;
    }
    dir.rename(rel, dir, slot)
}

/// Files under `rel`, symlinks not followed, capped at `MAX_COUNT`.
pub fn count(dir: &Dir, rel: &Path, n: &mut usize) -> io::Result<()> {
    for entry in dir.read_dir(rel)? {
        if *n >= MAX_COUNT {
            return Ok(());
        }
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            count(dir, &rel.join(entry.file_name()), n)?;
        } else {
            *n += 1;
        }
    }
    Ok(())
}

/// Removes this session's trash, and that of sessions no longer running.
pub fn purge(dir: &Dir, all_dead: bool) {
    let me = std::process::id().to_string();
    let Ok(entries) = dir.read_dir(TRASH) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let dead = || name.parse::<u32>().is_ok_and(|pid| !crate::os::alive(pid));
        if name == me || (all_dead && dead()) {
            let _ = dir.remove_dir_all(Path::new(TRASH).join(&name));
        }
    }
    // Kept while another running session still has a trash here.
    let others = dir.read_dir(TRASH).map(|e| e.flatten().any(|e| e.file_name() != ".gitignore")).unwrap_or(true);
    if !others {
        let _ = dir.remove_dir_all(TRASH);
    }
}
