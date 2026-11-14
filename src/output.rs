//! Human-readable presentation is separate from policy and hardware operations.
use serde_json::{json, Value};
use win_mmio_msr_cli::{
    audit::{format_watts, hex, AuditLog, AuditRow},
    rapl::{decode_limits, power_fields_equal, Units},
    Plan, RegisterPlan, Snapshot,
};

/// Longest reason/detail cell in the audit table; CSV and JSON keep full text.
const MAX_TABLE_DETAIL: usize = 60;
/// Malformed lines listed below the audit table.
const MAX_MALFORMED_SHOWN: usize = 10;

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

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut text: String = value.chars().take(max_chars.saturating_sub(3)).collect();
    text.push_str("...");
    text
}

/// Seconds with up to four decimals and without trailing zeros.
fn seconds(value: f64) -> String {
    let text = format!("{value:.4}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Plain-text table; columns flagged in `numeric` are right-aligned.
fn table(headers: &[&str], numeric: &[bool], rows: &[Vec<String>], color: bool) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let render = |cells: &mut dyn Iterator<Item = &str>| -> String {
        cells
            .enumerate()
            .map(|(i, cell)| {
                let pad = " ".repeat(widths[i] - cell.chars().count());
                if numeric[i] {
                    format!("{pad}{cell}")
                } else {
                    format!("{cell}{pad}")
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_owned()
    };
    let mut text = paint(&render(&mut headers.iter().copied()), "34;1", color);
    text.push('\n');
    for row in rows {
        text.push_str(&render(&mut row.iter().map(String::as_str)));
        text.push('\n');
    }
    text
}

fn units_json(units: &Units) -> Value {
    json!({
        "raw": hex(units.raw),
        "watts_per_unit": units.watts_per_unit,
        "seconds_per_unit": units.seconds_per_unit,
    })
}

fn register_json(raw: u64, units: Units) -> Value {
    json!({"raw": hex(raw), "limits": decode_limits(raw, units)})
}

pub fn snapshot_json(s: &Snapshot) -> Value {
    json!({
        "identity": s.identity,
        "units": units_json(&s.units),
        "msr": register_json(s.msr, s.units),
        "mmio": s.mmio.map(|raw| register_json(raw, s.units)),
        "synchronized": s.mmio.map(|raw| power_fields_equal(s.msr, raw)),
    })
}

fn change_json(change: &RegisterPlan) -> Value {
    json!({
        "register": change.register,
        "path": change.path,
        "before": hex(change.before),
        "after": hex(change.after),
        "pl1_before_w": change.watts.pl1_before_w,
        "pl2_before_w": change.watts.pl2_before_w,
        "pl1_set_w": change.watts.pl1_set_w,
        "pl2_set_w": change.watts.pl2_set_w,
    })
}

pub fn plan_json(p: &Plan) -> Value {
    let (pl1, pl2) = p.watts();
    json!({
        "target": {"pl1_w": pl1, "pl2_w": pl2},
        "changes": p.changes().iter().map(change_json).collect::<Vec<_>>(),
        "already_applied": p.already_applied(),
        "blockers": p.blockers(),
        "warnings": p.warnings(),
        "before": snapshot_json(p.before()),
    })
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

pub fn snapshot(s: &Snapshot, color: bool) -> String {
    let id = &s.identity;
    let mode = if id.simulated {
        "SIMULATION / NO HARDWARE"
    } else {
        "LIVE / WINDOWS BRIDGE"
    };
    let mut text = format!(
        "{}  v{}\n{}\nCPU: {} (family 0x{:x}, model 0x{:X}, stepping 0x{:x}, packages {})\n",
        paint("RAPL CONTROL", "36;1", color),
        env!("CARGO_PKG_VERSION"),
        paint(mode, "33;1", color),
        clean(&id.model),
        id.cpu_family,
        id.cpu_model,
        id.cpu_stepping,
        id.packages,
    );
    text.push_str(&format!(
        "Driver gates: MMIO read {}, MMIO layout {}, live writes {}\nMCHBAR: {}\n\n",
        on_off(id.mmio_readable),
        if id.mmio_verified {
            "validated"
        } else {
            "NOT validated"
        },
        on_off(id.live_writes_enabled),
        id.mchbar_base.map_or("unavailable".into(), hex),
    ));
    let mmio_path = if id.mmio_verified {
        "MMIO"
    } else {
        "MMIO [UNVERIFIED LAYOUT]"
    };
    let registers = [("MSR", Some(s.msr)), (mmio_path, s.mmio)];
    let rows: Vec<Vec<String>> = registers
        .iter()
        .filter_map(|(path, raw)| raw.map(|raw| (path, raw)))
        .map(|(path, raw)| {
            let d = decode_limits(raw, s.units);
            vec![
                path.to_string(),
                format_watts(d.pl1.watts),
                format_watts(d.pl2.watts),
                format!("{}/{}", d.pl1.enabled, d.pl2.enabled),
                format!("{}/{}", d.pl1.clamp, d.pl2.clamp),
                seconds(d.pl1.time_window_s),
                seconds(d.pl2.time_window_s),
                d.locked.to_string(),
                hex(raw),
            ]
        })
        .collect();
    text.push_str(&table(
        &[
            "Path",
            "PL1 [W]",
            "PL2 [W]",
            "Enabled 1/2",
            "Clamp 1/2",
            "Tau1 [s]",
            "Tau2 [s]",
            "Lock",
            "Raw",
        ],
        &[false, true, true, false, false, true, true, false, false],
        &rows,
        color,
    ));
    if s.mmio.is_none() {
        text.push_str("MMIO: unavailable (the driver does not expose MCHBAR+0x59A0)\n");
    }
    text.push_str(&format!(
        "\nUnits: {} W / {} s per unit\n",
        s.units.watts_per_unit, s.units.seconds_per_unit
    ));
    let (message, style) = match s.mmio {
        Some(mmio) if power_fields_equal(s.msr, mmio) => {
            ("PL1/PL2 and enable bits are synchronized.", "32")
        }
        Some(_) => ("MISMATCH: MSR and MMIO power fields differ.", "33;1"),
        None => (
            "MSR/MMIO synchronization cannot be checked without MMIO access.",
            "33",
        ),
    };
    text.push_str(&paint(message, style, color));
    text.push('\n');
    text
}

fn change_table(changes: &[RegisterPlan], color: bool) -> String {
    let rows: Vec<Vec<String>> = changes
        .iter()
        .map(|c| {
            vec![
                c.path.to_string(),
                format_watts(c.watts.pl1_before_w),
                format_watts(c.watts.pl2_before_w),
                format_watts(c.watts.pl1_set_w),
                format_watts(c.watts.pl2_set_w),
                format!("{} -> {}", hex(c.before), hex(c.after)),
            ]
        })
        .collect();
    table(
        &[
            "Path",
            "PL1 before [W]",
            "PL2 before [W]",
            "PL1 set [W]",
            "PL2 set [W]",
            "Raw before -> after",
        ],
        &[false, true, true, true, true, false],
        &rows,
        color,
    )
}

pub fn plan(p: &Plan, color: bool) -> String {
    let (pl1, pl2) = p.watts();
    let mut text = snapshot(p.before(), color);
    text.push_str(&format!(
        "\n{}\nTarget: PL1 {pl1} W / PL2 {pl2} W\n",
        paint("CHANGE PLAN", "35;1", color)
    ));
    text.push_str(&change_table(&p.changes(), color));
    text.push_str(
        "Time windows, clamp, lock and reserved bits are preserved.\n\
         BIOS/EC/DTT may still impose lower limits.\n",
    );
    for warning in p.warnings() {
        text.push_str(&format!(
            "{} {}\n",
            paint("WARNING:", "33;1", color),
            clean(warning)
        ));
    }
    if !p.blockers().is_empty() {
        text.push_str(&paint("APPLY BLOCKED", "31;1", color));
        text.push('\n');
        for blocker in p.blockers() {
            text.push_str(&format!("  - {}\n", clean(blocker)));
        }
    } else if p.already_applied() {
        text.push_str(&paint(
            "Target already active: no register write needed.",
            "32",
            color,
        ));
        text.push('\n');
    }
    text
}

fn cell(value: &Option<String>) -> String {
    value.as_deref().map_or("-".into(), clean)
}

fn watts_cell(value: Option<f64>) -> String {
    value.map_or("-".into(), format_watts)
}

pub fn audit_table(log: &AuditLog, color: bool) -> String {
    let rows: Vec<Vec<String>> = log
        .rows
        .iter()
        .map(|row| {
            let mut source = cell(&row.source);
            if row.simulated {
                source.push_str(" [SIM]");
            }
            let note = [row.reason.as_deref(), row.detail.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" | ");
            vec![
                cell(&row.timestamp_utc),
                source,
                cell(&row.event),
                cell(&row.phase),
                cell(&row.register),
                watts_cell(row.pl1_before_w),
                watts_cell(row.pl2_before_w),
                watts_cell(row.pl1_set_w),
                watts_cell(row.pl2_set_w),
                cell(&row.outcome),
                if note.is_empty() {
                    "-".into()
                } else {
                    truncate(&clean(&note), MAX_TABLE_DETAIL)
                },
            ]
        })
        .collect();
    let mut text = if rows.is_empty() {
        "No audit records.\n".to_owned()
    } else {
        table(
            &[
                "Time (UTC)",
                "Source",
                "Event",
                "Phase",
                "Register",
                "PL1 before [W]",
                "PL2 before [W]",
                "PL1 set [W]",
                "PL2 set [W]",
                "Outcome",
                "Reason / detail",
            ],
            &[
                false, false, false, false, false, true, true, true, true, false, false,
            ],
            &rows,
            color,
        )
    };
    if !log.malformed.is_empty() {
        text.push_str(&paint(
            &format!("WARNING: {} malformed line(s) skipped", log.malformed.len()),
            "33;1",
            color,
        ));
        text.push('\n');
        for problem in log.malformed.iter().take(MAX_MALFORMED_SHOWN) {
            text.push_str(&format!("  - {}\n", clean(problem)));
        }
    }
    text
}

/// CSV text cell: quoted when needed and protected against spreadsheet formulas.
fn csv_text(value: &Option<String>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    let mut text = value.clone();
    if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        text.insert(0, '\'');
    }
    if text.contains([',', '"', '\n', '\r']) || text.trim() != text {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

fn csv_number(value: Option<f64>) -> String {
    value.map(format_watts).unwrap_or_default()
}

pub fn audit_csv(rows: &[AuditRow]) -> String {
    let mut text = String::from(
        "line,timestamp_utc,source,event,phase,reason,register,pl1_before_w,pl2_before_w,\
         pl1_set_w,pl2_set_w,pl1_readback_w,pl2_readback_w,outcome,previous,requested,\
         readback,simulated,detail\n",
    );
    for row in rows {
        let fields = [
            row.line.to_string(),
            csv_text(&row.timestamp_utc),
            csv_text(&row.source),
            csv_text(&row.event),
            csv_text(&row.phase),
            csv_text(&row.reason),
            csv_text(&row.register),
            csv_number(row.pl1_before_w),
            csv_number(row.pl2_before_w),
            csv_number(row.pl1_set_w),
            csv_number(row.pl2_set_w),
            csv_number(row.pl1_readback_w),
            csv_number(row.pl2_readback_w),
            csv_text(&row.outcome),
            csv_text(&row.previous),
            csv_text(&row.requested),
            csv_text(&row.readback),
            row.simulated.to_string(),
            csv_text(&row.detail),
        ];
        text.push_str(&fields.join(","));
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_cells_are_quoted_and_formula_safe() {
        assert_eq!(csv_text(&None), "");
        assert_eq!(csv_text(&Some("plain".into())), "plain");
        assert_eq!(csv_text(&Some("a,b".into())), "\"a,b\"");
        assert_eq!(csv_text(&Some("say \"hi\"".into())), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_text(&Some("=cmd()".into())), "'=cmd()");
        assert_eq!(csv_text(&Some("-1,2".into())), "\"'-1,2\"");
    }

    #[test]
    fn table_aligns_numeric_columns() {
        let text = table(
            &["Name", "W"],
            &[false, true],
            &[
                vec!["a".into(), "5".into()],
                vec!["bb".into(), "157".into()],
            ],
            false,
        );
        assert_eq!(text, "Name    W\na       5\nbb    157\n");
    }
}
