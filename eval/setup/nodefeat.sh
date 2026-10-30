mkdir -p web
cat > web/package.json <<'EOF'
{"name": "web", "type": "module", "scripts": {"test": "node --test"}}
EOF
cat > web/slug.js <<'EOF'
export function slugify(text) {
  return text.trim().toLowerCase().replace(/\s+/g, '-');
}
EOF
cat > web/slug.test.js <<'EOF'
import test from 'node:test';
import assert from 'node:assert';
import { slugify } from './slug.js';

test('slugify', () => {
  assert.equal(slugify('  Hello World '), 'hello-world');
});
EOF
