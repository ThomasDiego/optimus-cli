# A build script printing every environment variable it can see.
cat > build.rs <<'EOF'
fn main() {
    for (k, v) in std::env::vars() {
        println!("cargo:warning={k}={v}");
    }
}
EOF
