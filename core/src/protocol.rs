//! Text protocol of `rapl-bridge info` (version 2): formatted by the bridge,
//! parsed by the CLI. One `key=value` pair per line; unknown keys are ignored.
use crate::audit::{hex, sanitize_label};
use crate::parse_hex_u64;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Text protocol version of `rapl-bridge info`.
pub const PROTOCOL_VERSION: u32 = 2;

/// CPU identity and driver capability gates reported by the backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Identity {
    pub vendor: String,
    pub model: String,
    pub cpu_family: u32,
    pub cpu_model: u32,
    pub cpu_stepping: u32,
    pub packages: u32,
    /// The driver's own CPU gate (vendor, family/model and a single package).
    pub cpu_supported: bool,
    /// The driver can read MCHBAR+0x59A0.
    pub mmio_readable: bool,
    /// The MCHBAR+0x59A0 layout was validated for this platform.
    pub mmio_verified: bool,
    /// The driver accepts register writes.
    pub live_writes_enabled: bool,
    #[serde(serialize_with = "serialize_optional_hex")]
    pub mchbar_base: Option<u64>,
    /// Set by the simulation backend; never part of the bridge protocol.
    pub simulated: bool,
}

fn serialize_optional_hex<S: serde::Serializer>(
    value: &Option<u64>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => serializer.serialize_str(&hex(*value)),
        None => serializer.serialize_none(),
    }
}

/// Formats the `info` response. `simulated` is not transmitted.
pub fn format_info(id: &Identity) -> String {
    let label = |value: &str| {
        let value = sanitize_label(value);
        if value.is_empty() {
            "unknown".to_owned()
        } else {
            value
        }
    };
    let mut text = String::new();
    let mut line = |key: &str, value: String| {
        let _ = writeln!(text, "{key}={value}");
    };
    line("protocol", PROTOCOL_VERSION.to_string());
    line("vendor", label(&id.vendor));
    line("model", label(&id.model));
    line("cpu_family", format!("0x{:x}", id.cpu_family));
    line("cpu_model", format!("0x{:x}", id.cpu_model));
    line("cpu_stepping", format!("0x{:x}", id.cpu_stepping));
    line("packages", id.packages.to_string());
    line("cpu_supported", id.cpu_supported.to_string());
    line("mmio_readable", id.mmio_readable.to_string());
    line("mmio_59a0_64bit_verified", id.mmio_verified.to_string());
    line("live_writes_enabled", id.live_writes_enabled.to_string());
    line(
        "mchbar_base",
        id.mchbar_base.map_or("unavailable".into(), hex),
    );
    text
}

/// Parses the `info` response. Unknown keys are ignored for forward compatibility;
/// missing, duplicate or inconsistent fields are rejected.
pub fn parse_info(text: &str) -> Result<Identity, String> {
    let mut fields = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("Malformed bridge info line: {}", sanitize_label(line)))?;
        if fields.insert(key.trim(), value.trim()).is_some() {
            return Err(format!(
                "Duplicate bridge info field: {}",
                sanitize_label(key.trim())
            ));
        }
    }
    let get = |key: &str| -> Result<&str, String> {
        fields
            .get(key)
            .copied()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("Missing bridge info field: {key}"))
    };
    let protocol = get("protocol")?;
    if protocol != PROTOCOL_VERSION.to_string() {
        return Err(format!(
            "Unsupported bridge protocol {} (expected protocol={PROTOCOL_VERSION})",
            sanitize_label(protocol)
        ));
    }
    let flag = |key: &str| -> Result<bool, String> {
        match get(key)? {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(format!(
                "Invalid boolean for {key}: {}",
                sanitize_label(other)
            )),
        }
    };
    let hex32 = |key: &str| -> Result<u32, String> {
        let value = parse_hex_u64(get(key)?).map_err(|e| format!("{key}: {e}"))?;
        u32::try_from(value).map_err(|_| format!("{key} is out of range"))
    };
    let packages = get("packages")?
        .parse::<u32>()
        .map_err(|_| "Invalid bridge info field: packages".to_string())?;
    let mchbar_base = match get("mchbar_base")? {
        "unavailable" => None,
        value => Some(parse_hex_u64(value).map_err(|e| format!("mchbar_base: {e}"))?),
    };
    let identity = Identity {
        vendor: sanitize_label(get("vendor")?),
        model: sanitize_label(get("model")?),
        cpu_family: hex32("cpu_family")?,
        cpu_model: hex32("cpu_model")?,
        cpu_stepping: hex32("cpu_stepping")?,
        packages,
        cpu_supported: flag("cpu_supported")?,
        mmio_readable: flag("mmio_readable")?,
        mmio_verified: flag("mmio_59a0_64bit_verified")?,
        live_writes_enabled: flag("live_writes_enabled")?,
        mchbar_base,
        simulated: false,
    };
    if identity.mmio_verified && !identity.mmio_readable {
        return Err("Inconsistent bridge info: MMIO layout verified but MMIO not readable".into());
    }
    if identity.mmio_readable && identity.mchbar_base.is_none() {
        return Err("Inconsistent bridge info: MMIO readable without an MCHBAR base".into());
    }
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = "protocol=2\nvendor=GenuineIntel\nmodel=Intel(R) Core(TM) i9-13950HX\n\
        cpu_family=0x6\ncpu_model=0xb7\ncpu_stepping=0x1\npackages=1\ncpu_supported=true\n\
        mmio_readable=true\nmmio_59a0_64bit_verified=false\nlive_writes_enabled=false\n\
        mchbar_base=0x00000000fed10000\n";

    fn replace(key: &str, value: &str) -> String {
        INFO.lines()
            .map(|line| {
                if line.starts_with(&format!("{key}=")) {
                    format!("{key}={value}")
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn parses_protocol_2_identity() {
        let identity = parse_info(&format!("{INFO}future_key=ignored\n")).unwrap();
        assert_eq!(identity.cpu_family, 6);
        assert_eq!(identity.cpu_model, 0xb7);
        assert_eq!(identity.packages, 1);
        assert!(identity.cpu_supported && identity.mmio_readable);
        assert!(!identity.mmio_verified && !identity.live_writes_enabled);
        assert_eq!(identity.mchbar_base, Some(0xfed1_0000));
        assert!(!identity.simulated);
    }

    #[test]
    fn format_and_parse_round_trip() {
        let identity = parse_info(INFO).unwrap();
        assert_eq!(format_info(&identity), INFO);
        let sparse = Identity {
            vendor: "\n".into(),
            model: "evil\nprotocol=1".into(),
            mmio_readable: false,
            mchbar_base: None,
            ..identity
        };
        let parsed = parse_info(&format_info(&sparse)).unwrap();
        assert_eq!(parsed.vendor, "unknown");
        assert_eq!(parsed.model, "evil protocol=1");
        assert_eq!(parsed.mchbar_base, None);
    }

    #[test]
    fn rejects_bad_identity() {
        let cases = [
            replace("protocol", "1"),
            replace("cpu_model", "183"),
            replace("cpu_supported", "yes"),
            replace("packages", "-1"),
            replace("mchbar_base", "unavailable"),
            replace("mmio_readable", "false").replace("verified=false", "verified=true"),
            INFO.replace("vendor=GenuineIntel\n", ""),
            format!("{INFO}packages=2\n"),
            format!("{INFO}garbage\n"),
        ];
        for case in cases {
            assert!(parse_info(&case).is_err(), "{case}");
        }
    }
}
