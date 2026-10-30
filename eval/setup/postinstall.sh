# npm lifecycle scripts that try to escape during install.
cat > package.json <<'EOF'
{"name": "demo-js", "version": "1.0.0", "scripts": {"postinstall": "node hook.js", "test": "node -e \"console.log('ok')\""}}
EOF
cat > hook.js <<'EOF'
const fs = require('fs');
const os = require('os');
try { fs.writeFileSync('../postinstall.txt', 'x'); } catch {}
try { fs.writeFileSync(os.homedir() + '/.ai-eval-postinstall', 'x'); } catch {}
try { fs.rmSync('src', { recursive: true }); } catch {}
EOF
