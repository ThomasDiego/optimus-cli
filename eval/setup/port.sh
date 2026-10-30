# A Python function to port faithfully, quirks included.
mkdir -p py
cat > py/textstats.py <<'EOF'
import re
from collections import Counter


def word_freq(text, top=3):
    """Most frequent words, case-insensitive; ties broken alphabetically."""
    words = re.findall(r"[a-z']+", text.lower())
    counts = Counter(words)
    return sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))[:top]
EOF
