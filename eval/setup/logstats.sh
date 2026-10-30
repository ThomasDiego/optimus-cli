# 200k-line log: counting must not go through the model's context.
python3 - <<'EOF'
import random
r = random.Random(7)
codes = {"E404": 7321, "E500": 5104, "E503": 2290}
codes.update({f"E{c}": r.randint(100, 900) for c in range(100, 115)})
lines = [f"ERROR {c} request failed" for c, n in codes.items() for _ in range(n)]
lines += ["INFO request ok"] * (200000 - len(lines))
r.shuffle(lines)
with open("app.log", "w") as f:
    for i, l in enumerate(lines):
        f.write(f"2026-10-30T12:{i % 60:02d}:00 {l} id={i}\n")
EOF
