use std::ops::AddAssign;

use serde_json::Value;

/// Provider-neutral conversation message.
#[derive(Clone, Debug)]
pub enum Message {
    User(String),
    Assistant { text: String, calls: Vec<ToolCall> },
    Tool { id: String, content: String },
}

#[derive(Clone, Debug, Default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON arguments as streamed by the model.
    pub args: String,
}

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub cache_write: u64,
    pub cost: f64,
}

/// Dollars with two significant digits, so sub-cent costs stay visible.
pub fn money(dollars: f64) -> String {
    if dollars <= 0.0 {
        return "$0".into();
    }
    let decimals = (1 - dollars.log10().floor() as i32).clamp(2, 10) as usize;
    format!("${dollars:.decimals$}")
}

/// `950`, `3.2k`, `1.4M`.
pub fn kilo(n: u64) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

impl AddAssign for Usage {
    fn add_assign(&mut self, o: Self) {
        self.input += o.input;
        self.output += o.output;
        self.cached += o.cached;
        self.cache_write += o.cache_write;
        self.cost += o.cost;
    }
}

#[derive(Debug, Default)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub usage: Usage,
}

#[cfg(test)]
mod tests {
    use super::{kilo, money};

    #[test]
    fn formats_money_and_counts() {
        assert_eq!(money(0.0000072), "$0.0000072");
        assert_eq!(money(0.0016), "$0.0016");
        assert_eq!(money(0.25), "$0.25");
        assert_eq!(money(12.5), "$12.50");
        assert_eq!(money(0.0), "$0");
        assert_eq!((kilo(950), kilo(3200), kilo(1_400_000)), ("950".into(), "3.2k".into(), "1.4M".into()));
    }
}
