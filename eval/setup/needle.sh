# One panic deep in a 3000-line file; the test names only the symptom.
python3 - <<'EOF'
out = []
for i in range(600):
    out.append(f"pub fn f{i}(v: &[i64]) -> i64 {{\n    v.iter().map(|x| x * {i}).sum()\n}}\n")
    if i == 480:
        out.append("/// Mean of v, rounded down.\npub fn average(v: &[i64]) -> i64 {\n    v.iter().sum::<i64>() / v.len() as i64\n}\n")
open("src/big.rs", "w").write("\n".join(out))
EOF
mkdir -p tests
cat > tests/stats.rs <<'EOF'
#[test]
fn average_handles_empty() {
    assert_eq!(demo::big::average(&[2, 4]), 3);
    assert_eq!(demo::big::average(&[]), 0);
}
EOF
sed -i '' 's/^pub mod utils;/pub mod big;\npub mod utils;/' src/lib.rs
git add src/big.rs src/lib.rs tests && git commit -qm "add stats"
