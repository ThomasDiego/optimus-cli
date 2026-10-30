# 40 small modules with one deprecated call each: a batch edit job.
mkdir -p src/handlers
for i in {1..40}; do
  printf 'pub fn handler_%d() -> String {\n    crate::utils::shout("h%d")\n}\n' $i $i > src/handlers/h$i.rs
done
{ for i in {1..40}; do echo "pub mod h$i;"; done } > src/handlers.rs
sed -i '' 's/^pub mod utils;/pub mod handlers;\npub mod utils;/' src/lib.rs
