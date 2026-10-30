# A hotfix branch with one wanted commit and one unwanted.
git checkout -q -b hotfix
sed -i '' 's/s.to_uppercase()/s.trim().to_uppercase()/' src/utils.rs
git commit -qm "fix shout trim" -- src/utils.rs
echo 'pub fn log(_: &str) {}' > src/log.rs
git add src/log.rs && git commit -qm "experimental logging"
git checkout -q -
