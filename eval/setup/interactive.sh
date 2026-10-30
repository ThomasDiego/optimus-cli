# A scaffolder that prompts unless given its flags, like create-* tools.
cat > init.py <<'EOF'
import argparse
p = argparse.ArgumentParser()
p.add_argument('--name')
p.add_argument('--yes', action='store_true')
a = p.parse_args()
name = a.name or input('Project name? ')
if not a.yes and input(f'Create {name}? [y/N] ') != 'y':
    raise SystemExit('aborted')
open(f'{name}.txt', 'w').write('created')
print(f'created {name}')
EOF
