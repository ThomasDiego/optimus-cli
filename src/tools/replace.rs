use grep_matcher::{Captures, Matcher};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use serde::Deserialize;
use serde_json::json;

use super::change::{Change, Kind};
use super::{Batch, Ctx, Outcome, Tool, file, glob, parse};
use crate::message::ToolDef;

/// The same literal change across many files in one call: renames and API
/// migrations no longer cost a read and an edit per file.
pub struct Replace;

/// Files changed per call; past it the model should narrow the glob.
const MAX_FILES: usize = 200;
const LISTED: usize = 20;

#[derive(Deserialize)]
struct Args {
    old: String,
    new: String,
    glob: Option<String>,
    #[serde(default)]
    word: bool,
    #[serde(default)]
    regex: bool,
}

impl Tool for Replace {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "replace",
            description: "Replace text in every file matching glob (default **/*): renames, API changes across files. No read needed; all or nothing; undoable.",
            schema: json!({
                "type": "object",
                "properties": {
                    "old": {"type": "string"},
                    "new": {"type": "string"},
                    "glob": {"type": "string", "description": "e.g. src/**/*.rs"},
                    "word": {"type": "boolean", "description": "whole words only (add, not address)"},
                    "regex": {"type": "boolean", "description": "old is a regex, new may use $1 or ${name}: e.g. f\\((\\w+)\\) -> f($1, 0)"}
                },
                "required": ["old", "new"]
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        if a.old.is_empty() || a.old == a.new {
            return Err("old must be non-empty and different from new; give the text to find in old and its replacement in new".into());
        }
        let re = match a.regex {
            true => Some(RegexMatcherBuilder::new().multi_line(true).word(a.word).build(&a.old).map_err(|e| format!("invalid regex: {e}"))?),
            false => None,
        };
        let (mut changes, mut total) = (vec![], 0);
        for path in glob::files(ctx, a.glob.as_deref().unwrap_or("**/*"), None)? {
            let shown = ctx.ws.display(&path);
            // Secrets, .git, binaries and huge files are simply not candidates.
            let Ok(rel) = ctx.ws.rel_write(&shown) else { continue };
            let Ok(text) = file::read_text(ctx, &rel, &shown) else { continue };
            let (updated, n) = match &re {
                Some(re) => substitute_re(re, &text, &a.new),
                None => substitute(&text, &a.old, &a.new, a.word),
            };
            if n > 0 {
                total += n;
                changes.push(Change { rel, old: Some(text.into_bytes()), new: Some(updated.into_bytes()), kind: Kind::File });
            }
        }
        if changes.is_empty() {
            return Err(format!("no match for {:?} in any file; nothing changed. Check it with grep, or narrow/widen glob", a.old));
        }
        if changes.len() > MAX_FILES {
            return Err(format!("{} files match, more than the {MAX_FILES} allowed; nothing changed. Set a narrower glob (e.g. \"src/**/*.rs\") and call again", changes.len()));
        }
        let mut names: Vec<String> = changes.iter().take(LISTED).map(|c| c.rel.display().to_string()).collect();
        if changes.len() > LISTED {
            names.push(format!("+{} more", changes.len() - LISTED));
        }
        let ok = format!("replaced {total} in {} files: {}", changes.len(), names.join(", "));
        // The model did not see these files whole: edit_file will show them first.
        Ok(Outcome::Write(Batch { changes, ok, known: false }))
    }
}

/// `text` with every match replaced, and the match count. With `word`, a
/// match touching a letter, digit or `_` on either side is left alone.
fn substitute(text: &str, old: &str, new: &str, word: bool) -> (String, usize) {
    let ident = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let (mut out, mut last, mut n) = (String::with_capacity(text.len()), 0, 0);
    for (i, _) in text.match_indices(old) {
        let end = i + old.len();
        if word && (ident(text[..i].chars().next_back()) || ident(text[end..].chars().next())) {
            continue;
        }
        out.push_str(&text[last..i]);
        out.push_str(new);
        (last, n) = (end, n + 1);
    }
    out.push_str(&text[last..]);
    (out, n)
}

/// Regex flavour of `substitute`: `$1`/`${name}` in `new` expand to groups.
fn substitute_re(re: &RegexMatcher, text: &str, new: &str) -> (String, usize) {
    let (mut out, mut n) = (Vec::with_capacity(text.len()), 0);
    let Ok(mut caps) = re.new_captures() else { return (text.into(), 0) };
    let done = re.replace_with_captures(text.as_bytes(), &mut caps, &mut out, |caps, dst| {
        caps.interpolate(|name| re.capture_index(name), text.as_bytes(), new.as_bytes(), dst);
        n += 1;
        true
    });
    match (done, String::from_utf8(out)) {
        (Ok(()), Ok(s)) => (s, n),
        _ => (text.into(), 0),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::substitute;
    use crate::tools::change;
    use crate::tools::tests::{call, ctx};

    #[test]
    fn whole_words_only_when_asked() {
        assert_eq!(substitute("add(address, add_one, add)", "add", "sum", true), ("sum(address, add_one, sum)".into(), 2));
        assert_eq!(substitute("aaa", "aa", "b", false), ("ba".into(), 1));
    }

    #[test]
    fn regex_expands_groups() {
        let mut c = ctx("replace-re", &[("h1.rs", "shout(\"h1\")\n"), ("h2.rs", "shout(\"h2\") + shout(x)\n")]);
        let ok = call(&mut c, "replace", json!({"old": r#"shout\(("h\d+")\)"#, "new": "shout($1, 1)", "regex": true})).unwrap();
        assert_eq!(ok, "replaced 2 in 2 files: h1.rs, h2.rs");
        assert_eq!(std::fs::read_to_string(c.ws.root.join("h2.rs")).unwrap(), "shout(\"h2\", 1) + shout(x)\n");
        assert!(call(&mut c, "replace", json!({"old": "(", "new": "x", "regex": true})).unwrap_err().contains("invalid regex"));
    }

    #[test]
    fn replaces_across_files_atomically_and_undoably() {
        let files = [("src/a.rs", "use x::shout;\nshout()"), ("src/b.rs", "shout"), ("README.md", "shout"), (".env", "shout=1")];
        let mut c = ctx("replace", &files);
        change::begin_turn(&mut c);
        let ok = call(&mut c, "replace", json!({"old": "shout", "new": "yell", "glob": "src/**/*.rs"})).unwrap();
        assert_eq!(ok, "replaced 3 in 2 files: src/a.rs, src/b.rs");
        let root = c.ws.root.clone();
        let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap();
        assert_eq!((read("src/a.rs"), read("README.md"), read(".env")), ("use x::yell;\nyell()".into(), "shout".into(), "shout=1".into()));

        // Secrets are never candidates, even with the default glob.
        call(&mut c, "replace", json!({"old": "shout", "new": "x"})).unwrap();
        assert_eq!(read(".env"), "shout=1");
        assert!(call(&mut c, "replace", json!({"old": "nothing", "new": "x"})).unwrap_err().starts_with("no match"));

        change::begin_turn(&mut c);
        change::undo(&mut c).unwrap();
        assert_eq!(read("README.md"), "shout");
    }
}
