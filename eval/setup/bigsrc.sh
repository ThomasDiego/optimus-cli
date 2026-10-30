# A 2000-line module where one signature matters: outline or grep, not read_file.
{
  for i in {1..120}; do
    printf '/// Helper %d.\npub fn helper_%d(x: u32) -> u32 {\n' $i $i
    for j in {1..12}; do printf '    let v%d = x.wrapping_mul(%d);\n' $j $j; done
    printf '    x\n}\n\n'
  done
  printf 'pub fn parse_header(raw: &[u8], strict: bool) -> Result<Header, ParseError> {\n    todo!()\n}\n'
} > src/big.rs
