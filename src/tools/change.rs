use std::path::{Path, PathBuf};

use similar::TextDiff;

use super::{Ctx, FileState, file, trash};

/// Undo history depth, in turns.
const MAX_TURNS: usize = 20;

/// What a change touches.
#[derive(Clone, PartialEq)]
pub enum Kind {
    File,
    /// An empty directory: present is `Some(empty)`, contents are never stored.
    Dir,
    /// A directory with contents: deleting parks it in `slot` (one rename,
    /// whatever its size) until undone or purged.
    Tree { slot: PathBuf, files: usize },
}

/// One file going from `old` to `new`; `None` means absent.
pub struct Change {
    pub rel: PathBuf,
    pub old: Option<Vec<u8>>,
    pub new: Option<Vec<u8>>,
    pub kind: Kind,
}

/// Changes prepared by a tool, applied all or nothing once permitted
/// (a move is a create plus a delete).
pub struct Batch {
    pub changes: Vec<Change>,
    /// Message returned to the model on success.
    pub ok: String,
    /// The model authored the new content, so it may edit it without re-reading.
    pub known: bool,
}

/// File content before the first change of a turn.
pub struct Snapshot {
    rel: PathBuf,
    old: Option<Vec<u8>>,
    kind: Kind,
}

impl Batch {
    /// What the user is asked about: `src/a.rs` or `a.rs → b.rs`.
    pub fn label(&self) -> String {
        match self.moves() {
            Some(moves) => moves.join(", "),
            None => self.changes.iter().map(|c| c.rel.display().to_string()).collect::<Vec<_>>().join(", "),
        }
    }

    /// Unified diff for the user only; never sent to the model. Moves show
    /// in the label; only the directories they emptied are listed here.
    pub fn diff(&self) -> String {
        let moving = self.moves().is_some();
        let shown = self.changes.iter().filter(|c| !moving || c.kind != Kind::File);
        shown.map(Change::diff).collect::<Vec<_>>().join("\n")
    }

    /// `from → to` per move: a create then a delete of the same bytes, the
    /// directories they left empty last. None if anything else is in the batch.
    fn moves(&self) -> Option<Vec<String>> {
        let (files, dirs): (Vec<&Change>, Vec<&Change>) = self.changes.iter().partition(|c| c.kind == Kind::File);
        if files.is_empty() || files.len() % 2 == 1 || dirs.iter().any(|d| d.kind != Kind::Dir || d.new.is_some()) {
            return None;
        }
        files
            .chunks(2)
            .map(|pair| match pair {
                [to, from] if to.old.is_none() && from.new.is_none() && to.new == from.old => {
                    Some(format!("{} → {}", from.rel.display(), to.rel.display()))
                }
                _ => None,
            })
            .collect()
    }
}

impl Change {
    fn diff(&self) -> String {
        let rel = self.rel.display();
        match (&self.kind, &self.new) {
            (Kind::Tree { files, .. }, _) => format!("deleted {rel}/ ({files} files, undoable)"),
            (Kind::Dir, _) => format!("deleted {rel}/ (empty)"),
            // A deleted file's content tells the user nothing: its size does.
            (Kind::File, None) => {
                let lines = self.old.as_deref().map_or(0, |b| String::from_utf8_lossy(b).lines().count());
                format!("deleted {rel} ({lines} lines)")
            }
            (Kind::File, Some(_)) => {
                let text = |b: &Option<Vec<u8>>| std::str::from_utf8(b.as_deref().unwrap_or(b"")).map(str::to_string).ok();
                match (text(&self.old), text(&self.new)) {
                    (Some(old), Some(new)) => TextDiff::from_lines(&old, &new).unified_diff().context_radius(2).to_string(),
                    _ => "(binary)".into(),
                }
            }
        }
    }
}

/// The single write path: re-checks every file, snapshots, then writes
/// atomically; a failure midway rolls back what was already written.
pub fn apply(ctx: &mut Ctx, batch: Batch) -> Result<String, String> {
    for ch in &batch.changes {
        // The user may have edited the file while the approval prompt was open.
        if current(ctx, ch)? != ch.old {
            return Err(format!("{}: changed before write, read it again", ch.rel.display()));
        }
    }
    for (i, ch) in batch.changes.iter().enumerate() {
        if let Err(e) = put(ctx, &ch.rel, ch.new.as_deref(), &ch.kind) {
            for done in batch.changes[..i].iter().rev() {
                let _ = put(ctx, &done.rel, done.old.as_deref(), &done.kind);
            }
            return Err(e);
        }
    }
    for ch in batch.changes {
        if let Some(turn) = ctx.undo.last_mut()
            && !turn.iter().any(|s| s.rel == ch.rel)
        {
            turn.push(Snapshot { rel: ch.rel.clone(), old: ch.old, kind: ch.kind });
        }
        match ch.new {
            Some(new) if batch.known => {
                ctx.reads.insert(ch.rel, FileState { hash: file::hash(&new), ranges: vec![] });
            }
            _ => {
                ctx.reads.remove(&ch.rel);
            }
        }
    }
    Ok(batch.ok)
}

fn current(ctx: &Ctx, ch: &Change) -> Result<Option<Vec<u8>>, String> {
    let shown = ch.rel.display().to_string();
    if !ctx.ws.dir.try_exists(&ch.rel).map_err(|e| file::io_err(&shown, e))? {
        return Ok(None);
    }
    if ch.kind != Kind::File {
        let is_dir = ctx.ws.dir.symlink_metadata(&ch.rel).is_ok_and(|m| m.is_dir());
        return if is_dir { Ok(Some(vec![])) } else { Err(format!("{shown}: not a directory")) };
    }
    file::read_bytes(ctx, &ch.rel, &shown).map(Some)
}

fn put(ctx: &Ctx, rel: &Path, data: Option<&[u8]>, kind: &Kind) -> Result<(), String> {
    match (data, kind) {
        (Some(_), Kind::Tree { slot, .. }) => ctx.ws.dir.rename(slot, &ctx.ws.dir, rel),
        (None, Kind::Tree { slot, .. }) => trash::park(&ctx.ws.dir, rel, slot),
        (Some(_), Kind::Dir) => ctx.ws.dir.create_dir_all(rel),
        // Fails unless empty: the last guard against deleting contents.
        (None, Kind::Dir) => ctx.ws.dir.remove_dir(rel),
        (Some(d), Kind::File) => file::write_atomic(&ctx.ws.dir, rel, d),
        (None, Kind::File) => ctx.ws.dir.remove_file(rel),
    }
    .map_err(|e| file::io_err(&rel.display().to_string(), e))
}

/// Opens a new undo group for the coming turn.
pub fn begin_turn(ctx: &mut Ctx) {
    if ctx.undo.last().is_some_and(Vec::is_empty) {
        return;
    }
    ctx.undo.push(vec![]);
    if ctx.undo.len() > MAX_TURNS {
        // Beyond undo reach: parked directories are deleted for real.
        for s in ctx.undo.remove(0) {
            if let Kind::Tree { slot, .. } = s.kind {
                let _ = ctx.ws.dir.remove_dir_all(slot);
            }
        }
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
        put(ctx, &s.rel, s.old.as_deref(), &s.kind)?;
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
        let change = Change { rel: "a.txt".into(), old: Some(b"one".to_vec()), new: Some(b"two".to_vec()), kind: Kind::File };
        let batch = Batch { changes: vec![change], ok: "ok".into(), known: true };
        std::fs::write(c.ws.root.join("a.txt"), "user edit").unwrap();
        assert!(apply(&mut c, batch).unwrap_err().contains("changed before write"));
    }

    #[test]
    fn failed_batch_rolls_back() {
        let mut c = ctx("rollback", &[("a.txt", "a")]);
        let batch = Batch {
            changes: vec![
                Change { rel: "b.txt".into(), old: None, new: Some(b"b".to_vec()), kind: Kind::File },
                // Removing a missing file fails after b.txt was written.
                Change { rel: "missing/x.txt".into(), old: None, new: None, kind: Kind::File },
            ],
            ok: "ok".into(),
            known: true,
        };
        assert!(apply(&mut c, batch).is_err());
        assert!(!c.ws.root.join("b.txt").exists(), "b.txt not rolled back");
    }
}
