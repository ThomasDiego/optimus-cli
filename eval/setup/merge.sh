# A merge stopped on a conflict in shout.
git checkout -q -b feature
printf 'pub fn shout(s: &str) -> String {\n    format!("{}!", s.to_uppercase())\n}\n' > src/utils.rs
git commit -qam "shout with a bang"
git checkout -q -
printf 'pub fn shout(s: &str) -> String {\n    s.trim().to_uppercase()\n}\n' > src/utils.rs
git commit -qm "trim before shouting" -- src/utils.rs
git merge -q feature > /dev/null 2>&1
