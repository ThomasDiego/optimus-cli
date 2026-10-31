use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::sandbox::Workspace;

/// Build output dirs: huge and never worth reporting.
const SKIP: &[&str] = &["target", "node_modules", crate::sandbox::TRASH];
const MAX_LISTED: usize = 20;
/// Past this, a huge repository would make every command wait on two full
/// walks: tracking is skipped instead.
const MAX_FILES: usize = 20_000;

pub type Snapshot = HashMap<PathBuf, (u64, SystemTime)>;

/// Size and mtime of every tracked project file (respects .gitignore);
/// `None` when the project is too large to track cheaply.
pub fn snapshot(ws: &Workspace) -> Option<Snapshot> {
    let mut walker = ws.walker(&ws.root);
    walker.filter_entry(|e| e.file_name() != ".git" && !SKIP.iter().any(|s| e.file_name() == *s));
    let snap: Snapshot = walker
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .take(MAX_FILES + 1)
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let rel = crate::os::slashes(e.path().strip_prefix(&ws.root).ok()?.to_path_buf());
            Some((rel, (meta.len(), meta.modified().ok()?)))
        })
        .collect();
    (snap.len() <= MAX_FILES).then_some(snap)
}

/// What a command did to the project: `+added ~modified -removed`.
pub fn changes(before: &Snapshot, after: &Snapshot) -> Vec<String> {
    let mut out: Vec<String> = after
        .iter()
        .filter_map(|(p, state)| match before.get(p) {
            None => Some(format!("+{}", p.display())),
            Some(old) if old != state => Some(format!("~{}", p.display())),
            Some(_) => None,
        })
        .chain(before.keys().filter(|p| !after.contains_key(*p)).map(|p| format!("-{}", p.display())))
        .collect();
    out.sort_by(|a, b| a[1..].cmp(&b[1..]));
    out
}

/// Appends the change list to a command result, for the model and the user.
pub fn annotate(output: String, changed: &[String]) -> String {
    if changed.is_empty() {
        return output;
    }
    let (head, rest) = output.split_once('\n').unwrap_or((&output, ""));
    let mut list = changed.iter().take(MAX_LISTED).cloned().collect::<Vec<_>>().join(" ");
    if changed.len() > MAX_LISTED {
        list.push_str(&format!(" +{} more", changed.len() - MAX_LISTED));
    }
    let sep = if rest.is_empty() { "" } else { "\n" };
    let s = if changed.len() == 1 { "" } else { "s" };
    format!("{head}, {} file{s} changed\n[changed] {list}{sep}{rest}", changed.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_added_modified_removed_and_skips_build_dirs() {
        let root = crate::sandbox::tests::temp("track");
        for f in ["keep.txt", "edit.txt", "gone.txt"] {
            std::fs::write(root.join(f), "x").unwrap();
        }
        let ws = Workspace::open(root.clone()).unwrap();
        let before = snapshot(&ws).unwrap();
        std::fs::write(root.join("edit.txt"), "longer").unwrap();
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        std::fs::write(root.join("new.txt"), "x").unwrap();
        std::fs::create_dir(root.join("target")).unwrap();
        std::fs::write(root.join("target/artifact"), "x").unwrap();
        let changed = changes(&before, &snapshot(&ws).unwrap());
        assert_eq!(changed, ["~edit.txt", "-gone.txt", "+new.txt"]);
        let out = annotate("exit 0 0.1s\n[stdout]\nok".into(), &changed);
        assert_eq!(out, "exit 0 0.1s, 3 files changed\n[changed] ~edit.txt -gone.txt +new.txt\n[stdout]\nok");
    }
}
