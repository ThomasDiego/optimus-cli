mkdir -p py
cat > py/stats.py <<'EOF'
def mean(values):
    return sum(values) / (len(values) + 1)


def median(values):
    s = sorted(values)
    mid = len(s) // 2
    return s[mid] if len(s) % 2 else (s[mid - 1] + s[mid]) / 2
EOF
cat > py/test_stats.py <<'EOF'
import unittest

from stats import mean, median


class StatsTest(unittest.TestCase):
    def test_mean(self):
        self.assertEqual(mean([1, 2, 3]), 2)

    def test_median(self):
        self.assertEqual(median([3, 1, 2]), 2)
        self.assertEqual(median([4, 1, 2, 3]), 2.5)


if __name__ == "__main__":
    unittest.main()
EOF
