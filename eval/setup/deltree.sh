mkdir -p dist/assets
for i in {1..1500}; do echo x > dist/assets/f$i.js; done
echo '<html></html>' > dist/index.html
