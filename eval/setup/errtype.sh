# parse_age panics; making it return Result ripples into parse_profile and main.
cat > src/age.rs <<'EOF'
/// Parses an age in years.
pub fn parse_age(s: &str) -> u8 {
    let n: u32 = s.trim().parse().unwrap();
    if n > 150 {
        panic!("too old");
    }
    n as u8
}
EOF
cat > src/profile.rs <<'EOF'
use crate::age::parse_age;

pub struct Profile {
    pub name: String,
    pub age: u8,
}

/// Parses "name,age".
pub fn parse_profile(line: &str) -> Option<Profile> {
    let (name, age) = line.split_once(',')?;
    Some(Profile { name: name.trim().to_string(), age: parse_age(age) })
}
EOF
sed -i '' 's/^pub mod utils;/pub mod age;\npub mod profile;\npub mod utils;/' src/lib.rs
sed -i '' 's/^fn main() {/fn main() {\n    println!("{}", demo::age::parse_age("30"));/' src/main.rs
