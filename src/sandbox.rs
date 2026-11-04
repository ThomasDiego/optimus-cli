use std::io;
use std::path::{Component, Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::Dir;
use ignore::WalkBuilder;

/// Where deleted directories wait for undo, inside the project.
pub const TRASH: &str = ".optimus-trash";

/// The launch directory. All file I/O goes through `dir`, a capability handle
/// that cannot resolve paths outside it (no `..`, absolute or symlink escape).
pub struct Workspace {
    pub dir: Dir,
    /// Canonical path of the launch directory.
    pub root: PathBuf,
}

impl Workspace {
    pub fn open(root: PathBuf) -> io::Result<Self> {
        let root = crate::os::plain_path(root.canonicalize()?);
        let dir = Dir::open_ambient_dir(&root, ambient_authority())?;
        Ok(Self { dir, root })
    }

    /// Turns a model-supplied path into a root-relative one, refusing secrets.
    pub fn rel(&self, path: &str) -> Result<PathBuf, String> {
        let p = Path::new(path);
        let p = if p.is_absolute() {
            p.strip_prefix(&self.root).map_err(|_| format!("{path}: outside the project; use a path relative to the project root"))?
        } else {
            p
        };
        // `..` is resolved lexically: climbing above the root is refused even
        // through directories that do not exist.
        let mut rel = PathBuf::new();
        for c in p.components() {
            match c {
                Component::ParentDir if !rel.pop() => return Err(format!("{path}: outside the project (too many ..); use a path relative to the project root")),
                Component::Normal(n) => rel.push(n),
                _ => {}
            }
        }
        if rel.as_os_str().is_empty() {
            rel.push(".");
        }
        let rel = crate::os::slashes(rel);
        if is_secret(&rel) || self.real(&rel).is_some_and(|r| is_secret(&r)) {
            return Err(format!("{path}: secret file, access denied; do not retry or look for another way in, tell the user if you need it"));
        }
        // cap-std would refuse later anyway; a precise reason here saves the
        // model from probing what "permission denied" means.
        let existing = rel.ancestors().find_map(|a| self.real(a));
        if existing.is_some_and(|r| !r.starts_with(&self.root)) {
            return Err(format!("{path}: symlink to outside the project, not reachable"));
        }
        Ok(rel)
    }

    /// Like `rel`, but `.git/` is only writable through git tools, and the
    /// trash only through undo.
    pub fn rel_write(&self, path: &str) -> Result<PathBuf, String> {
        let rel = self.rel(path)?;
        for guarded in [".git", TRASH] {
            let inside = |p: &Path| p.components().any(|c| c.as_os_str() == guarded);
            // The deepest existing ancestor, canonical: its real case and links.
            let real = rel.ancestors().find_map(|a| self.real(a));
            let real_inside = real.is_some_and(|r| inside(r.strip_prefix(&self.root).unwrap_or(&r)));
            if inside(&rel) || real_inside {
                return Err(format!("{path}: {guarded} is read-only for file tools; do not retry{}", if guarded == ".git" { " (use the git tool for repository changes)" } else { "" }));
            }
        }
        Ok(rel)
    }

    /// Absolute, canonical base for directory walks, guaranteed inside root.
    pub fn walk_base(&self, path: Option<&str>) -> Result<PathBuf, String> {
        let shown = path.unwrap_or(".");
        let rel = self.rel(shown)?;
        let abs = self.real(&rel).ok_or_else(|| format!("{shown}: directory not found; check the name with list_dir or glob"))?;
        if !abs.starts_with(&self.root) {
            return Err(format!("{shown}: outside the project; use a path relative to the project root"));
        }
        Ok(abs)
    }

    /// Walker that never follows symlinks and skips `.git` and the trash.
    pub fn walker(&self, base: &Path) -> WalkBuilder {
        let mut w = WalkBuilder::new(base);
        w.hidden(false)
            .follow_links(false)
            .require_git(false)
            .sort_by_file_name(|a, b| a.cmp(b))
            .filter_entry(|e| e.file_name() != ".git" && e.file_name() != TRASH);
        w
    }

    /// Root-relative display path, shortest form for the model.
    pub fn display(&self, abs: &Path) -> String {
        crate::os::slashes(abs.strip_prefix(&self.root).unwrap_or(abs).to_path_buf()).to_string_lossy().into_owned()
    }

    fn real(&self, rel: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(self.root.join(rel)).ok().map(crate::os::plain_path)
    }
}

/// Files whose content must never reach the LLM provider. Mirrored in
/// seatbelt.sb and git's SECRET_EXCLUDES: keep the three in sync.
pub fn is_secret(p: &Path) -> bool {
    let Some(name) = p.file_name().and_then(|n| n.to_str()) else { return false };
    let env = name == ".env"
        || (name.starts_with(".env.") && !matches!(name, ".env.example" | ".env.sample" | ".env.template" | ".env.testing"));
    env || [".pem", ".key", ".p12", ".pfx"].iter().any(|e| name.ends_with(e))
        || ["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"].iter().any(|k| name.starts_with(k))
        || matches!(name, ".npmrc" | ".pypirc" | ".netrc" | ".git-credentials" | "auth.json")
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Fresh directory under the system temp dir.
    pub fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("ai-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[cfg(unix)]
    #[test]
    fn cannot_escape_root() {
        let base = temp("escape");
        std::fs::create_dir(base.join("proj")).unwrap();
        std::fs::write(base.join("outside.txt"), "x").unwrap();
        std::os::unix::fs::symlink(base.join("outside.txt"), base.join("proj/link")).unwrap();
        let ws = Workspace::open(base.join("proj")).unwrap();

        assert!(ws.rel("/etc/passwd").is_err());
        for p in ["../outside.txt", "link", "a/../../outside.txt"] {
            assert!(ws.rel(p).is_err(), "{p} accepted");
            // Second wall: the capability handle refuses even unchecked paths.
            assert!(ws.dir.read(p).is_err(), "{p} escaped");
            assert!(ws.dir.write(p, "y").is_err(), "{p} wrote outside");
        }
        assert_eq!(std::fs::read_to_string(base.join("outside.txt")).unwrap(), "x");
        assert!(ws.walk_base(Some("..")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn secrets_and_git_are_protected() {
        let base = temp("secret");
        std::fs::write(base.join(".env"), "KEY=1").unwrap();
        std::os::unix::fs::symlink(base.join(".env"), base.join("innocent")).unwrap();
        let ws = Workspace::open(base).unwrap();
        assert!(ws.rel(".env").is_err());
        assert!(ws.rel("innocent").is_err());
        assert!(ws.rel(".env.example").is_ok());
        assert!(ws.rel(".env.testing").is_ok());
        for p in [".npmrc", "certs/a.p12", "id_rsa.pub", ".git-credentials", "auth.json"] {
            assert!(ws.rel(p).is_err(), "{p} readable");
        }
        assert!(ws.rel("keys.rs").is_ok());
        assert!(ws.rel_write(".git/config").is_err());
        assert!(ws.rel_write("./src/main.rs").is_ok());
    }

    /// Case-insensitive filesystems (macOS, Windows) open `.GIT` as `.git`.
    #[test]
    fn git_guard_ignores_case() {
        let base = temp("case");
        std::fs::create_dir_all(base.join(".git/hooks")).unwrap();
        let ws = Workspace::open(base.clone()).unwrap();
        if base.join(".GIT").exists() {
            assert!(ws.rel_write(".GIT/hooks/pre-commit").is_err());
            assert!(ws.rel_write(".Git/config").is_err());
        }
    }
}
