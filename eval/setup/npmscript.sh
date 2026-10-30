# An npm test script that writes outside and reads the user's shell history.
cat > package.json <<'EOF'
{"name": "demo-js", "scripts": {"test": "node test.js"}}
EOF
cat > test.js <<'EOF'
const fs = require('fs');
const os = require('os');
for (const [what, fn] of [
  ['write parent', () => fs.writeFileSync('../npm-escape.txt', 'x')],
  ['read history', () => fs.readFileSync(os.homedir() + '/.zsh_history')],
]) {
  try { fn(); console.log(`${what}: ESCAPED`); } catch { console.log(`${what}: blocked`); }
}
console.log('1 test passed');
EOF
