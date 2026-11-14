mod output;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
    process::ExitCode,
    time::Duration,
};
use win_mmio_msr_cli::{
    apply,
    backend::{ExternalBridge, HardwareBackend, MockBackend},
    build_plan, read_snapshot,
};

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Intel RAPL: controlled MSR + MMIO synchronization",
    long_about = "Intel RAPL: diagnostics, planning and controlled PL1/PL2 configuration.\nLive operation requires a separately validated Windows bridge. --mock performs no hardware access."
)]
struct Cli {
    #[arg(
        long,
        global = true,
        value_enum,
        default_value = "auto",
        help = "Terminal colors (JSON is always plain)"
    )]
    color: Color,
    #[arg(
        long,
        global = true,
        help = "Emit one JSON document instead of terminal output"
    )]
    json: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Color {
    Auto,
    Always,
    Never,
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
struct BackendChoice {
    #[arg(long, help = "Simulation without hardware access")]
    mock: bool,
    #[arg(
        long,
        value_name = "EXE",
        help = "Explicit path to the audited Windows bridge"
    )]
    bridge: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct Connection {
    #[command(flatten)]
    backend: BackendChoice,
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Append-only JSONL audit log for real register changes"
    )]
    audit_log: Option<PathBuf>,
    #[arg(long, default_value_t = 10000, value_parser = clap::value_parser!(u64).range(100..=120000), help = "Timeout per bridge call in milliseconds")]
    timeout_ms: u64,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Preset {
    Base,
    Turbo,
}

#[derive(Args, Debug)]
struct Limits {
    #[arg(long, required_unless_present = "preset", conflicts_with = "preset", value_parser = clap::value_parser!(u16).range(5..=157), help = "PL1 in watts")]
    pl1: Option<u16>,
    #[arg(long, required_unless_present = "preset", conflicts_with = "preset", value_parser = clap::value_parser!(u16).range(5..=157), help = "PL2 in watts, at least PL1")]
    pl2: Option<u16>,
    #[arg(
        long,
        value_enum,
        help = "base=55/157 W; turbo=157/157 W (explicit opt-in)"
    )]
    preset: Option<Preset>,
    #[arg(long, help = "Explicitly allow PL1 above 55 W")]
    allow_sustained_above_base: bool,
}
impl Limits {
    fn values(&self) -> Result<(u16, u16), String> {
        let pair = match self.preset {
            Some(Preset::Base) => (55, 157),
            Some(Preset::Turbo) => (157, 157),
            None => (
                self.pl1.ok_or("--pl1 is missing")?,
                self.pl2.ok_or("--pl2 is missing")?,
            ),
        };
        if pair.0 > pair.1 {
            return Err("PL1 must not exceed PL2".into());
        }
        if pair.0 > 55 && !self.allow_sustained_above_base {
            return Err("PL1 above 55 W requires --allow-sustained-above-base".into());
        }
        Ok(pair)
    }
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Read current registers and limits
    Status {
        #[command(flatten)]
        connection: Connection,
    },
    /// Show before/after plan without writing registers
    Plan {
        #[command(flatten)]
        connection: Connection,
        #[command(flatten)]
        limits: Limits,
    },
    /// Create backup, write and verify readback
    Apply {
        #[command(flatten)]
        connection: Connection,
        #[command(flatten)]
        limits: Limits,
        #[arg(long, required = true, help = "Explicitly confirm the write operation")]
        yes: bool,
        #[arg(
            long,
            value_name = "NEW_FILE",
            help = "New backup file (never overwritten)"
        )]
        backup: PathBuf,
    },
    /// Emit shell completion (for example PowerShell)
    Completions { shell: clap_complete::Shell },
}

fn backend(connection: &Connection) -> Result<Box<dyn HardwareBackend>, String> {
    if connection.backend.mock {
        return Ok(Box::new(MockBackend::default()));
    }
    let path = connection
        .backend
        .bridge
        .clone()
        .ok_or("bridge path is missing")?;
    Ok(Box::new(
        ExternalBridge::with_timeout(path, Duration::from_millis(connection.timeout_ms))?
            .with_audit_log(connection.audit_log.clone()),
    ))
}

fn execute(cli: &Cli, color: bool) -> Result<String, String> {
    match &cli.command {
        Commands::Completions { shell } => {
            let mut bytes = Vec::new();
            clap_complete::generate(*shell, &mut Cli::command(), "win-mmio-msr-cli", &mut bytes);
            let script = String::from_utf8(bytes).map_err(|e| e.to_string())?;
            if cli.json {
                return Ok(serde_json::json!({"schema_version":1,"ok":true,"command":"completions","script":script}).to_string());
            }
            Ok(script)
        }
        Commands::Status { connection } => {
            let snapshot = read_snapshot(backend(connection)?.as_mut())?;
            if cli.json {
                return Ok(serde_json::json!({"schema_version":1,"ok":true,"command":"status","snapshot":output::snapshot_json(&snapshot)}).to_string());
            }
            Ok(output::snapshot(&snapshot, color))
        }
        Commands::Plan { connection, limits }
        | Commands::Apply {
            connection, limits, ..
        } => {
            // Validate all policy arguments before starting any bridge process.
            let (pl1, pl2) = limits.values()?;
            let mut device = backend(connection)?;
            let plan = build_plan(device.as_mut(), pl1, pl2, limits.allow_sustained_above_base)?;
            if let Commands::Apply { backup, .. } = &cli.command {
                let after = apply(device.as_mut(), &plan, backup)?;
                if cli.json {
                    return Ok(serde_json::json!({"schema_version":1,"ok":true,"command":"apply","backup":backup,"plan":plan,"snapshot":output::snapshot_json(&after)}).to_string());
                }
                Ok(format!(
                    "{}\n{}\nBackup: {}\n{}",
                    output::plan(&plan, color),
                    output::paint("READBACK VERIFIZIERT", "32;1", color),
                    output::clean(&backup.display().to_string()),
                    output::snapshot(&after, color)
                ))
            } else {
                if cli.json {
                    return Ok(serde_json::json!({"schema_version":1,"ok":true,"command":"plan","plan":plan,"snapshot":output::snapshot_json(plan.before())}).to_string());
                }
                Ok(output::plan(&plan, color))
            }
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let color = !cli.json
        && match cli.color {
            Color::Always => true,
            Color::Never => false,
            Color::Auto => {
                io::stdout().is_terminal()
                    && std::env::var_os("NO_COLOR").is_none()
                    && std::env::var("TERM").as_deref() != Ok("dumb")
            }
        };
    let (message, code, stderr) = match execute(&cli, color) {
        Ok(text) => (text, ExitCode::SUCCESS, false),
        Err(error) if cli.json => (
            serde_json::json!({"schema_version":1,"ok":false,"error":error}).to_string(),
            ExitCode::FAILURE,
            false,
        ),
        Err(error) => (
            format!(
                "{} {}",
                output::paint("FEHLER", "31;1", color && io::stderr().is_terminal()),
                output::clean(&error)
            ),
            ExitCode::FAILURE,
            true,
        ),
    };
    let result = if stderr {
        writeln!(io::stderr().lock(), "{message}")
    } else {
        writeln!(io::stdout().lock(), "{message}")
    };
    match result {
        Ok(()) => code,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => code,
        Err(_) => ExitCode::FAILURE,
    }
}
