use std::io::{Write, stdout};

/// Copies to the system clipboard. OSC 52 asks the terminal to do it (works
/// over ssh and in tmux); macOS Terminal ignores it, hence pbcopy there.
pub fn copy(text: &str) {
    let mut out = stdout();
    let _ = write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes())).and_then(|_| out.flush());
    #[cfg(target_os = "macos")]
    pbcopy(text);
}

#[cfg(target_os = "macos")]
fn pbcopy(text: &str) {
    use std::process::{Command, Stdio};
    // Fixed path, and UTF-8 forced: accents would be mangled under a bare locale.
    let spawned = Command::new("/usr/bin/pbcopy")
        .env("LANG", "en_US.UTF-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else { return };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    let _ = child.wait();
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { TABLE[(n >> (18 - 6 * i)) as usize & 63] as char } else { '=' });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn encodes_base64_with_padding() {
        let cases = [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy"), ("é", "w6k=")];
        for (text, want) in cases {
            assert_eq!(super::base64(text.as_bytes()), want, "{text:?}");
        }
    }
}
