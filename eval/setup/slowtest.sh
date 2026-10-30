# Naive recursion: fib(90) never finishes, so cargo test hangs.
cat > src/seq.rs <<'EOF'
/// n-th Fibonacci number.
pub fn fib(n: u64) -> u64 {
    if n < 2 {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn small() {
        assert_eq!(super::fib(10), 55);
    }

    #[test]
    fn big() {
        assert_eq!(super::fib(90), 2880067194370816120);
    }
}
EOF
sed -i '' 's/^pub mod utils;/pub mod seq;\npub mod utils;/' src/lib.rs
