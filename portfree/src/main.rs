use std::io::{self, Write};

use clap::{CommandFactory, FromArgMatches};

use portfree::app::{self, AppDeps};
use portfree::cli::Cli;
use portfree::clock::RealClock;
use portfree::console::{self, ConsolePhase};
use portfree::model::{Envelope, ErrorCode};
use portfree::output::{self, ColorConfig, Streams};
use portfree::ports::NativePortTable;
use portfree::processes::SysinfoProcessTable;
use portfree::system::WindowsSystem;
use portfree::tty::TtyStatus;

fn main() {
    let no_color_requested = no_color_requested();
    let cli = parse_cli(no_color_requested);

    console::install();
    let tty = TtyStatus::detect();
    if tty.stdout || tty.stderr {
        console::enable_virtual_terminal();
    }
    let no_color = cli.no_color || no_color_requested;
    let color = ColorConfig {
        stdout: !cli.json && !no_color && tty.stdout,
        stderr: !cli.json && !no_color && tty.stderr,
    };

    let clock = RealClock::new();
    let mut ports = NativePortTable;
    let mut system = WindowsSystem::new();
    let phase = ConsolePhase;

    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let stderr = io::stderr();
    let mut stderr = stderr.lock();

    let mut processes = match SysinfoProcessTable::new() {
        Ok(processes) => processes,
        Err(_) => {
            let envelope = Envelope::refused(cli.port, ErrorCode::QueryFailed, Vec::new());
            let code = output::write_output(
                cli.json,
                &envelope,
                &mut Streams {
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                },
                color,
                false,
            );
            std::process::exit(code);
        }
    };

    let code = {
        let mut deps = AppDeps {
            ports: &mut ports,
            processes: &mut processes,
            system: &mut system,
            clock: &clock,
            stdin: &mut stdin,
            streams: Streams {
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
            tty,
            color,
            phase: &phase,
        };
        app::run(&cli, &mut deps)
    };

    let _ = stdout.flush();
    let _ = stderr.flush();
    std::process::exit(code);
}

fn parse_cli(no_color: bool) -> Cli {
    let mut command = Cli::command();
    if no_color {
        command = command.color(clap::ColorChoice::Never);
    }
    let matches = command.get_matches();
    Cli::from_arg_matches(&matches).expect("clap matches the derived Cli")
}

fn no_color_requested() -> bool {
    std::env::args_os().any(|arg| arg == "--no-color") || std::env::var_os("NO_COLOR").is_some()
}
