//! `server.properties` — the dedicated server's primary config.
//!
//! Vanilla parses this with `java.util.Properties` and writes it back on every
//! load, filling in any missing key with its default. We mirror that round-trip
//! behaviour: load the file (if present), keep unknown keys verbatim, expose
//! typed accessors for the values we actually honour, and `store` the full
//! canonical key set back so the file ends up complete and self-documenting —
//! exactly like a freshly generated vanilla `server.properties`.
//!
//! Keys, defaults and serialized forms are taken from the decompiled 26.2
//! `DedicatedServerProperties`. Java's `Properties` is a `Hashtable`, so its
//! on-disk key order is unspecified scramble; we instead write a stable, logical
//! order, which is an equally valid properties file.

use std::collections::HashMap;
use std::path::Path;

use tracing::{info, warn};

/// The full canonical key set with vanilla defaults, in the order we write them.
/// Every key the 26.2 server persists appears here (legacy/removed keys such as
/// `announce-player-achievements` and `resource-pack-hash` are intentionally
/// absent — vanilla strips them on load rather than writing them back).
const DEFAULTS: &[(&str, &str)] = &[
    // Identity / networking
    ("online-mode", "true"),
    ("prevent-proxy-connections", "false"),
    ("server-ip", ""),
    ("server-port", "25565"),
    ("use-native-transport", "true"),
    ("network-compression-threshold", "256"),
    ("rate-limit", "0"),
    ("accepts-transfers", "false"),
    // Presentation
    ("motd", "A Minecraft Server"),
    ("enable-status", "true"),
    ("hide-online-players", "false"),
    ("enable-code-of-conduct", "false"),
    ("bug-report-link", ""),
    // Gameplay
    ("gamemode", "survival"),
    ("force-gamemode", "false"),
    ("difficulty", "easy"),
    ("hardcore", "false"),
    ("allow-flight", "false"),
    ("spawn-protection", "16"),
    ("max-players", "20"),
    ("view-distance", "10"),
    ("simulation-distance", "10"),
    ("entity-broadcast-range-percentage", "100"),
    ("player-idle-timeout", "0"),
    ("pause-when-empty-seconds", "60"),
    ("max-chained-neighbor-updates", "1000000"),
    // World
    ("level-name", "world"),
    ("level-seed", ""),
    ("level-type", "minecraft:normal"),
    ("generate-structures", "true"),
    ("generator-settings", "{}"),
    ("max-world-size", "29999984"),
    ("initial-enabled-packs", "vanilla"),
    ("initial-disabled-packs", ""),
    // Permissions / access control
    ("white-list", "false"),
    ("enforce-whitelist", "false"),
    ("op-permission-level", "4"),
    ("function-permission-level", "3"),
    ("enforce-secure-profile", "true"),
    ("log-ips", "true"),
    // Resource pack
    ("resource-pack", ""),
    ("resource-pack-id", ""),
    ("resource-pack-sha1", ""),
    ("resource-pack-prompt", ""),
    ("require-resource-pack", "false"),
    // Spam / rate limiting
    ("command-spam-threshold-seconds", "10"),
    ("chat-spam-threshold-seconds", "10"),
    // Persistence / performance
    ("sync-chunk-writes", "true"),
    ("region-file-compression", "deflate"),
    ("max-tick-time", "60000"),
    ("status-heartbeat-interval", "0"),
    // RCON / query (legacy remote consoles)
    ("enable-rcon", "false"),
    ("rcon.port", "25575"),
    ("rcon.password", ""),
    ("broadcast-rcon-to-ops", "true"),
    ("broadcast-console-to-ops", "true"),
    ("enable-query", "false"),
    ("query.port", "25565"),
    // Management server (26.x JSON-RPC)
    ("management-server-enabled", "false"),
    ("management-server-host", "localhost"),
    ("management-server-port", "0"),
    ("management-server-secret", ""),
    ("management-server-tls-enabled", "true"),
    ("management-server-tls-keystore", ""),
    ("management-server-tls-keystore-password", ""),
    ("management-server-allowed-origins", ""),
    // Monitoring / filtering
    ("enable-jmx-monitoring", "false"),
    ("text-filtering-config", ""),
    ("text-filtering-version", "0"),
];

/// Parsed `server.properties`. Holds every key as a string (typed parsing happens
/// in the accessors), so unknown keys survive a load/store round-trip untouched.
#[derive(Debug, Clone)]
pub struct ServerProperties {
    values: HashMap<String, String>,
}

impl Default for ServerProperties {
    fn default() -> Self {
        let values = DEFAULTS
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Self { values }
    }
}

impl ServerProperties {
    /// Load `server.properties` from `path`, creating it (with the full default
    /// key set) if absent. Missing keys are backfilled with their defaults and
    /// the completed file is written back, mirroring vanilla. A parse/IO failure
    /// is logged and falls back to all-defaults rather than aborting startup.
    pub fn load_or_create(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let mut props = match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                info!(file = %path.display(), "no server.properties; writing defaults");
                Self::default()
            }
            Err(e) => {
                warn!(file = %path.display(), error = %e, "failed to read server.properties; using defaults");
                Self::default()
            }
        };
        // Backfill any key the file was missing so `store` emits a complete file.
        for (k, v) in DEFAULTS {
            props.values.entry(k.to_string()).or_insert_with(|| v.to_string());
        }
        if let Err(e) = props.store(path) {
            warn!(file = %path.display(), error = %e, "failed to write server.properties");
        }
        props
    }

    /// Parse the `key=value` body of a properties file. See [`parse_properties`].
    /// Unknown keys are retained so they survive a load/store round-trip.
    fn parse(text: &str) -> Self {
        Self {
            values: parse_properties(text),
        }
    }

    /// Write the canonical key set (then any extra unknown keys, sorted) back to
    /// `path` in `key=value` form with a header comment, matching how vanilla
    /// regenerates the file.
    pub fn store(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let mut out = String::from("#Minecraft server properties\n#Generated by Vela\n");
        let mut seen = std::collections::HashSet::new();
        for (key, default) in DEFAULTS {
            if !seen.insert(*key) {
                continue; // guard against accidental duplicate canonical keys
            }
            let value = self.values.get(*key).map(String::as_str).unwrap_or(default);
            out.push_str(&format!("{}={}\n", key, escape_value(value)));
        }
        // Preserve any keys the operator added that we don't know about.
        let mut extras: Vec<_> = self
            .values
            .keys()
            .filter(|k| !DEFAULTS.iter().any(|(d, _)| *d == k.as_str()))
            .collect();
        extras.sort();
        for key in extras {
            out.push_str(&format!("{}={}\n", key, escape_value(&self.values[key])));
        }
        std::fs::write(path, out)
    }

    // --- typed accessors for the values we currently honour ---

    fn str(&self, key: &str) -> &str {
        self.values.get(key).map(String::as_str).unwrap_or("")
    }

    fn int(&self, key: &str, default: i32) -> i32 {
        self.values.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
    }

    fn bool(&self, key: &str, default: bool) -> bool {
        // Java `Boolean.valueOf`: only the literal "true" (case-insensitive) is true.
        self.values
            .get(key)
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(default)
    }

    pub fn server_ip(&self) -> &str {
        self.str("server-ip")
    }

    pub fn server_port(&self) -> u16 {
        self.int("server-port", 25565).clamp(0, u16::MAX as i32) as u16
    }

    pub fn motd(&self) -> &str {
        self.str("motd")
    }

    pub fn max_players(&self) -> i32 {
        self.int("max-players", 20)
    }

    pub fn enable_status(&self) -> bool {
        self.bool("enable-status", true)
    }

    pub fn online_mode(&self) -> bool {
        self.bool("online-mode", true)
    }

    /// `prevent-proxy-connections`: when set, the online-mode `hasJoined` call
    /// pins authentication to the client's source IP (`DedicatedServerProperties`).
    pub fn prevent_proxy_connections(&self) -> bool {
        self.bool("prevent-proxy-connections", false)
    }

    /// `network-compression-threshold`: the packet size (uncompressed bytes) at
    /// or above which a frame is zlib-deflated. A value `< 0` disables
    /// compression entirely (vanilla `getCompressionThreshold`); the default 256
    /// matches `DedicatedServerProperties`.
    pub fn network_compression_threshold(&self) -> i32 {
        self.int("network-compression-threshold", 256)
    }

    pub fn hardcore(&self) -> bool {
        self.bool("hardcore", false)
    }

    /// `view-distance`, clamped to the protocol's sane 2..=32 chunk range.
    pub fn view_distance(&self) -> i32 {
        self.int("view-distance", 10).clamp(2, 32)
    }

    /// `simulation-distance`, clamped to the same range as the view distance.
    pub fn simulation_distance(&self) -> i32 {
        self.int("simulation-distance", 10).clamp(2, 32)
    }

    /// The default game mode as the wire id (0=survival, 1=creative, 2=adventure,
    /// 3=spectator). Accepts the vanilla name or numeric form.
    pub fn gamemode(&self) -> u8 {
        match self.str("gamemode") {
            "0" | "survival" => 0,
            "1" | "creative" => 1,
            "2" | "adventure" => 2,
            "3" | "spectator" => 3,
            other => {
                warn!(value = other, "unknown gamemode; defaulting to survival");
                0
            }
        }
    }
}

/// Parse the body of a `java.util.Properties` file into key/value pairs, shared
/// by `server.properties` and `eula.txt` (both are Properties files in vanilla).
///
/// Lines that are blank or start with `#`/`!` are comments. The key is split from
/// the value on the first unescaped `=` or `:`; a backslash escapes the next char
/// (so `\=`, `\:`, `\\`, `\n` round-trip). Java's line-continuation (trailing
/// `\`) and `\uXXXX` escapes are intentionally unsupported — they never appear in
/// a real `server.properties`/`eula.txt`.
pub(super) fn parse_properties(text: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    for raw in text.lines() {
        let line = raw.trim_start();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        // Split on the first unescaped '=' or ':'.
        let mut key = String::new();
        let mut chars = line.chars().peekable();
        let mut split = false;
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(&n) = chars.peek() {
                        key.push(unescape(n));
                        chars.next();
                    }
                }
                '=' | ':' => {
                    split = true;
                    break;
                }
                _ => key.push(c),
            }
        }
        if !split {
            continue;
        }
        let value: String = chars.collect();
        values.insert(key.trim_end().to_string(), unescape_value(value.trim_start()));
    }
    values
}

/// Map an escaped char to its literal (`\n` -> newline, `\t` -> tab, else self).
fn unescape(c: char) -> char {
    match c {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        'f' => '\u{000C}',
        other => other,
    }
}

/// Unescape a value body (handles `\\`, `\n`, `\t`, `\=`, `\:`, …).
fn unescape_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(unescape(n));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Escape a value for writing: backslash and control chars only. Spaces, `=`,
/// `:` inside a value need no escaping (Java only escapes those in keys / leading
/// position), so the output stays readable for typical MOTD/path values.
fn escape_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_parse() {
        let defaults = ServerProperties::default();
        // Render to text, parse it back, and confirm the honoured accessors agree.
        let mut tmp = std::env::temp_dir();
        tmp.push(format!("vela-props-{}.properties", std::process::id()));
        defaults.store(&tmp).unwrap();
        let text = std::fs::read_to_string(&tmp).unwrap();
        let parsed = ServerProperties::parse(&text);
        std::fs::remove_file(&tmp).ok();

        assert_eq!(parsed.server_port(), 25565);
        assert_eq!(parsed.max_players(), 20);
        assert_eq!(parsed.motd(), "A Minecraft Server");
        assert_eq!(parsed.view_distance(), 10);
        assert!(parsed.enable_status());
        assert!(parsed.online_mode());
        assert_eq!(parsed.gamemode(), 0);
    }

    #[test]
    fn unknown_keys_survive_round_trip() {
        let text = "custom-plugin-key=hello\nmax-players=50\n";
        let props = ServerProperties::parse(text);
        assert_eq!(props.max_players(), 50);
        let mut tmp = std::env::temp_dir();
        tmp.push(format!("vela-props-extra-{}.properties", std::process::id()));
        // store must include the unknown key and the overridden known key.
        let mut full = props;
        for (k, v) in DEFAULTS {
            full.values.entry(k.to_string()).or_insert_with(|| v.to_string());
        }
        full.store(&tmp).unwrap();
        let out = std::fs::read_to_string(&tmp).unwrap();
        std::fs::remove_file(&tmp).ok();
        assert!(out.contains("custom-plugin-key=hello"));
        assert!(out.contains("max-players=50"));
    }

    #[test]
    fn boolean_parsing_matches_java() {
        let props = ServerProperties::parse("hardcore=TRUE\nonline-mode=yes\n");
        assert!(props.hardcore()); // "TRUE" -> true
        assert!(!props.online_mode()); // "yes" is not "true" -> false
    }
}
