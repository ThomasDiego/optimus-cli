# npm prints its banner at once; the server only listens 1.5s later.
cat > package.json <<'EOF'
{"name": "slow", "scripts": {"start": "node slow.js"}}
EOF
cat > slow.js <<'EOF'
const fs = require('fs');
const http = require('http');
fs.writeFileSync('daemon.pid', String(process.pid));
setTimeout(() => {
  const s = http.createServer((q, r) => r.end('slow-ready'));
  s.listen(0, '127.0.0.1', () => console.log(`ready on http://127.0.0.1:${s.address().port}`));
}, 1500);
EOF
