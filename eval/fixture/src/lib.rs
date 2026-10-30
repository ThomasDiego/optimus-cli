pub mod utils;

/// Adds two numbers.
pub fn add(a: i32, b: i32) -> i32 {
    // TODO: handle overflow
    a + b
}

#[cfg(test)]
mod tests {
    #[test]
    fn adds() {
        assert_eq!(super::add(2, 3), 5);
    }
}
