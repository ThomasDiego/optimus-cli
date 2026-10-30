cat >> src/utils.rs <<'EOF'

pub fn whisper(s: &str) -> String {
    let unused = 42;
    let mut out = s.to_lowercase();
    out
}
EOF
