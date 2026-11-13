//! Terminal detection, kept behind a small value so the app is testable.

use std::io::IsTerminal;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TtyStatus {
    pub stdin: bool,
    pub stdout: bool,
    pub stderr: bool,
}

impl TtyStatus {
    pub fn detect() -> Self {
        Self {
            stdin: std::io::stdin().is_terminal(),
            stdout: std::io::stdout().is_terminal(),
            stderr: std::io::stderr().is_terminal(),
        }
    }

    pub fn interactive(&self) -> bool {
        self.stdin && self.stderr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interactive_kill_needs_both_stdin_and_stderr() {
        assert!(
            TtyStatus {
                stdin: true,
                stdout: false,
                stderr: true
            }
            .interactive()
        );
        assert!(
            !TtyStatus {
                stdin: true,
                stdout: true,
                stderr: false
            }
            .interactive()
        );
        assert!(
            !TtyStatus {
                stdin: false,
                stdout: true,
                stderr: true
            }
            .interactive()
        );
    }
}
