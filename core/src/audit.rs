//! Append-only JSONL audit records shared by the bridge, the watchdog and the CLI.
//!
//! Every register-level record carries the raw register values plus four readable
//! columns: `pl1_before_w`, `pl2_before_w`, `pl1_set_w` and `pl2_set_w`.
use crate::rapl::{decode_limits, Units};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::io::{self, BufRead, Write};
use std::path::Path;

/// Version of the JSONL record layout. Records without `schema` are version 1.
pub const AUDIT_SCHEMA_VERSION: u32 = 2;

pub const REGISTER_MSR_UNITS: &str = "MSR_0x606";
pub const REGISTER_MSR_PKG_LIMIT: &str = "MSR_0x610";
pub const REGISTER_MMIO_PKG_LIMIT: &str = "MMIO_MCHBAR_0x59A0";

/// Longest free-text label (for example a reason) stored in a record.
pub const MAX_LABEL_CHARS: usize = 128;
/// Longest diagnostic text (for example a driver error) stored or displayed.
pub const MAX_DETAIL_CHARS: usize = 1024;

/// Step of a change sequence that caused a register write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditPhase {
    Apply,
    Rollback,
}

impl AuditPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apply => "apply",
            Self::Rollback => "rollback",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "apply" => Some(Self::Apply),
            "rollback" => Some(Self::Rollback),
            _ => None,
        }
    }
}

/// Readable package limits in watts for one register transition.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WattColumns {
    pub pl1_before_w: f64,
    pub pl2_before_w: f64,
    pub pl1_set_w: f64,
    pub pl2_set_w: f64,
}

impl WattColumns {
    pub fn new(previous: u64, requested: u64, units: Units) -> Self {
        let before = decode_limits(previous, units);
        let set = decode_limits(requested, units);
        Self {
            pl1_before_w: before.pl1.watts,
            pl2_before_w: before.pl2.watts,
            pl1_set_w: set.pl1.watts,
            pl2_set_w: set.pl2.watts,
        }
    }
}

/// One power-limit register transition.
#[derive(Debug, Clone, Copy)]
pub struct RegisterChange<'a> {
    pub register: &'a str,
    pub units: Units,
    pub previous: u64,
    pub requested: u64,
    pub readback: Option<u64>,
}

/// Builder for one JSONL line.
#[derive(Debug, Clone)]
pub struct AuditRecord {
    fields: Map<String, Value>,
}

impl AuditRecord {
    pub fn new(source: &str, event: &str) -> Self {
        let mut fields = Map::new();
        fields.insert("schema".into(), AUDIT_SCHEMA_VERSION.into());
        fields.insert("timestamp_utc".into(), utc_timestamp().into());
        fields.insert("source".into(), source.into());
        fields.insert("pid".into(), std::process::id().into());
        fields.insert("event".into(), event.into());
        Self { fields }
    }

    pub fn field(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    /// Adds raw values and the readable PL1/PL2 watt columns.
    pub fn register_change(self, change: &RegisterChange<'_>) -> Self {
        let watts = WattColumns::new(change.previous, change.requested, change.units);
        let readback = change.readback.map(|raw| decode_limits(raw, change.units));
        self.field("register", change.register)
            .field("previous", hex(change.previous))
            .field("requested", hex(change.requested))
            .field("readback", change.readback.map(hex))
            .field("pl1_before_w", watts.pl1_before_w)
            .field("pl2_before_w", watts.pl2_before_w)
            .field("pl1_set_w", watts.pl1_set_w)
            .field("pl2_set_w", watts.pl2_set_w)
            .field("pl1_readback_w", readback.map(|d| d.pl1.watts))
            .field("pl2_readback_w", readback.map(|d| d.pl2.watts))
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields.get(key)
    }

    pub fn to_line(&self) -> String {
        Value::Object(self.fields.clone()).to_string()
    }

    /// Appends one line and flushes it to stable storage before returning.
    pub fn append_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let mut line = self.to_line();
        line.push('\n');
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        // One write call per line keeps concurrent appenders line-atomic.
        file.write_all(line.as_bytes())?;
        file.sync_all()
    }
}

/// Context for one audited register write.
#[derive(Debug, Clone, Copy)]
pub struct WriteContext<'a> {
    pub log: &'a Path,
    pub source: &'a str,
    pub command: &'a str,
    pub register: &'a str,
    pub units: Units,
    pub reason: Option<&'a str>,
    pub phase: Option<AuditPhase>,
    pub simulated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Unchanged,
    Changed,
}

/// Records one guarded register write: `write_unchanged`, or `write_attempt`
/// followed by `write_result`. The attempt is persisted before `write` runs, so an
/// unwritable audit log prevents the hardware write. `write` returns the readback.
pub fn audited_write(
    ctx: &WriteContext<'_>,
    previous: u64,
    requested: u64,
    write: impl FnOnce() -> Result<u64, String>,
) -> Result<WriteOutcome, String> {
    let record = |event: &str, readback: Option<u64>, outcome: &str| {
        let mut record = AuditRecord::new(ctx.source, event)
            .field("command", ctx.command)
            .register_change(&RegisterChange {
                register: ctx.register,
                units: ctx.units,
                previous,
                requested,
                readback,
            })
            .field("outcome", outcome);
        if let Some(reason) = ctx.reason {
            record = record.field("reason", sanitize_label(reason));
        }
        if let Some(phase) = ctx.phase {
            record = record.field("phase", phase.as_str());
        }
        if ctx.simulated {
            record = record.field("simulated", true);
        }
        record
    };
    let persist = |record: AuditRecord| {
        record
            .append_to(ctx.log)
            .map_err(|e| format!("audit log write failed for {}: {e}", ctx.log.display()))
    };
    if previous == requested {
        persist(record("write_unchanged", Some(previous), "no_change"))?;
        return Ok(WriteOutcome::Unchanged);
    }
    persist(record("write_attempt", None, "pending"))?;
    match write() {
        Ok(readback) if readback == requested => {
            persist(record("write_result", Some(readback), "changed"))?;
            Ok(WriteOutcome::Changed)
        }
        Ok(readback) => {
            persist(record("write_result", Some(readback), "readback_mismatch"))?;
            Err("register readback did not match the requested value".into())
        }
        Err(error) => {
            persist(
                record("write_result", None, "driver_error")
                    .field("error", sanitize_text(&error, MAX_DETAIL_CHARS)),
            )?;
            Err(error)
        }
    }
}

/// One decoded audit line for tables and CSV. Older records leave watt columns empty.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AuditRow {
    pub line: usize,
    pub timestamp_utc: Option<String>,
    pub source: Option<String>,
    pub event: Option<String>,
    pub phase: Option<String>,
    pub reason: Option<String>,
    pub register: Option<String>,
    pub pl1_before_w: Option<f64>,
    pub pl2_before_w: Option<f64>,
    pub pl1_set_w: Option<f64>,
    pub pl2_set_w: Option<f64>,
    pub pl1_readback_w: Option<f64>,
    pub pl2_readback_w: Option<f64>,
    pub outcome: Option<String>,
    pub previous: Option<String>,
    pub requested: Option<String>,
    pub readback: Option<String>,
    pub simulated: bool,
    pub detail: Option<String>,
}

pub fn parse_line(line_number: usize, line: &str) -> Result<AuditRow, String> {
    let value: Value =
        serde_json::from_str(line).map_err(|e| format!("line {line_number}: invalid JSON: {e}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| format!("line {line_number}: record is not a JSON object"))?;
    let text = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .map(|value| sanitize_text(value, MAX_DETAIL_CHARS))
    };
    let number = |key: &str| object.get(key).and_then(Value::as_f64);
    let detail = ["error", "halt_reason", "backup"]
        .iter()
        .find_map(|key| text(key).map(|value| format!("{key}: {value}")));
    Ok(AuditRow {
        line: line_number,
        timestamp_utc: text("timestamp_utc"),
        source: text("source"),
        event: text("event"),
        phase: text("phase"),
        reason: text("reason"),
        register: text("register"),
        pl1_before_w: number("pl1_before_w"),
        pl2_before_w: number("pl2_before_w"),
        pl1_set_w: number("pl1_set_w"),
        pl2_set_w: number("pl2_set_w"),
        pl1_readback_w: number("pl1_readback_w"),
        pl2_readback_w: number("pl2_readback_w"),
        outcome: text("outcome"),
        previous: text("previous"),
        requested: text("requested"),
        readback: text("readback"),
        simulated: object.get("simulated").and_then(Value::as_bool) == Some(true),
        detail,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AuditLog {
    pub rows: Vec<AuditRow>,
    pub malformed: Vec<String>,
}

/// Reads a JSONL audit log. Malformed lines are reported, never silently dropped.
/// `last` keeps only the newest rows (and newest malformed-line reports).
pub fn read_log(reader: impl BufRead, last: Option<usize>) -> io::Result<AuditLog> {
    let keep = last.unwrap_or(usize::MAX);
    let mut rows = VecDeque::new();
    let mut malformed = VecDeque::new();
    for (index, line) in reader.split(b'\n').enumerate() {
        let bytes = line?;
        let text = String::from_utf8_lossy(&bytes);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        match parse_line(index + 1, text) {
            Ok(row) => push_bounded(&mut rows, row, keep),
            Err(problem) => push_bounded(&mut malformed, problem, keep),
        }
    }
    Ok(AuditLog {
        rows: rows.into(),
        malformed: malformed.into(),
    })
}

fn push_bounded<T>(items: &mut VecDeque<T>, item: T, keep: usize) {
    if keep == 0 {
        return;
    }
    if items.len() == keep {
        items.pop_front();
    }
    items.push_back(item);
}

pub fn hex(value: u64) -> String {
    format!("0x{value:016x}")
}

/// Formats watts with up to three decimals and without trailing zeros (55, 55.125).
pub fn format_watts(value: f64) -> String {
    let text = format!("{value:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

pub fn utc_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Removes control characters and bounds the length of untrusted labels.
pub fn sanitize_label(value: &str) -> String {
    sanitize_text(value, MAX_LABEL_CHARS)
}

/// Removes control characters and keeps at most `max_chars` characters.
pub fn sanitize_text(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rapl::with_power_limits;

    fn units() -> Units {
        Units::decode(0x000a_0e03)
    }

    fn temp_log(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rapl_core_audit_{}_{name}.jsonl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn context<'a>(log: &'a Path) -> WriteContext<'a> {
        WriteContext {
            log,
            source: "test",
            command: "msr-write",
            register: REGISTER_MSR_PKG_LIMIT,
            units: units(),
            reason: Some("unit\ttest"),
            phase: Some(AuditPhase::Apply),
            simulated: true,
        }
    }

    #[test]
    fn records_readable_watt_columns() {
        let before = with_power_limits(0, units(), 55, 157).unwrap();
        let after = with_power_limits(before, units(), 157, 157).unwrap();
        let log = temp_log("columns");
        let ctx = context(&log);
        assert_eq!(
            audited_write(&ctx, before, after, || Ok(after)),
            Ok(WriteOutcome::Changed)
        );
        let parsed = read_log(std::fs::read(&log).unwrap().as_slice(), None).unwrap();
        assert!(parsed.malformed.is_empty());
        assert_eq!(parsed.rows.len(), 2);
        let attempt = &parsed.rows[0];
        let result = &parsed.rows[1];
        assert_eq!(attempt.event.as_deref(), Some("write_attempt"));
        assert_eq!(attempt.outcome.as_deref(), Some("pending"));
        assert_eq!(result.event.as_deref(), Some("write_result"));
        assert_eq!(result.outcome.as_deref(), Some("changed"));
        assert_eq!(result.pl1_before_w, Some(55.0));
        assert_eq!(result.pl2_before_w, Some(157.0));
        assert_eq!(result.pl1_set_w, Some(157.0));
        assert_eq!(result.pl2_set_w, Some(157.0));
        assert_eq!(result.pl1_readback_w, Some(157.0));
        assert_eq!(result.phase.as_deref(), Some("apply"));
        assert_eq!(result.reason.as_deref(), Some("unit test"));
        assert!(result.simulated);
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn unchanged_value_skips_hardware_write() {
        let value = with_power_limits(0, units(), 55, 157).unwrap();
        let log = temp_log("unchanged");
        let outcome = audited_write(&context(&log), value, value, || panic!("no write expected"));
        assert_eq!(outcome, Ok(WriteOutcome::Unchanged));
        let parsed = read_log(std::fs::read(&log).unwrap().as_slice(), None).unwrap();
        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.rows[0].event.as_deref(), Some("write_unchanged"));
        assert_eq!(parsed.rows[0].pl1_set_w, Some(55.0));
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn failed_write_and_mismatch_are_recorded() {
        let before = with_power_limits(0, units(), 55, 157).unwrap();
        let after = with_power_limits(before, units(), 50, 120).unwrap();
        let log = temp_log("failures");
        let ctx = context(&log);
        assert!(audited_write(&ctx, before, after, || Err("boom".into())).is_err());
        assert!(audited_write(&ctx, before, after, || Ok(before)).is_err());
        let parsed = read_log(std::fs::read(&log).unwrap().as_slice(), None).unwrap();
        let outcomes: Vec<_> = parsed
            .rows
            .iter()
            .map(|row| row.outcome.clone().unwrap())
            .collect();
        assert_eq!(
            outcomes,
            ["pending", "driver_error", "pending", "readback_mismatch"]
        );
        assert_eq!(parsed.rows[1].detail.as_deref(), Some("error: boom"));
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn unwritable_log_prevents_the_write() {
        let dir = temp_log("dir-as-file");
        std::fs::create_dir_all(&dir).unwrap();
        let mut called = false;
        let result = audited_write(&context(&dir), 1, 2, || {
            called = true;
            Ok(2)
        });
        assert!(result.is_err());
        assert!(!called);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_legacy_and_reports_malformed_lines() {
        let text = concat!(
            "{\"timestamp_utc\":\"2026-01-01T00:00:00.000Z\",\"event\":\"write_result\",",
            "\"register\":\"MSR_0x610\",\"previous\":\"0x1\",\"requested\":\"0x2\",\"outcome\":\"changed\"}\n",
            "not json\n",
            "\n",
            "[1,2]\n",
            "{\"event\":\"service_error\",\"error\":\"bridge\\u0007 timeout\"}\n"
        );
        let parsed = read_log(text.as_bytes(), None).unwrap();
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(parsed.rows[0].pl1_before_w, None);
        assert_eq!(parsed.rows[0].register.as_deref(), Some("MSR_0x610"));
        assert_eq!(
            parsed.rows[1].detail.as_deref(),
            Some("error: bridge  timeout")
        );
        assert_eq!(parsed.malformed.len(), 2);
        assert!(parsed.malformed[0].starts_with("line 2:"));
        let last = read_log(text.as_bytes(), Some(1)).unwrap();
        assert_eq!(last.rows.len(), 1);
        assert_eq!(last.rows[0].line, 5);
    }

    #[test]
    fn labels_are_sanitized_and_bounded() {
        assert_eq!(sanitize_label(" a\nb "), "a b");
        assert_eq!(sanitize_label(&"x".repeat(500)).len(), MAX_LABEL_CHARS);
        assert_eq!(AuditPhase::parse("rollback"), Some(AuditPhase::Rollback));
        assert_eq!(AuditPhase::parse("other"), None);
    }

    #[test]
    fn watts_are_formatted_without_trailing_zeros() {
        assert_eq!(format_watts(157.0), "157");
        assert_eq!(format_watts(55.125), "55.125");
        assert_eq!(format_watts(42.5), "42.5");
        assert_eq!(format_watts(0.0), "0");
    }
}
