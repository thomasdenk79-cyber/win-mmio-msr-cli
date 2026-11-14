use std::{env, path::PathBuf, process::ExitCode};
use win_mmio_msr_cli::{
    apply, build_plan, describe_plan, describe_snapshot, read_snapshot,
    backend::{ExternalBridge, HardwareBackend, MockBackend},
};

#[derive(Default)]
struct Options {
    command: String,
    pl1: Option<u16>,
    pl2: Option<u16>,
    bridge: Option<PathBuf>,
    mock: bool,
    yes: bool,
    allow_sustained_above_base: bool,
    backup: Option<PathBuf>,
}

fn help() {
    println!(r#"win-mmio-msr-cli 0.1.0

Usage:
  win-mmio-msr-cli status --mock
  win-mmio-msr-cli plan --mock --pl1 55 --pl2 157
  win-mmio-msr-cli status --bridge C:\Tools\rapl-bridge.exe
  win-mmio-msr-cli plan --bridge C:\Tools\rapl-bridge.exe --pl1 55 --pl2 157
  win-mmio-msr-cli apply --bridge C:\Tools\rapl-bridge.exe --pl1 55 --pl2 157 --backup C:\Temp\rapl-before.txt --yes

Commands:
  status            Read current MSR/MMIO package power limits
  plan              Show exact read-modify-write values without writing
  apply             Write, verify, attempt rollback on failure (explicit opt-in)
  help              This text

Options:
  --mock                          Simulation; no hardware I/O
  --bridge PATH                   Explicit path to an audited Windows bridge .exe
  --pl1 W                         Long-duration PL1 (5..157)
  --pl2 W                         Short-duration PL2 (PL1..157)
  --allow-sustained-above-base    Permit PL1 > 55 W, not recommended for this laptop
  --backup PATH                   Mandatory new backup file for apply
  --yes                           Mandatory confirmation for apply

NOTES:
  The bridge is NOT bundled. See docs/BRIDGE_PROTOCOL.md and AGENTS.md.
  Setting 157 W in RAPL does NOT defeat BIOS/EC/thermal/current limits.
  Never disable Windows driver signature enforcement or vulnerable-driver blocklist.
"#);
}

fn take_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("Missing value after {flag}"))
}

fn parse_args() -> Result<Options, String> {
    let mut args = env::args().skip(1);
    let mut opt = Options {
        command: args.next().unwrap_or_else(|| "help".into()),
        ..Options::default()
    };
    while let Some(token) = args.next() {
        match token.as_str() {
            "--pl1" => opt.pl1 = Some(take_value(&mut args, "--pl1")?
                .parse::<u16>().map_err(|_| "Invalid PL1 watt value")?),
            "--pl2" => opt.pl2 = Some(take_value(&mut args, "--pl2")?
                .parse::<u16>().map_err(|_| "Invalid PL2 watt value")?),
            "--mock" => opt.mock = true,
            "--bridge" => opt.bridge = Some(PathBuf::from(take_value(&mut args, "--bridge")?)),
            "--yes" => opt.yes = true,
            "--allow-sustained-above-base" => opt.allow_sustained_above_base = true,
            "--backup" => opt.backup = Some(PathBuf::from(take_value(&mut args, "--backup")?)),
            "--help" | "-h" => opt.command = "help".into(),
            _ => return Err(format!("Unknown option: {token}")),
        }
    }
    if opt.mock && opt.bridge.is_some() {
        return Err("Choose --mock OR --bridge, not both".into());
    }
    Ok(opt)
}

fn run(opt: Options) -> Result<(), String> {
    if opt.command == "help" {
        help();
        return Ok(());
    }
    let mut backend: Box<dyn HardwareBackend> = match (opt.mock, opt.bridge) {
        (true, None) => Box::new(MockBackend::default()),
        (false, Some(path)) => Box::new(ExternalBridge::new(path)?),
        _ => return Err("Select --mock or an explicit --bridge PATH".into()),
    };

    match opt.command.as_str() {
        "status" => {
            let before = read_snapshot(backend.as_mut())?;
            print!("{}", describe_snapshot(&before));
        }
        "plan" | "apply" => {
            let pl1 = opt.pl1.ok_or("Missing --pl1 W")?;
            let pl2 = opt.pl2.ok_or("Missing --pl2 W")?;
            let plan = build_plan(
                backend.as_mut(), pl1, pl2, opt.allow_sustained_above_base
            )?;
            print!("{}", describe_plan(&plan));
            if opt.command == "apply" {
                if !opt.yes {
                    return Err("No writes performed: --yes is required".into());
                }
                let backup = opt.backup.ok_or(
                    "No writes performed: --backup NEW_FILENAME is required"
                )?;
                if plan.before.identity.simulated {
                    println!("SIMULATION ONLY: no physical registers will be written.");
                } else {
                    eprintln!(
                        "WARNING: Writing live CPU power limits. \
                         Firmware and cooling safety mechanisms remain authoritative."
                    );
                }
                let after = apply(backend.as_mut(), &plan, &backup)?;
                println!("Readback verified. Backup: {}", backup.display());
                print!("{}", describe_snapshot(&after));
            }
        }
        other => return Err(format!("Unknown command: {other}")),
    }
    Ok(())
}

fn main() -> ExitCode {
    match parse_args().and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ERROR: {e}\nRun 'win-mmio-msr-cli help' for usage.");
            ExitCode::FAILURE
        }
    }
}
