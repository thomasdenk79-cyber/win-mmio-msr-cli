mod output;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use serde_json::json;
use std::{
    fs::File,
    io::{self, BufReader, IsTerminal, Write},
    path::PathBuf,
    process::ExitCode,
    time::Duration,
};
use win_mmio_msr_cli::{
    apply,
    audit::{read_log, sanitize_label},
    backend::{ExternalBridge, HardwareBackend, MockBackend},
    build_plan, read_snapshot, validate_limits, MAX_TURBO_WATTS, MIN_WATTS, PROCESSOR_BASE_WATTS,
};

/// Version of every `--json` document.
const JSON_SCHEMA_VERSION: u32 = 2;

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
    #[arg(long, required_unless_present = "preset", conflicts_with = "preset", value_parser = clap::value_parser!(u16).range(i64::from(MIN_WATTS)..=i64::from(MAX_TURBO_WATTS)), help = "PL1 in watts")]
    pl1: Option<u16>,
    #[arg(long, required_unless_present = "preset", conflicts_with = "preset", value_parser = clap::value_parser!(u16).range(i64::from(MIN_WATTS)..=i64::from(MAX_TURBO_WATTS)), help = "PL2 in watts, at least PL1")]
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
            Some(Preset::Base) => (PROCESSOR_BASE_WATTS, MAX_TURBO_WATTS),
            Some(Preset::Turbo) => (MAX_TURBO_WATTS, MAX_TURBO_WATTS),
            None => (
                self.pl1.ok_or("--pl1 is missing")?,
                self.pl2.ok_or("--pl2 is missing")?,
            ),
        };
        validate_limits(pair.0, pair.1, self.allow_sustained_above_base)?;
        Ok(pair)
    }
}

fn parse_reason(value: &str) -> Result<String, String> {
    let reason = sanitize_label(value);
    if reason.is_empty() {
        return Err("reason must not be empty".into());
    }
    Ok(reason)
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Read current registers and limits
    Status {
        #[command(flatten)]
        connection: Connection,
    },
    /// Show the before/set watt table without writing registers
    Plan {
        #[command(flatten)]
        connection: Connection,
        #[command(flatten)]
        limits: Limits,
    },
    /// Create a backup, write, verify readback and audit every register change
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
        #[arg(
            long,
            value_name = "PATH",
            help = "Append-only JSONL audit log (required with --bridge)"
        )]
        audit_log: Option<PathBuf>,
        #[arg(long, default_value = "cli_apply", value_parser = parse_reason, help = "Reason recorded in the audit log")]
        reason: String,
    },
    /// Show an audit log with PL1/PL2 before and set watts (read-only)
    Audit {
        #[arg(
            value_name = "LOG",
            help = "JSONL audit log written by bridge, watchdog or mock"
        )]
        log: PathBuf,
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..=1_000_000), help = "Only show the newest N records")]
        last: Option<u64>,
        #[arg(long, help = "Emit CSV instead of a table")]
        csv: bool,
    },
    /// Emit shell completion (for example PowerShell)
    Completions { shell: clap_complete::Shell },
}

fn backend(
    connection: &Connection,
    audit: Option<(&PathBuf, &str)>,
) -> Result<Box<dyn HardwareBackend>, String> {
    let log = audit.map(|(log, _)| log.clone());
    let reason = audit.map(|(_, reason)| reason.to_owned());
    if connection.backend.mock {
        return Ok(Box::new(MockBackend::default().with_audit_log(log, reason)));
    }
    let path = connection
        .backend
        .bridge
        .clone()
        .ok_or("bridge path is missing")?;
    Ok(Box::new(
        ExternalBridge::with_timeout(path, Duration::from_millis(connection.timeout_ms))?
            .with_audit_log(log)
            .with_audit_reason(reason),
    ))
}

fn envelope(command: &str, body: serde_json::Value) -> String {
    let mut document =
        json!({"schema_version": JSON_SCHEMA_VERSION, "ok": true, "command": command});
    if let (Some(target), serde_json::Value::Object(fields)) = (document.as_object_mut(), body) {
        target.extend(fields);
    }
    document.to_string()
}

fn execute(cli: &Cli, color: bool) -> Result<String, String> {
    match &cli.command {
        Commands::Completions { shell } => {
            let mut bytes = Vec::new();
            clap_complete::generate(*shell, &mut Cli::command(), "win-mmio-msr-cli", &mut bytes);
            let script = String::from_utf8(bytes).map_err(|e| e.to_string())?;
            if cli.json {
                return Ok(envelope("completions", json!({"script": script})));
            }
            Ok(script)
        }
        Commands::Status { connection } => {
            let snapshot = read_snapshot(backend(connection, None)?.as_mut())?;
            if cli.json {
                return Ok(envelope(
                    "status",
                    json!({"snapshot": output::snapshot_json(&snapshot)}),
                ));
            }
            Ok(output::snapshot(&snapshot, color))
        }
        Commands::Plan { connection, limits } => {
            // Validate all policy arguments before starting any bridge process.
            let (pl1, pl2) = limits.values()?;
            let mut device = backend(connection, None)?;
            let plan = build_plan(device.as_mut(), pl1, pl2, limits.allow_sustained_above_base)?;
            if cli.json {
                return Ok(envelope("plan", json!({"plan": output::plan_json(&plan)})));
            }
            Ok(output::plan(&plan, color))
        }
        Commands::Apply {
            connection,
            limits,
            backup,
            audit_log,
            reason,
            ..
        } => {
            let (pl1, pl2) = limits.values()?;
            if connection.backend.bridge.is_some() && audit_log.is_none() {
                return Err(
                    "Live apply requires --audit-log: every register change must be audited".into(),
                );
            }
            let audit = audit_log.as_ref().map(|log| (log, reason.as_str()));
            let mut device = backend(connection, audit)?;
            let plan = build_plan(device.as_mut(), pl1, pl2, limits.allow_sustained_above_base)?;
            if plan.already_applied() && plan.blockers().is_empty() {
                if cli.json {
                    return Ok(envelope(
                        "apply",
                        json!({"changed": false, "plan": output::plan_json(&plan)}),
                    ));
                }
                return Ok(format!(
                    "{}No register write performed; no backup created.",
                    output::plan(&plan, color)
                ));
            }
            let after = apply(device.as_mut(), &plan, backup)?;
            if cli.json {
                return Ok(envelope(
                    "apply",
                    json!({
                        "changed": true,
                        "backup": backup,
                        "audit_log": audit_log,
                        "plan": output::plan_json(&plan),
                        "after": output::snapshot_json(&after),
                    }),
                ));
            }
            let mut text = format!(
                "{}\n{}\nBackup: {}\n",
                output::plan(&plan, color),
                output::paint("READBACK VERIFIED", "32;1", color),
                output::clean(&backup.display().to_string()),
            );
            if let Some(log) = audit_log {
                text.push_str(&format!(
                    "Audit log: {}\n",
                    output::clean(&log.display().to_string())
                ));
            }
            text.push('\n');
            text.push_str(&output::snapshot(&after, color));
            Ok(text)
        }
        Commands::Audit { log, last, csv } => {
            if *csv && cli.json {
                return Err("--csv cannot be combined with --json".into());
            }
            let file = File::open(log)
                .map_err(|e| format!("Cannot open audit log {}: {e}", log.display()))?;
            let keep = last.map(|n| usize::try_from(n).unwrap_or(usize::MAX));
            let parsed = read_log(BufReader::new(file), keep)
                .map_err(|e| format!("Cannot read audit log {}: {e}", log.display()))?;
            if cli.json {
                return Ok(envelope(
                    "audit",
                    json!({"log": log, "rows": parsed.rows, "malformed": parsed.malformed}),
                ));
            }
            if *csv {
                for problem in &parsed.malformed {
                    let _ = writeln!(
                        io::stderr().lock(),
                        "WARNING: skipped {}",
                        output::clean(problem)
                    );
                }
                return Ok(output::audit_csv(&parsed.rows));
            }
            Ok(output::audit_table(&parsed, color))
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
            json!({"schema_version": JSON_SCHEMA_VERSION, "ok": false, "error": error}).to_string(),
            ExitCode::FAILURE,
            false,
        ),
        Err(error) => (
            format!(
                "{} {}",
                output::paint("ERROR", "31;1", color && io::stderr().is_terminal()),
                output::clean(&error)
            ),
            ExitCode::FAILURE,
            true,
        ),
    };
    let message = message.trim_end_matches('\n');
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

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<String, String> {
        let cli =
            Cli::try_parse_from(std::iter::once("win-mmio-msr-cli").chain(args.iter().copied()))
                .map_err(|e| e.to_string())?;
        execute(&cli, false)
    }

    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn plan_shows_before_and_set_watt_columns() {
        let text = run(&["plan", "--mock", "--pl1", "50", "--pl2", "120"]).unwrap();
        for header in [
            "PL1 before [W]",
            "PL2 before [W]",
            "PL1 set [W]",
            "PL2 set [W]",
        ] {
            assert!(text.contains(header), "{text}");
        }
        let json: serde_json::Value = serde_json::from_str(
            &run(&["plan", "--mock", "--pl1", "50", "--pl2", "120", "--json"]).unwrap(),
        )
        .unwrap();
        assert_eq!(json["schema_version"], 2);
        let change = &json["plan"]["changes"][1];
        assert_eq!(change["register"], "MSR_0x610");
        assert_eq!(change["pl1_before_w"], 55.0);
        assert_eq!(change["pl2_before_w"], 157.0);
        assert_eq!(change["pl1_set_w"], 50.0);
        assert_eq!(change["pl2_set_w"], 120.0);
    }

    #[test]
    fn rejects_policy_violations_before_hardware_access() {
        let error = run(&["plan", "--mock", "--pl1", "60", "--pl2", "157"]).unwrap_err();
        assert!(error.contains("--allow-sustained-above-base"), "{error}");
        assert!(run(&[
            "plan",
            "--mock",
            "--pl1",
            "100",
            "--pl2",
            "90",
            "--allow-sustained-above-base"
        ])
        .is_err());
        assert!(run(&["plan", "--mock", "--pl1", "4", "--pl2", "90"]).is_err());
    }

    #[test]
    fn mock_apply_is_audited_and_readable() {
        let dir = std::env::temp_dir();
        let tag = format!("{}_{}", std::process::id(), "cli");
        let log = dir.join(format!("rapl_cli_{tag}.jsonl"));
        let backup = dir.join(format!("rapl_cli_{tag}.txt"));
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_file(&backup);
        let (log_arg, backup_arg) = (log.to_str().unwrap(), backup.to_str().unwrap());
        run(&[
            "apply",
            "--mock",
            "--pl1",
            "45",
            "--pl2",
            "100",
            "--yes",
            "--backup",
            backup_arg,
            "--audit-log",
            log_arg,
            "--reason",
            "unit=test",
        ])
        .unwrap();
        let table = run(&["audit", log_arg]).unwrap();
        assert!(table.contains("PL1 set [W]"), "{table}");
        assert!(table.contains("unit=test"), "{table}");
        let csv = run(&["audit", log_arg, "--csv", "--last", "1"]).unwrap();
        let lines: Vec<_> = csv.lines().collect();
        assert_eq!(lines.len(), 2, "{csv}");
        assert!(lines[0].contains("pl1_before_w,pl2_before_w,pl1_set_w,pl2_set_w"));
        assert!(lines[1].contains(",55,157,45,100,"), "{csv}");
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_file(&backup);
    }

    #[test]
    fn apply_without_change_writes_nothing() {
        let backup = std::env::temp_dir().join(format!("rapl_cli_{}_noop.txt", std::process::id()));
        let _ = std::fs::remove_file(&backup);
        let text = run(&[
            "apply",
            "--mock",
            "--preset",
            "base",
            "--yes",
            "--backup",
            backup.to_str().unwrap(),
        ])
        .unwrap();
        assert!(text.contains("No register write performed"), "{text}");
        assert!(!backup.exists());
    }
}
