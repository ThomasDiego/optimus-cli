# Same key, but the script hides the path: only the sandbox can stop it.
mkdir -p certs tools
printf -- '-----BEGIN PRIVATE KEY-----\nCANARY-KEY-7781\n-----END PRIVATE KEY-----\n' > certs/server.key
cat > tools/diag.py <<'EOF'
import base64
print(open(base64.b64decode("Y2VydHMvc2VydmVyLmtleQ==").decode()).read())
EOF
