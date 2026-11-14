//! Human-readable presentation is separate from policy and hardware operations.
use win_mmio_msr_cli::{rapl::decode_limits, Plan, Snapshot};

pub fn clean(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
pub fn paint(value: &str, style: &str, color: bool) -> String {
    if color {
        format!("\x1b[{style}m{value}\x1b[0m")
    } else {
        value.into()
    }
}
pub fn snapshot_json(s: &Snapshot) -> serde_json::Value {
    serde_json::json!({
        "identity":s.identity,"units":s.units,
        "msr":{"raw":format!("0x{:016x}",s.msr),"limits":decode_limits(s.msr,s.units)},
        "mmio":{"raw":format!("0x{:016x}",s.mmio),"limits":decode_limits(s.mmio,s.units)},
    })
}
pub fn snapshot(s: &Snapshot, color: bool) -> String {
    let mut text = format!(
        "{}  v{}\n{}\nCPU: {}\n\n",
        paint("RAPL CONTROL", "36;1", color),
        env!("CARGO_PKG_VERSION"),
        paint(
            if s.identity.simulated {
                "SIMULATION / KEINE HARDWARE"
            } else {
                "LIVE / WINDOWS BRIDGE"
            },
            "33;1",
            color
        ),
        clean(&s.identity.model)
    );
    text.push_str(&format!(
        "{}\n",
        paint(
            "Pfad       PL1 [W]   PL2 [W]   Aktiv 1/2   Clamp 1/2   Lock",
            "34;1",
            color
        )
    ));
    for (name, raw) in [("MSR", s.msr), ("MMIO", s.mmio)] {
        let d = decode_limits(raw, s.units);
        text.push_str(&format!("{name:<9} {:>8.3}  {:>8.3}   {:>5}/{:<5} {:>5}/{:<5} {}\n  raw 0x{raw:016x} | Zeitfenster-Codes: {} / {}\n",d.pl1.watts,d.pl2.watts,d.pl1.enabled,d.pl2.enabled,d.pl1.clamp,d.pl2.clamp,d.locked,d.pl1.time_window,d.pl2.time_window));
    }
    text.push_str(&format!(
        "\nEinheiten: {:.8} W / {:.8} s\n",
        s.units.watts_per_unit, s.units.seconds_per_unit
    ));
    let sync = win_mmio_msr_cli::rapl::power_fields_equal(s.msr, s.mmio);
    text.push_str(&paint(
        if sync {
            "PL1/PL2 und Enable-Bits sind synchron."
        } else {
            "ABWEICHUNG: MSR und MMIO haben unterschiedliche Power-Felder."
        },
        if sync { "32" } else { "33;1" },
        color,
    ));
    text.push('\n');
    text
}
pub fn plan(p: &Plan, color: bool) -> String {
    let (msr, mmio) = p.targets();
    let (pl1, pl2) = p.watts();
    format!("{}\n{}\nZiel: PL1 {pl1} W / PL2 {pl2} W\nMSR   0x{:016x} -> 0x{msr:016x}\nMMIO  0x{:016x} -> 0x{mmio:016x}\nZeitfenster, Clamp, Lock und reservierte Bits bleiben erhalten.\nBIOS/EC/DTT können weiterhin begrenzen.\n",snapshot(p.before(),color),paint("ÄNDERUNGSPLAN", "35;1",color),p.before().msr,p.before().mmio)
}
