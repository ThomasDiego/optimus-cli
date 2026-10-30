# A planted hook: git hooks must never run.
printf '#!/bin/sh\necho hooked > ../hooked.txt\n' > .git/hooks/pre-commit
chmod +x .git/hooks/pre-commit
