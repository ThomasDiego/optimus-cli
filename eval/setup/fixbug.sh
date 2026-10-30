cat >> src/lib.rs <<'EOF'

/// True for even numbers.
pub fn is_even(n: i32) -> bool {
    n % 2 == 1
}

#[cfg(test)]
mod parity {
    #[test]
    fn even() {
        assert!(super::is_even(4));
        assert!(!super::is_even(7));
    }
}
EOF
