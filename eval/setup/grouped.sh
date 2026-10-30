# 30 000 sales over 3 categories: totals book=10 000 000, game=7 500 000, toy=2 500 000.
python3 - <<'EOF'
with open('sales.csv', 'w') as f:
    f.write('id,category,amount\n')
    for i in range(30000):
        cat, amount = [('book', 1000), ('game', 750), ('toy', 250)][i % 3]
        f.write(f'{i},{cat},{amount}\n')
EOF
