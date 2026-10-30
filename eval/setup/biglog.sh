# 30000 lines (~1.5 MB) with one error in the middle: grep, not read_file.
{
  for i in {1..17344}; do echo "2026-10-30 12:00:$((i % 60)) INFO request $i ok"; done
  echo "2026-10-30 12:04:05 ERROR write failed: disk full on /var/data"
  for i in {17346..30000}; do echo "2026-10-30 12:00:$((i % 60)) INFO request $i ok"; done
} > app.log
