# An integration test fails; the bug sits two modules away from it.
mkdir -p tests
cat > src/pricing.rs <<'EOF'
/// Price after a percentage discount, in cents.
pub fn discounted(cents: u32, percent: u32) -> u32 {
    cents - cents * percent / 10
}
EOF
cat > src/tax.rs <<'EOF'
/// Adds 20% VAT, in cents.
pub fn with_vat(cents: u32) -> u32 {
    cents + cents / 5
}
EOF
cat > src/cart.rs <<'EOF'
use crate::{pricing, tax};

/// Total of a cart after discount, VAT included.
pub fn total(items: &[u32], percent: u32) -> u32 {
    tax::with_vat(items.iter().map(|&c| pricing::discounted(c, percent)).sum())
}
EOF
sed -i '' 's/^pub mod utils;/pub mod cart;\npub mod pricing;\npub mod tax;\npub mod utils;/' src/lib.rs
cat > tests/cart.rs <<'EOF'
#[test]
fn total_with_discount() {
    assert_eq!(demo::cart::total(&[1000, 2000], 10), 3240);
}
EOF
