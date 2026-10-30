/// How much the agent may do without asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Confirm every write and every command.
    Ask,
    /// Writes are automatic, commands are confirmed.
    Edit,
    /// Nothing is confirmed except network access (commands stay OS-sandboxed).
    Auto,
}

pub enum Kind {
    Write,
    Exec,
    /// A command with network access: the only way data can leave the machine.
    ExecNet,
}

impl Mode {
    pub fn needs_approval(self, kind: Kind) -> bool {
        match kind {
            Kind::Write => self == Mode::Ask,
            Kind::Exec => self != Mode::Auto,
            Kind::ExecNet => true,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Mode::Ask => Mode::Edit,
            Mode::Edit => Mode::Auto,
            Mode::Auto => Mode::Ask,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ask" => Some(Mode::Ask),
            "edit" => Some(Mode::Edit),
            "auto" => Some(Mode::Auto),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::Edit => "edit",
            Mode::Auto => "auto",
        }
    }
}
