# 20000 rows; value = id % 10, so the sum is 90000.
{ echo id,value; for i in {1..20000}; do echo "$i,$((i % 10))"; done } > data.csv
