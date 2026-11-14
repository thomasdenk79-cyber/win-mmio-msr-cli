//! rapl-watchdog: opt-in Windows service that re-applies audited PL1/PL2 limits
//! when firmware, the embedded controller or DTT resets them.
//!
//! The configuration comes only from the service command line (ImagePath). A rolling
//! correction budget and an error streak limit make the service halt instead of
//! fighting the platform or a failing driver forever.
#![cfg_attr(not(windows), allow(dead_code))]

use chrono::Utc;
use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};
use win_mmio_msr_cli::{
    apply,
    audit::{sanitize_text, AuditRecord, RegisterChange, MAX_DETAIL_CHARS, REGISTER_MSR_PKG_LIMIT},
    backend::{ExternalBridge, HardwareBackend},
    build_plan, validate_limits, Plan, Snapshot, MAX_TURBO_WATTS, MIN_WATTS, PROCESSOR_BASE_WATTS,
};

const SOURCE: &str = "rapl-watchdog";
const SERVICE_NAME: &str = "RaplWatchdog";
const AUDIT_REASON: &str = "watchdog_correction";
const BRIDGE_TIMEOUT: Duration = Duration::from_secs(10);
const BUDGET_WINDOW: Duration = Duration::from_secs(3600);
const BACKUP_PREFIX: &str = "rapl-before-";
/// Newest watchdog backups kept in the backup directory.
const KEEP_BACKUPS: usize = 100;
/// Service-specific exit codes reported to the Service Control Manager.
const EXIT_CONFIG: u32 = 1;
const EXIT_HALTED: u32 = 2;
const USAGE: &str = "usage: rapl-watchdog run|check-config --bridge <rapl-bridge.exe> \
                     --audit-log <file> --backup-dir <dir> [--pl1 W] [--pl2 W] \
                     [--interval-ms MS] [--max-corrections-per-hour N] \
                     [--max-consecutive-errors N]";
const OPTIONS: [&str; 8] = [
    "--bridge",
    "--audit-log",
    "--backup-dir",
    "--pl1",
    "--pl2",
    "--interval-ms",
    "--max-corrections-per-hour",
    "--max-consecutive-errors",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    bridge: PathBuf,
    audit_log: PathBuf,
    backup_dir: PathBuf,
    pl1: u16,
    pl2: u16,
    interval: Duration,
    max_corrections_per_hour: usize,
    max_consecutive_errors: u32,
}

fn parse_config(args: &[String]) -> Result<Config, String> {
    let mut values = BTreeMap::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let key = OPTIONS
            .into_iter()
            .find(|option| option == arg)
            .ok_or_else(|| format!("Unknown argument {arg}; {USAGE}"))?;
        let value = it.next().ok_or_else(|| format!("{key} needs a value"))?;
        if values.insert(key, value.as_str()).is_some() {
            return Err(format!("{key} was given more than once"));
        }
    }
    let path = |key: &str| -> Result<PathBuf, String> {
        let path = PathBuf::from(
            values
                .get(key)
                .ok_or_else(|| format!("{key} is required"))?,
        );
        if !path.is_absolute() {
            return Err(format!("{key} must be an absolute path"));
        }
        Ok(path)
    };
    let number = |key: &str, default: u64, min: u64, max: u64| -> Result<u64, String> {
        values.get(key).map_or(Ok(default), |text| {
            text.parse()
                .ok()
                .filter(|value| (min..=max).contains(value))
                .ok_or_else(|| format!("{key} must be an integer from {min} to {max}"))
        })
    };
    let watts = |key: &str, default: u16| -> Result<u16, String> {
        let value = number(
            key,
            default.into(),
            MIN_WATTS.into(),
            MAX_TURBO_WATTS.into(),
        )?;
        Ok(value as u16)
    };
    let pl1 = watts("--pl1", PROCESSOR_BASE_WATTS)?;
    let pl2 = watts("--pl2", MAX_TURBO_WATTS)?;
    validate_limits(pl1, pl2)?;
    Ok(Config {
        bridge: path("--bridge")?,
        audit_log: path("--audit-log")?,
        backup_dir: path("--backup-dir")?,
        pl1,
        pl2,
        interval: Duration::from_millis(number("--interval-ms", 5000, 1000, 3_600_000)?),
        max_corrections_per_hour: number("--max-corrections-per-hour", 6, 1, 60)? as usize,
        max_consecutive_errors: number("--max-consecutive-errors", 3, 1, 100)? as u32,
    })
}

fn describe(config: &Config) -> String {
    format!(
        "bridge={}\naudit_log={}\nbackup_dir={}\npl1_w={}\npl2_w={}\ninterval_ms={}\n\
         max_corrections_per_hour={}\nmax_consecutive_errors={}",
        config.bridge.display(),
        config.audit_log.display(),
        config.backup_dir.display(),
        config.pl1,
        config.pl2,
        config.interval.as_millis(),
        config.max_corrections_per_hour,
        config.max_consecutive_errors
    )
}

fn log_event(log: &Path, record: AuditRecord) -> Result<(), String> {
    record
        .append_to(log)
        .map_err(|e| format!("audit log write failed for {}: {e}", log.display()))
}

/// One record per register with PL1/PL2 before and set watts.
fn register_events(
    log: &Path,
    kind: &str,
    plan: &Plan,
    after: Option<&Snapshot>,
    backup: &Path,
) -> Result<(), String> {
    let units = plan.before().units;
    for change in plan.changes() {
        let readback = after.and_then(|snapshot| {
            if change.register == REGISTER_MSR_PKG_LIMIT {
                Some(snapshot.msr)
            } else {
                snapshot.mmio
            }
        });
        log_event(
            log,
            AuditRecord::new(SOURCE, kind)
                .field("reason", AUDIT_REASON)
                .register_change(&RegisterChange {
                    register: change.register,
                    units,
                    previous: change.before,
                    requested: change.after,
                    readback,
                })
                .field("backup", backup.display().to_string()),
        )?;
    }
    Ok(())
}

/// Deletes the oldest watchdog backups beyond `keep`; the names sort chronologically.
fn prune_backups(dir: &Path, keep: usize) -> Result<usize, String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
    let mut backups: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(BACKUP_PREFIX) && name.ends_with(".txt"))
        })
        .collect();
    backups.sort();
    let excess = backups.len().saturating_sub(keep);
    for path in &backups[..excess] {
        std::fs::remove_file(path).map_err(|e| format!("cannot delete {}: {e}", path.display()))?;
    }
    Ok(excess)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tick {
    InSync,
    Corrected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Fault {
    /// Counts toward the consecutive error limit.
    Error(String),
    /// Stops the watchdog immediately.
    Halt(String),
}

/// Correction budget and backup numbering of one service run.
struct Watchdog {
    max_corrections: usize,
    corrections: VecDeque<Instant>,
    sequence: u64,
}

impl Watchdog {
    fn new(config: &Config) -> Self {
        Self {
            max_corrections: config.max_corrections_per_hour,
            corrections: VecDeque::new(),
            sequence: 0,
        }
    }

    /// Reserves one correction within the rolling one-hour budget.
    fn reserve_correction(&mut self, now: Instant) -> Result<(), String> {
        while self
            .corrections
            .front()
            .is_some_and(|&time| now.saturating_duration_since(time) >= BUDGET_WINDOW)
        {
            self.corrections.pop_front();
        }
        if self.corrections.len() >= self.max_corrections {
            return Err(format!(
                "correction budget exhausted: {} corrections within one hour; firmware, EC \
                 or DTT keeps resetting the limits",
                self.max_corrections
            ));
        }
        self.corrections.push_back(now);
        Ok(())
    }

    fn tick(
        &mut self,
        config: &Config,
        backend: &mut dyn HardwareBackend,
        now: Instant,
    ) -> Result<Tick, Fault> {
        let plan = build_plan(backend, config.pl1, config.pl2).map_err(Fault::Error)?;
        if plan.already_applied() {
            return Ok(Tick::InSync);
        }
        if !plan.blockers().is_empty() {
            return Err(Fault::Error(format!(
                "Apply blocked: {}",
                plan.blockers().join("; ")
            )));
        }
        self.reserve_correction(now).map_err(Fault::Halt)?;
        std::fs::create_dir_all(&config.backup_dir).map_err(|e| {
            Fault::Error(format!(
                "cannot create {}: {e}",
                config.backup_dir.display()
            ))
        })?;
        self.sequence += 1;
        let backup = config.backup_dir.join(format!(
            "{BACKUP_PREFIX}{}-{:06}.txt",
            Utc::now().format("%Y%m%dT%H%M%S%.3fZ"),
            self.sequence
        ));
        let log = &config.audit_log;
        register_events(log, "correction_detected", &plan, None, &backup).map_err(Fault::Error)?;
        let after = apply(backend, &plan, &backup).map_err(Fault::Error)?;
        register_events(log, "correction_verified", &plan, Some(&after), &backup)
            .map_err(Fault::Error)?;
        Ok(Tick::Corrected)
    }
}

/// Runs corrections until `wait` reports a stop request. Returns the halt reason
/// when the watchdog gives up; start, errors, halt and stop are audited.
fn serve(
    config: &Config,
    mut connect: impl FnMut() -> Result<Box<dyn HardwareBackend>, String>,
    mut wait: impl FnMut(Duration) -> bool,
    mut clock: impl FnMut() -> Instant,
) -> Result<(), String> {
    let log = &config.audit_log;
    log_event(
        log,
        AuditRecord::new(SOURCE, "service_started")
            .field("version", env!("CARGO_PKG_VERSION"))
            .field("pl1_target_w", config.pl1)
            .field("pl2_target_w", config.pl2)
            .field("interval_ms", config.interval.as_millis() as u64)
            .field("max_corrections_per_hour", config.max_corrections_per_hour)
            .field("max_consecutive_errors", config.max_consecutive_errors),
    )?;
    let mut watchdog = Watchdog::new(config);
    let mut errors = 0u32;
    let halt = loop {
        let result = connect()
            .map_err(Fault::Error)
            .and_then(|mut backend| watchdog.tick(config, backend.as_mut(), clock()));
        match result {
            Ok(tick) => {
                errors = 0;
                if tick == Tick::Corrected {
                    if let Err(error) = prune_backups(&config.backup_dir, KEEP_BACKUPS) {
                        let _ = log_event(
                            log,
                            AuditRecord::new(SOURCE, "backup_prune_failed").field("error", error),
                        );
                    }
                }
            }
            Err(Fault::Halt(reason)) => break Some(reason),
            Err(Fault::Error(error)) => {
                errors += 1;
                let _ = log_event(
                    log,
                    AuditRecord::new(SOURCE, "service_error")
                        .field("error", sanitize_text(&error, MAX_DETAIL_CHARS))
                        .field("consecutive_errors", errors),
                );
                if errors >= config.max_consecutive_errors {
                    break Some(format!("{errors} consecutive errors; last error: {error}"));
                }
            }
        }
        if wait(config.interval) {
            break None;
        }
    };
    match halt {
        None => {
            let _ = log_event(log, AuditRecord::new(SOURCE, "service_stopped"));
            Ok(())
        }
        Some(reason) => {
            let reason = sanitize_text(&reason, MAX_DETAIL_CHARS);
            let _ = log_event(
                log,
                AuditRecord::new(SOURCE, "service_halted").field("halt_reason", reason.as_str()),
            );
            Err(reason)
        }
    }
}

fn connect_bridge(config: &Config) -> Result<Box<dyn HardwareBackend>, String> {
    let bridge = ExternalBridge::with_timeout(config.bridge.clone(), BRIDGE_TIMEOUT)?
        .with_audit_log(Some(config.audit_log.clone()))
        .with_audit_reason(Some(AUDIT_REASON.into()));
    Ok(Box::new(bridge))
}

fn arguments() -> Result<Vec<String>, String> {
    std::env::args_os()
        .skip(1)
        .map(|arg| {
            arg.into_string()
                .map_err(|_| "Arguments must be valid Unicode".to_string())
        })
        .collect()
}

/// Configuration of `rapl-watchdog run ...` from the service command line.
fn service_config(args: &[String]) -> Result<Config, String> {
    match args.split_first() {
        Some((mode, rest)) if mode == "run" => parse_config(rest),
        _ => Err(USAGE.into()),
    }
}

#[cfg(windows)]
mod scm {
    use super::*;
    use std::ffi::OsString;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use windows_service::{
        define_windows_service,
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher,
    };

    define_windows_service!(ffi_service_main, service_main);

    pub fn start() -> Result<String, String> {
        service_dispatcher::start(SERVICE_NAME, ffi_service_main).map_err(|e| {
            format!(
                "Cannot connect to the Service Control Manager ({e}); \
                 start the installed service with: sc.exe start {SERVICE_NAME}"
            )
        })?;
        Ok(String::new())
    }

    fn status(state: ServiceState, exit_code: ServiceExitCode) -> ServiceStatus {
        let running = matches!(state, ServiceState::Running);
        let pending = matches!(
            state,
            ServiceState::StartPending | ServiceState::StopPending
        );
        ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if running {
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            exit_code,
            checkpoint: 0,
            wait_hint: if pending {
                Duration::from_secs(30)
            } else {
                Duration::default()
            },
            process_id: None,
        }
    }

    fn service_main(start_parameters: Vec<OsString>) {
        let (stop_tx, stop_rx) = mpsc::channel();
        let handler = move |control: ServiceControl| match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = stop_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        };
        let Ok(handle) = service_control_handler::register(SERVICE_NAME, handler) else {
            return;
        };
        let ok = ServiceExitCode::Win32(0);
        let _ = handle.set_service_status(status(ServiceState::StartPending, ok));
        // The configuration comes only from the ImagePath so that it is visible and
        // validated at install time. The first start parameter is the service name.
        let exit_code = match arguments().and_then(|args| service_config(&args)) {
            Err(_) => ServiceExitCode::ServiceSpecific(EXIT_CONFIG),
            Ok(config) if start_parameters.len() > 1 => {
                let _ = log_event(
                    &config.audit_log,
                    AuditRecord::new(SOURCE, "service_start_refused")
                        .field("error", "service start parameters are not supported"),
                );
                ServiceExitCode::ServiceSpecific(EXIT_CONFIG)
            }
            Ok(config) => {
                let _ = handle.set_service_status(status(ServiceState::Running, ok));
                let stop_requested = |interval| {
                    !matches!(
                        stop_rx.recv_timeout(interval),
                        Err(RecvTimeoutError::Timeout)
                    )
                };
                let result = serve(
                    &config,
                    || connect_bridge(&config),
                    stop_requested,
                    Instant::now,
                );
                let _ = handle.set_service_status(status(ServiceState::StopPending, ok));
                match result {
                    Ok(()) => ok,
                    Err(_) => ServiceExitCode::ServiceSpecific(EXIT_HALTED),
                }
            }
        };
        let _ = handle.set_service_status(status(ServiceState::Stopped, exit_code));
    }
}

#[cfg(windows)]
fn start_service() -> Result<String, String> {
    scm::start()
}

#[cfg(not(windows))]
fn start_service() -> Result<String, String> {
    Err("rapl-watchdog run is Windows-only; build for x86_64-pc-windows-msvc".into())
}

fn execute(args: &[String]) -> Result<String, String> {
    match args.split_first() {
        Some((mode, rest)) if mode == "check-config" => parse_config(rest).map(|c| describe(&c)),
        Some((mode, _)) if mode == "run" => {
            service_config(args)?;
            start_service()
        }
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    match arguments().and_then(|args| execute(&args)) {
        Ok(text) if text.is_empty() => ExitCode::SUCCESS,
        Ok(text) => match writeln!(io::stdout().lock(), "{text}") {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        },
        Err(error) => {
            let _ = writeln!(
                io::stderr().lock(),
                "{}",
                sanitize_text(&error, MAX_DETAIL_CHARS)
            );
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use win_mmio_msr_cli::audit::read_log;
    use win_mmio_msr_cli::backend::MockBackend;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rapl_watchdog_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn args(dir: &Path, extra: &[&str]) -> Vec<String> {
        let path = |name: &str| dir.join(name).display().to_string();
        let mut args = vec![
            "--bridge".to_string(),
            path("rapl-bridge.exe"),
            "--audit-log".into(),
            path("audit.jsonl"),
            "--backup-dir".into(),
            path("backups"),
        ];
        args.extend(extra.iter().map(|arg| arg.to_string()));
        args
    }

    fn config(dir: &Path, extra: &[&str]) -> Config {
        parse_config(&args(dir, extra)).unwrap()
    }

    fn events(config: &Config) -> Vec<String> {
        let bytes = std::fs::read(&config.audit_log).unwrap();
        let log = read_log(bytes.as_slice(), None).unwrap();
        assert!(log.malformed.is_empty(), "{:?}", log.malformed);
        log.rows
            .into_iter()
            .filter(|row| row.source.as_deref() == Some(SOURCE))
            .map(|row| row.event.unwrap())
            .collect()
    }

    /// A fresh mock per tick models firmware restoring 55/157 W after each correction.
    fn firmware_resets(
        config: &Config,
    ) -> impl FnMut() -> Result<Box<dyn HardwareBackend>, String> + '_ {
        move || {
            let mock = MockBackend::default()
                .with_audit_log(Some(config.audit_log.clone()), Some(AUDIT_REASON.into()));
            Ok(Box::new(mock) as Box<dyn HardwareBackend>)
        }
    }

    fn stop_after(ticks: usize) -> impl FnMut(Duration) -> bool {
        let mut count = 0;
        move |_| {
            count += 1;
            count >= ticks
        }
    }

    #[test]
    fn parses_a_strict_configuration() {
        let dir = temp_dir("config");
        let defaults = config(&dir, &[]);
        assert_eq!((defaults.pl1, defaults.pl2), (55, 157));
        assert_eq!(defaults.interval, Duration::from_secs(5));
        assert_eq!(defaults.max_corrections_per_hour, 6);
        assert_eq!(defaults.max_consecutive_errors, 3);
        let custom = config(
            &dir,
            &["--pl1", "45", "--pl2", "100", "--interval-ms", "1000"],
        );
        assert_eq!((custom.pl1, custom.pl2), (45, 100));
        assert!(describe(&custom).contains("\npl1_w=45\npl2_w=100\ninterval_ms=1000\n"));
        let sustained = config(&dir, &["--pl1", "120", "--pl2", "157"]);
        assert_eq!((sustained.pl1, sustained.pl2), (120, 157));
        for extra in [
            &["--pl1", "160"][..],
            &["--pl1", "50", "--pl2", "45"],
            &["--pl2", "158"],
            &["--pl1", "-5"],
            &["--interval-ms", "999"],
            &["--max-corrections-per-hour", "0"],
            &["--max-corrections-per-hour", "61"],
            &["--max-consecutive-errors", "0"],
            &["--pl1", "45", "--pl1", "50"],
            &["--unknown", "1"],
            &["--pl1"],
        ] {
            assert!(parse_config(&args(&dir, extra)).is_err(), "{extra:?}");
        }
        let mut relative = args(&dir, &[]);
        relative[1] = "rapl-bridge.exe".into();
        assert!(parse_config(&relative).unwrap_err().contains("absolute"));
        let error = parse_config(&args(&dir, &[])[2..]).unwrap_err();
        assert!(error.contains("--bridge is required"), "{error}");
        assert!(
            service_config(&args(&dir, &[])).is_err(),
            "mode is required"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn in_sync_platform_needs_no_write() {
        let dir = temp_dir("in_sync");
        let config = config(&dir, &[]);
        let result = serve(
            &config,
            firmware_resets(&config),
            stop_after(3),
            Instant::now,
        );
        assert_eq!(result, Ok(()));
        assert_eq!(events(&config), ["service_started", "service_stopped"]);
        assert!(!config.backup_dir.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn correction_budget_halts_a_fight_with_firmware() {
        let dir = temp_dir("budget");
        let extra = [
            "--pl1",
            "45",
            "--pl2",
            "100",
            "--max-corrections-per-hour",
            "2",
        ];
        let config = config(&dir, &extra);
        let result = serve(
            &config,
            firmware_resets(&config),
            stop_after(10),
            Instant::now,
        );
        let reason = result.unwrap_err();
        assert!(
            reason.starts_with("correction budget exhausted"),
            "{reason}"
        );
        let events = events(&config);
        let count = |name: &str| events.iter().filter(|event| *event == name).count();
        assert_eq!(events.first().map(String::as_str), Some("service_started"));
        assert_eq!(events.last().map(String::as_str), Some("service_halted"));
        // Two corrections of two registers each.
        assert_eq!(count("correction_detected"), 4);
        assert_eq!(count("correction_verified"), 4);
        assert_eq!(std::fs::read_dir(&config.backup_dir).unwrap().count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn budget_is_a_rolling_hour() {
        let dir = temp_dir("rolling");
        let mut watchdog = Watchdog::new(&config(&dir, &["--max-corrections-per-hour", "2"]));
        let start = Instant::now();
        assert!(watchdog.reserve_correction(start).is_ok());
        assert!(watchdog
            .reserve_correction(start + Duration::from_secs(60))
            .is_ok());
        assert!(watchdog
            .reserve_correction(start + Duration::from_secs(120))
            .is_err());
        assert!(watchdog.reserve_correction(start + BUDGET_WINDOW).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn consecutive_errors_halt_the_watchdog() {
        let dir = temp_dir("errors");
        let config = config(&dir, &[]);
        let unavailable = || Err::<Box<dyn HardwareBackend>, _>("bridge unavailable".to_string());
        let reason = serve(&config, unavailable, stop_after(10), Instant::now).unwrap_err();
        assert_eq!(
            reason,
            "3 consecutive errors; last error: bridge unavailable"
        );
        assert_eq!(
            events(&config),
            [
                "service_started",
                "service_error",
                "service_error",
                "service_error",
                "service_halted"
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_success_resets_the_error_streak() {
        let dir = temp_dir("streak");
        let config = config(&dir, &[]);
        let mut calls = 0;
        let flaky = || {
            calls += 1;
            if calls % 3 == 0 {
                Ok(Box::new(MockBackend::default()) as Box<dyn HardwareBackend>)
            } else {
                Err("bridge timeout".to_string())
            }
        };
        assert_eq!(serve(&config, flaky, stop_after(9), Instant::now), Ok(()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prunes_only_old_watchdog_backups() {
        let dir = temp_dir("prune");
        let names = [
            "rapl-before-20260101T000000.000Z-000001.txt",
            "rapl-before-20260101T000001.000Z-000002.txt",
            "rapl-before-20260101T000002.000Z-000003.txt",
            "notes.txt",
            "rapl-before-manual.log",
        ];
        for name in names {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        assert_eq!(prune_backups(&dir, 1), Ok(2));
        let mut left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, [names[3], names[2], names[4]]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
