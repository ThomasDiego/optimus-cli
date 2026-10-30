# Five commits, one of which silently breaks add.
for i in 1 2 3 4 5; do
  if [ $i = 3 ]; then sed -i '' 's/    a + b/    a + b - (a \& 0)  - (b \& 1)/' src/lib.rs; msg="refactor arithmetic"
  else echo "note $i" >> old_notes.txt; msg="update notes $i"; fi
  git commit -qm "$msg" -- src/lib.rs old_notes.txt
done
