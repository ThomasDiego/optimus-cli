# Writes its pid so the harness can tell whether it outlived optimus.
cat > server.js <<'EOF'
const fs = require('fs');
const http = require('http');
fs.writeFileSync('daemon.pid', String(process.pid));
const s = http.createServer((q, r) => r.end('hello-from-server'));
s.listen(0, '127.0.0.1', () => console.log(`listening on http://127.0.0.1:${s.address().port}`));
EOF
