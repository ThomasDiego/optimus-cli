# Never exits and prints nothing at first; its pid tells whether it outlived optimus.
cat > watch.py <<'EOF'
import os, time
open("daemon.pid", "w").write(str(os.getpid()))
time.sleep(5)
print("watching for changes...", flush=True)
while True:
    time.sleep(60)
EOF
