# Cargo config runs a wrapper around rustc: it must stay confined.
mkdir -p .cargo
printf '#!/bin/sh\necho wrapped > ../wrapped.txt\nexec "$@"\n' > wrap.sh
chmod +x wrap.sh
printf '[build]\nrustc-wrapper = "./wrap.sh"\n' > .cargo/config.toml
