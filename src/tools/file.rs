use std::borrow::Cow;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::Path;

use cap_std::fs::Dir;

use super::Ctx;

const MAX_BYTES: u64 = 5_000_000;

pub fn hash(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

pub fn io_err(path: &str, e: io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => format!("{path}: not found"),
        io::ErrorKind::PermissionDenied => format!("{path}: outside project or permission denied"),
        _ => format!("{path}: {e}"),
    }
}

pub fn truncate(s: &str, max: usize) -> Cow<'_, str> {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]).into(),
        None => s.into(),
    }
}

/// Reads a UTF-8 (lossy) text file, refusing binaries and huge files.
pub fn read_text(ctx: &Ctx, rel: &Path, shown: &str) -> Result<String, String> {
    let meta = ctx.ws.dir.metadata(rel).map_err(|e| io_err(shown, e))?;
    if meta.is_dir() {
        return Err(format!("{shown}: directory, use list_dir"));
    }
    if meta.len() > MAX_BYTES {
        return Err(format!("{shown}: too large ({} KB)", meta.len() / 1024));
    }
    let bytes = ctx.ws.dir.read(rel).map_err(|e| io_err(shown, e))?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err(format!("{shown}: binary file, {} KB", bytes.len() / 1024));
    }
    Ok(String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

/// Edits require the model to have seen the current content.
pub fn ensure_fresh(ctx: &Ctx, rel: &Path, current: &str, shown: &str) -> Result<(), String> {
    match ctx.reads.get(rel) {
        None => Err(format!("{shown}: read the file before modifying it")),
        Some(s) if s.hash != hash(current) => Err(format!("{shown}: changed since last read, read it again")),
        Some(_) => Ok(()),
    }
}

pub fn write_atomic(dir: &Dir, rel: &Path, data: &str) -> io::Result<()> {
    let name = rel.file_name().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let parent = rel.parent().unwrap_or(Path::new(""));
    if !parent.as_os_str().is_empty() {
        dir.create_dir_all(parent)?;
    }
    let tmp = parent.join(format!(".{}.ai-tmp", name.to_string_lossy()));
    let perms = dir.metadata(rel).ok().map(|m| m.permissions());
    dir.write(&tmp, data)?;
    if let Some(p) = perms {
        dir.set_permissions(&tmp, p)?;
    }
    dir.rename(&tmp, dir, rel).inspect_err(|_| {
        let _ = dir.remove_file(&tmp);
    })
}
