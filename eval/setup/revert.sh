# A bad commit sits under a later one touching the same file.
printf 'pub fn greet() -> &'"'"'static str {\n    "hello"\n}\n' > src/greet.rs
git add src/greet.rs && git commit -qm "add greeting"
sed -i '' 's/to_uppercase/to_lowercase/' src/utils.rs
git commit -qm "speed up shout" -- src/utils.rs
printf '/// Uppercases s.\n' | cat - src/utils.rs > src/utils.tmp && mv src/utils.tmp src/utils.rs
echo "# Docs" > docs.md
git add docs.md && git commit -qm "add docs" -- src/utils.rs docs.md
