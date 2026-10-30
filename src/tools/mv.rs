use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::json;

use super::change::{Change, Kind};
use super::{Batch, Ctx, Outcome, Tool, file, parse};
use crate::message::ToolDef;

/// Moves or renames files, several at once; destinations must not exist.
pub struct Move;

const MAX_MOVES: usize = 50;

#[derive(Deserialize)]
struct Args {
    from: Option<String>,
    to: Option<String>,
    #[serde(default)]
    moves: Vec<Pair>,
}

#[derive(Deserialize)]
struct Pair {
    from: String,
    to: String,
}

impl Tool for Move {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "move_file",
            description: "Move or rename files; parent dirs are created, dirs left empty are removed. Fails if a destination exists.",
            schema: json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string"},
                    "to": {"type": "string"},
                    "moves": {
                        "type": "array",
                        "description": "several at once",
                        "items": {"type": "object", "properties": {"from": {"type": "string"}, "to": {"type": "string"}}, "required": ["from", "to"]}
                    }
                }
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        let single = a.from.zip(a.to).map(|(from, to)| Pair { from, to });
        let pairs: Vec<Pair> = single.into_iter().chain(a.moves).collect();
        if pairs.is_empty() || pairs.len() > MAX_MOVES {
            return Err(format!("give from and to, or 1 to {MAX_MOVES} moves"));
        }
        let (mut changes, mut gone, mut dests, mut done) = (vec![], HashSet::new(), vec![], vec![]);
        for p in &pairs {
            let (from, to) = (ctx.ws.rel_write(&p.from)?, ctx.ws.rel_write(&p.to)?);
            if ctx.ws.dir.try_exists(&to).map_err(|e| file::io_err(&p.to, e))? {
                return Err(format!("{}: exists; delete it first or pick another name", p.to));
            }
            if gone.contains(&from) || dests.contains(&to) || gone.contains(&to) || dests.contains(&from) {
                return Err(format!("{} → {}: each path may appear once", p.from, p.to));
            }
            let bytes = file::read_bytes(ctx, &from, &p.from)?;
            changes.push(Change { rel: to.clone(), old: None, new: Some(bytes.clone()), kind: Kind::File });
            changes.push(Change { rel: from.clone(), old: Some(bytes), new: None, kind: Kind::File });
            done.push(format!("{} → {}", p.from, p.to));
            gone.insert(from);
            dests.push(to);
        }
        let emptied = emptied(ctx, &mut gone, &dests);
        let mut ok = format!("moved {}", done.join(", "));
        if !emptied.is_empty() {
            let shown: Vec<String> = emptied.iter().map(|d| format!("{}/", d.display())).collect();
            ok.push_str(&format!("; removed empty {}", shown.join(", ")));
        }
        changes.extend(emptied.into_iter().map(|rel| Change { rel, old: Some(vec![]), new: None, kind: Kind::Dir }));
        // The model did not author this content: it must read before editing.
        Ok(Outcome::Write(Batch { changes, ok, known: false }))
    }
}

/// Like git mv, the directories the moves leave empty go too (undoable),
/// deepest first; never a destination's ancestor nor the root.
fn emptied(ctx: &Ctx, gone: &mut HashSet<PathBuf>, dests: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = gone.iter().flat_map(|p| p.ancestors().skip(1).map(Path::to_path_buf)).collect();
    dirs.retain(|d| !d.as_os_str().is_empty() && !dests.iter().any(|t| t.starts_with(d)));
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    let mut out = vec![];
    for d in dirs {
        let Ok(entries) = ctx.ws.dir.read_dir(&d) else { continue };
        if entries.into_iter().all(|e| e.is_ok_and(|e| gone.contains(&d.join(e.file_name())))) {
            gone.insert(d.clone());
            out.push(d);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::tools::change;
    use crate::tools::tests::{call, ctx};

    #[test]
    fn moves_several_and_removes_what_they_empty() {
        let files = [("src/lib.rs", "l"), ("src/main.rs", "m"), ("src/sub/u.rs", "u"), ("keep/a.rs", "a"), ("keep/b.rs", "b")];
        let mut c = ctx("mv-many", &files);
        let root = c.ws.root.clone();
        change::begin_turn(&mut c);
        let moves = json!({"moves": [
            {"from": "src/lib.rs", "to": "core/src/lib.rs"},
            {"from": "src/sub/u.rs", "to": "core/src/u.rs"},
            {"from": "src/main.rs", "to": "cli/src/main.rs"},
            {"from": "keep/a.rs", "to": "a.rs"}
        ]});
        let ok = call(&mut c, "move_file", moves).unwrap();
        assert!(ok.ends_with("; removed empty src/sub/, src/"), "{ok}");
        assert!(!root.join("src").exists() && root.join("keep/b.rs").exists());
        assert_eq!(std::fs::read_to_string(root.join("core/src/u.rs")).unwrap(), "u");

        change::begin_turn(&mut c);
        change::undo(&mut c).unwrap();
        assert!(root.join("src/sub/u.rs").exists() && root.join("keep/a.rs").exists() && !root.join("core/src/u.rs").exists());
    }

    #[test]
    fn refuses_conflicting_moves_atomically() {
        let mut c = ctx("mv-guard", &[("a.rs", "a"), ("b.rs", "b")]);
        let twice = json!({"moves": [{"from": "a.rs", "to": "x.rs"}, {"from": "b.rs", "to": "x.rs"}]});
        assert!(call(&mut c, "move_file", twice).unwrap_err().contains("once"));
        let chain = json!({"moves": [{"from": "a.rs", "to": "c.rs"}, {"from": "c.rs", "to": "d.rs"}]});
        assert!(call(&mut c, "move_file", chain).is_err());
        assert!(call(&mut c, "move_file", json!({"moves": [{"from": "a.rs", "to": "../x"}]})).is_err());
        assert!(call(&mut c, "move_file", json!({})).is_err());
        assert!(c.ws.root.join("a.rs").exists() && !c.ws.root.join("x.rs").exists());
    }
}
