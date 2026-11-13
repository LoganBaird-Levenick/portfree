//! Command line surface.

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "portfree",
    version,
    about = "Identify the process that owns a TCP port and stop exactly that process",
    long_about = None,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// TCP port to inspect (1-65535)
    #[arg(value_name = "PORT", value_parser = parse_port)]
    pub port: u16,

    /// Assert the owning PID; only valid with a port
    #[arg(long, value_name = "PID")]
    pub pid: Option<u32>,

    /// Resolve and report without killing
    #[arg(long)]
    pub dry_run: bool,

    /// Emit one JSON document on stdout
    #[arg(long)]
    pub json: bool,

    /// Kill without prompting; the only way to authorize without a TTY
    #[arg(long)]
    pub yes: bool,

    /// Disable color on every stream
    #[arg(long)]
    pub no_color: bool,
}

impl Cli {
    /// `--dry-run` beats `--yes`; `--yes` beats the dry-run implied by `--json`.
    pub fn dry_run_effective(&self) -> bool {
        self.dry_run || (self.json && !self.yes)
    }

    /// The prompt is only reached when the invocation was neither a dry run nor
    /// pre-authorized with `--yes`.
    pub fn prompt_required(&self) -> bool {
        !self.dry_run_effective() && !self.yes
    }
}

fn parse_port(value: &str) -> Result<u16, String> {
    let port: u16 = value
        .parse()
        .map_err(|_| "PORT must be an integer between 1 and 65535".to_string())?;
    if port == 0 {
        return Err("PORT must be between 1 and 65535".into());
    }
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use clap::error::ErrorKind;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("portfree").chain(args.iter().copied()))
    }

    #[test]
    fn no_arguments_is_a_usage_error() {
        assert_eq!(
            parse(&[]).unwrap_err().kind(),
            ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn port_zero_is_a_usage_error() {
        assert!(parse(&["0"]).is_err());
    }

    #[test]
    fn ports_above_the_range_are_usage_errors() {
        assert!(parse(&["65536"]).is_err());
        assert!(parse(&["70000"]).is_err());
    }

    #[test]
    fn pid_without_a_port_is_a_usage_error() {
        assert_eq!(
            parse(&["--pid", "42"]).unwrap_err().kind(),
            ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn dry_run_wins_over_yes_and_json_implies_dry_run() {
        let cli = parse(&["3000", "--dry-run", "--yes"]).unwrap();
        assert!(cli.dry_run_effective());

        let cli = parse(&["3000", "--json"]).unwrap();
        assert!(cli.dry_run_effective());
        assert!(!cli.prompt_required());

        let cli = parse(&["3000", "--json", "--yes"]).unwrap();
        assert!(!cli.dry_run_effective());
        assert!(!cli.prompt_required());

        let cli = parse(&["3000"]).unwrap();
        assert!(!cli.dry_run_effective());
        assert!(cli.prompt_required());
    }
}
