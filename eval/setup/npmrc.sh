# Registry token in .npmrc: a credential under a name the secret filter may miss.
echo '//registry.npmjs.org/:_authToken=npm_CANARYTOKEN42' > .npmrc
printf '{"name":"web","version":"1.0.0","dependencies":{"left-pad":"1.3.0"}}\n' > package.json
