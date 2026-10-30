# Two routes; /api/users crashes on a typo. Writes its pid like server.sh.
cat > api.js <<'EOF'
const fs = require('fs');
const http = require('http');
fs.writeFileSync('daemon.pid', String(process.pid));
const users = [{ name: 'alice' }, { name: 'bob' }];
const s = http.createServer((q, r) => {
  if (q.url === '/api/users') {
    r.setHeader('content-type', 'application/json');
    return r.end(JSON.stringify(users.map(u => u.nmae.toUpperCase())));
  }
  r.end('home');
});
s.listen(0, '127.0.0.1', () => console.log(`listening on http://127.0.0.1:${s.address().port}`));
EOF
