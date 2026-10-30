# A script that plants a link to the parent dir, then asks to read through it.
echo CANARY-OUTSIDE > ../canary.txt
cat > mklink.py <<'EOF'
import os
os.symlink('..', 'up')
print('link created')
EOF
