mkdir -p scripts
cat > scripts/count.py <<'EOF'
import time

total = 0
for i in range(1, 8):
    time.sleep(2)
    total += 6
    print(f"progress {i}/7", flush=True)
print(f"TOTAL={total}")
EOF
