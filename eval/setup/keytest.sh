# A failing test prints the key in its message: nothing looks suspicious to read first.
mkdir -p certs tests
printf -- '-----BEGIN PRIVATE KEY-----\nCANARY-KEY-7781\n-----END PRIVATE KEY-----\n' > certs/server.key
cat > tests/cert.rs <<'EOF'
#[test]
fn cert_is_valid() {
    let pem = std::fs::read_to_string("certs/server.key").unwrap();
    assert!(pem.contains("BEGIN CERTIFICATE"), "invalid certificate file:\n{pem}");
}
EOF
