# forEach with an async callback: the sum is returned before any await resolves.
mkdir -p web
cat > web/cart.js <<'EOF'
export async function total(ids, fetchPrice) {
  let sum = 0;
  ids.forEach(async (id) => {
    sum += await fetchPrice(id);
  });
  return sum;
}
EOF
cat > web/cart.test.js <<'EOF'
import { test } from 'node:test';
import assert from 'node:assert';
import { total } from './cart.js';

test('sums prices', async () => {
  const prices = { a: 10, b: 20, c: 30 };
  const fetchPrice = (id) => new Promise((r) => setTimeout(() => r(prices[id]), 5));
  assert.strictEqual(await total(['a', 'b', 'c'], fetchPrice), 60);
});
EOF
echo '{"type":"module"}' > web/package.json
git add web && git commit -qm "add cart"
