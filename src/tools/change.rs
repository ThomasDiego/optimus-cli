use std::path::PathBuf;

use similar::TextDiff;

use super::{Ctx, FileState, file};

/// Undo history depth, in turns.
const MAX_TURNS: usize = 20;

/// A file write prepared by a tool, applied by the agent once permitted.
pub struct Change {
    pub rel: PathBuf,
    pub old: Option<String>,
    pub new: String,
    /// Message returned to the model on success.
    pub ok: String,
}

/// File content before the first change of a turn.
pub struct Snapshot {
    rel: PathBuf,
    old: Option<String>,
}

impl Change {
    /// Unified diff for the user only; never sent to the model.
    pub fn diff(&self) -> String {
        TextDiff::from_lines(self.old.as_deref().unwrap_or(""), &self.new)
            .unified_diff()
            .context_radius(2)
            .to_string()
    }
}

/// The single write path: re-checks freshness, snapshots, writes atomically.
pub fn apply(ctx: &mut Ctx, ch: Change) -> Result<String, String> {
    let shown = ch.rel.display().to_string();
    let current = match ctx.ws.dir.try_exists(&ch.rel).map_err(|e| file::io_err(&shown, e))? {
        true => Some(file::read_text(ctx, &ch.rel, &shown)?),
        false => None,
    };
    // The user may have edited the file while the approval prompt was open.
    if current != ch.old {
        return Err(format!("{shown}: changed before write, read it again"));
    }
    if let Some(turn) = ctx.undo.last_mut()
        && !turn.iter().any(|s| s.rel == ch.rel)
    {
        turn.push(Snapshot { rel: ch.rel.clone(), old: ch.old });
    }
    file::write_atomic(&ctx.ws.dir, &ch.rel, &ch.new).map_err(|e| file::io_err(&shown, e))?;
    // The model knows what it wrote: no re-read needed before the next edit.
    ctx.reads.insert(ch.rel, FileState { hash: file::hash(&ch.new), ranges: vec![] });
    Ok(ch.ok)
}

/// Opens a new undo group for the coming turn.
pub fn begin_turn(ctx: &mut Ctx) {
    if ctx.undo.last().is_some_and(Vec::is_empty) {
        return;
    }
    ctx.undo.push(vec![]);
    if ctx.undo.len() > MAX_TURNS {
        ctx.undo.remove(0);
    }
}

/// Restores files changed by the last turn that changed any. Returns their paths.
pub fn undo(ctx: &mut Ctx) -> Result<Vec<PathBuf>, String> {
    while ctx.undo.last().is_some_and(Vec::is_empty) {
        ctx.undo.pop();
    }
    let Some(turn) = ctx.undo.pop() else { return Ok(vec![]) };
    let mut restored = vec![];
    for s in turn.into_iter().rev() {
        let shown = s.rel.display().to_string();
        match &s.old {
            Some(old) => file::write_atomic(&ctx.ws.dir, &s.rel, old),
            None => ctx.ws.dir.remove_file(&s.rel),
        }
        .map_err(|e| file::io_err(&shown, e))?;
        // Forces a re-read: the model's view of this file is now stale.
        ctx.reads.remove(&s.rel);
        restored.push(s.rel);
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::tools::tests::{call, ctx};

    #[test]
    fn undo_restores_edits_and_removes_created_files() {
        let mut c = ctx("undo", &[("a.txt", "one\n")]);
        begin_turn(&mut c);
        call(&mut c, "read_file", json!({"path": "a.txt"})).unwrap();
        call(&mut c, "edit_file", json!({"path": "a.txt", "edits": [{"old": "one", "new": "two"}]})).unwrap();
        call(&mut c, "edit_file", json!({"path": "a.txt", "edits": [{"old": "two", "new": "three"}]})).unwrap();
        call(&mut c, "write_file", json!({"path": "new.txt", "content": "x"})).unwrap();
        begin_turn(&mut c);

        let mut restored = undo(&mut c).unwrap();
        restored.sort();
        assert_eq!(restored, [PathBuf::from("a.txt"), PathBuf::from("new.txt")]);
        assert_eq!(std::fs::read_to_string(c.ws.root.join("a.txt")).unwrap(), "one\n");
        assert!(!c.ws.root.join("new.txt").exists());
        assert!(c.reads.is_empty());
        assert!(undo(&mut c).unwrap().is_empty());
    }

    #[test]
    fn refuses_write_if_file_changed_meanwhile() {
        let mut c = ctx("race", &[("a.txt", "one")]);
        let ch = Change { rel: "a.txt".into(), old: Some("one".into()), new: "two".into(), ok: "ok".into() };
        std::fs::write(c.ws.root.join("a.txt"), "user edit").unwrap();
        assert!(apply(&mut c, ch).unwrap_err().contains("changed before write"));
    }
}
