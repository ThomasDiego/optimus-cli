use std::path::Path;

use grep_regex::RegexMatcher;
use grep_searcher::SearcherBuilder;
use grep_searcher::sinks::UTF8;

use super::file;

/// Declarations of a file with their line numbers: the shape of a module for
/// a fraction of the tokens of reading it. read_file shows them for large
/// code files read without a range.
const MAX_LINE: usize = 160;

/// Declaration lines per extension; indentation is kept, so nesting shows.
const LANGS: &[(&[&str], &str)] = &[
    (&["rs"], r"^\s*(pub(\([^)]*\))?\s+)?((async|const|unsafe|extern)\s+)*(fn|struct|enum|union|trait|impl|mod|type|static|macro_rules!)\b"),
    (
        &["js", "jsx", "ts", "tsx", "mjs", "cjs"],
        r"^\s*(export\s+(default\s+)?)?((async\s+)?function\b|class\b|interface\b|type\s+\w+|enum\b|(const|let)\s+\w+\s*=\s*(async\s*)?(\([^)]*\)|\w+)\s*=>)",
    ),
    (&["py"], r"^\s*(async\s+)?(def|class)\s"),
    (&["go"], r"^(func|type)\s"),
];

/// `line:declaration` entries, or None for an unsupported language.
pub(super) fn decls(rel: &Path, text: &str) -> Option<Vec<String>> {
    let ext = rel.extension().and_then(|e| e.to_str()).unwrap_or("");
    let (_, pattern) = LANGS.iter().find(|(exts, _)| exts.contains(&ext))?;
    let matcher = RegexMatcher::new_line_matcher(pattern).ok()?;
    let mut out = vec![];
    SearcherBuilder::new()
        .line_number(true)
        .build()
        .search_slice(&matcher, text.as_bytes(), UTF8(|ln, line| {
            let decl = line.trim_end().trim_end_matches('{').trim_end();
            out.push(format!("{ln}:{}", file::truncate(decl, MAX_LINE)));
            Ok(true)
        }))
        .ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn lists_declarations_per_language() {
        let rs = "pub mod utils;\n\n/// Adds.\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\nimpl Foo {\n    pub(crate) async fn go(&self) {}\n}\n";
        let ts = "import x from 'y';\nexport default function App() {\n}\nexport const sum = (a, b) => a + b;\nclass Store {}\n";
        let py = "import os\n\nclass A:\n    def run(self):\n        pass\n";
        let decls = |name: &str, text: &str| super::decls(Path::new(name), text).map(|d| d.join("\n"));
        let rs_out = "1:pub mod utils;\n4:pub fn add(a: i32, b: i32) -> i32\n7:impl Foo\n8:    pub(crate) async fn go(&self) {}";
        assert_eq!(decls("a.rs", rs).unwrap(), rs_out);
        assert_eq!(decls("b.tsx", ts).unwrap(), "2:export default function App()\n4:export const sum = (a, b) => a + b;\n5:class Store {}");
        assert_eq!(decls("c.py", py).unwrap(), "3:class A:\n4:    def run(self):");
        assert!(decls("d.txt", "fn x()").is_none());
    }
}
