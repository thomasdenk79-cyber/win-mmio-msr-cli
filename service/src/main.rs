#![cfg_attr(not(windows), allow(dead_code, unused_imports))]
use chrono::Utc;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use win_mmio_msr_cli::{
    apply,
    audit::{AuditRecord, RegisterChange, REGISTER_MSR_PKG_LIMIT},
    backend::{ExternalBridge, HardwareBackend},
    build_plan, validate_limits, Plan, Snapshot,
};

const SOURCE: &str = "rapl-watchdog";

#[derive(Debug, Clone)]
struct Config {
    bridge: PathBuf,
    audit_log: PathBuf,
    backup_dir: PathBuf,
    pl1: u16,
    pl2: u16,
    interval: Duration,
}

fn parse_config(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let mut bridge = None;
    let mut audit = None;
    let mut backup = None;
    let mut pl1 = 55u16;
    let mut pl2 = 157u16;
    let mut interval = 5000u64;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "run" => {}
            "--bridge" => bridge = Some(PathBuf::from(it.next().ok_or("--bridge needs a path")?)),
            "--audit-log" => {
                audit = Some(PathBuf::from(it.next().ok_or("--audit-log needs a path")?))
            }
            "--backup-dir" => {
                backup = Some(PathBuf::from(it.next().ok_or("--backup-dir needs a path")?))
            }
            "--pl1" => {
                pl1 = it
                    .next()
                    .ok_or("--pl1 needs watts")?
                    .parse()
                    .map_err(|_| "invalid --pl1")?
            }
            "--pl2" => {
                pl2 = it
                    .next()
                    .ok_or("--pl2 needs watts")?
                    .parse()
                    .map_err(|_| "invalid --pl2")?
            }
            "--interval-ms" => {
                interval = it
                    .next()
                    .ok_or("--interval-ms needs milliseconds")?
                    .parse()
                    .map_err(|_| "invalid --interval-ms")?
            }
            other => return Err(format!("unknown service argument: {other}")),
        }
    }
    // The watchdog never runs PL1 above the processor base power (security rule 8).
    validate_limits(pl1, pl2, false)?;
    if interval < 1000 {
        return Err("interval must be at least 1000 ms".into());
    }
    Ok(Config {
        bridge: bridge.ok_or("--bridge is required")?,
        audit_log: audit.ok_or("--audit-log is required")?,
        backup_dir: backup.ok_or("--backup-dir is required")?,
        pl1,
        pl2,
        interval: Duration::from_millis(interval),
    })
}

fn event(log: &Path, record: AuditRecord) -> Result<(), String> {
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
        event(
            log,
            AuditRecord::new(SOURCE, kind)
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

fn backup_name(dir: &Path) -> PathBuf {
    dir.join(format!(
        "rapl-before-{}.txt",
        Utc::now().format("%Y%m%dT%H%M%S%.3fZ")
    ))
}

fn tick(config: &Config) -> Result<bool, String> {
    let bridge = ExternalBridge::with_timeout(config.bridge.clone(), Duration::from_secs(10))?
        .with_audit_log(Some(config.audit_log.clone()))
        .with_audit_reason(Some("watchdog_correction".into()));
    let mut backend: Box<dyn HardwareBackend> = Box::new(bridge);
    let plan = build_plan(backend.as_mut(), config.pl1, config.pl2, false)?;
    if plan.already_applied() {
        return Ok(false);
    }
    if !plan.blockers().is_empty() {
        return Err(format!("Apply blocked: {}", plan.blockers().join("; ")));
    }
    std::fs::create_dir_all(&config.backup_dir).map_err(|e| e.to_string())?;
    let backup = backup_name(&config.backup_dir);
    register_events(
        &config.audit_log,
        "correction_detected",
        &plan,
        None,
        &backup,
    )?;
    let after = apply(backend.as_mut(), &plan, &backup)?;
    register_events(
        &config.audit_log,
        "correction_verified",
        &plan,
        Some(&after),
        &backup,
    )?;
    Ok(true)
}

#[cfg(not(windows))]
fn main() {
    eprintln!("rapl-watchdog is Windows-only; build with x86_64-pc-windows-msvc");
}

#[cfg(windows)]
fn main() -> windows_service::Result<()> {
    windows_service::service_dispatcher::start("RaplWatchdog", ffi_service_main)
}

#[cfg(windows)]
windows_service::define_windows_service!(ffi_service_main, service_main);

#[cfg(windows)]
fn service_main(arguments: Vec<std::ffi::OsString>) {
    use std::sync::mpsc;
    use windows_service::{
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
    };
    let config = parse_config(
        arguments
            .into_iter()
            .map(|v| v.to_string_lossy().into_owned()),
    )
    .unwrap_or_else(|e| panic!("invalid service configuration: {e}"));
    let (stop_tx, stop_rx) = mpsc::channel();
    let handler = service_control_handler::register("RaplWatchdog", move |event| match event {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(());
            ServiceControlHandlerResult::NoError
        }
        _ => ServiceControlHandlerResult::NotImplemented,
    })
    .expect("register service handler");
    handler
        .set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })
        .expect("set service state");
    let _ = event(
        &config.audit_log,
        AuditRecord::new(SOURCE, "service_started")
            .field("pl1_watts", config.pl1)
            .field("pl2_watts", config.pl2)
            .field("interval_ms", config.interval.as_millis() as u64),
    );
    loop {
        if stop_rx.recv_timeout(config.interval).is_ok() {
            break;
        }
        if let Err(error) = tick(&config) {
            let _ = event(
                &config.audit_log,
                AuditRecord::new(SOURCE, "service_error").field("error", error),
            );
        }
    }
    let _ = event(
        &config.audit_log,
        AuditRecord::new(SOURCE, "service_stopped"),
    );
    let _ = handler.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_fast_polling() {
        assert!(parse_config(vec![
            "run".into(),
            "--bridge".into(),
            "b.exe".into(),
            "--audit-log".into(),
            "a".into(),
            "--backup-dir".into(),
            "b".into(),
            "--interval-ms".into(),
            "999".into()
        ])
        .is_err());
    }
}
