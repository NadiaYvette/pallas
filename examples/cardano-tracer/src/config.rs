use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Top-level tracer configuration, matching the Haskell TracerConfig YAML format.
#[derive(Deserialize, Clone, Debug)]
pub struct TracerConfig {
    #[serde(rename = "networkMagic", default = "default_network_magic")]
    pub network_magic: u32,

    pub network: Network,

    /// How many TraceObjects to request per batch (default 100).
    #[serde(rename = "loRequestNum")]
    pub lo_request_num: Option<u16>,

    /// EKG metrics polling interval in seconds (default 1.0).
    #[serde(rename = "ekgRequestFreq")]
    pub ekg_request_freq: Option<f64>,

    /// Prometheus endpoint configuration.
    #[serde(rename = "hasPrometheus")]
    pub has_prometheus: Option<Endpoint>,

    /// EKG web endpoint configuration.
    #[serde(rename = "hasEKG")]
    pub has_ekg: Option<Endpoint>,

    /// One or more logging outputs.
    pub logging: Vec<LoggingParams>,

    /// Log rotation parameters.
    pub rotation: Option<RotationParams>,

    /// Tracer verbosity level.
    pub verbosity: Option<Verbosity>,
}

fn default_network_magic() -> u32 {
    764824073 // mainnet
}

impl TracerConfig {
    pub fn request_num(&self) -> u16 {
        self.lo_request_num.unwrap_or(100)
    }

    pub fn ekg_freq_secs(&self) -> f64 {
        self.ekg_request_freq.unwrap_or(1.0)
    }
}

/// Network connection mode.
///
/// AcceptAt: tracer listens (server), nodes connect to it.
/// ConnectTo: tracer connects (client) to one or more node addresses.
#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "tag", content = "contents")]
pub enum Network {
    AcceptAt(String),
    ConnectTo(Vec<String>),
}

/// A parsed network address: either a Unix socket path or a TCP host:port.
#[derive(Clone, Debug)]
pub enum Address {
    Unix(PathBuf),
    Tcp(String, u16),
}

impl Address {
    /// Parse an address string. If it contains a colon with a numeric port
    /// suffix, treat it as TCP; otherwise treat it as a Unix socket path.
    pub fn parse(s: &str) -> Self {
        if let Some(idx) = s.rfind(':') {
            let (host, port_str) = s.split_at(idx);
            if let Ok(port) = port_str[1..].parse::<u16>() {
                if !host.is_empty() && !host.contains('/') {
                    return Address::Tcp(host.to_string(), port);
                }
            }
        }
        Address::Unix(PathBuf::from(s))
    }
}

/// HTTP endpoint for Prometheus or EKG.
#[derive(Deserialize, Clone, Debug)]
pub struct Endpoint {
    #[serde(rename = "epHost")]
    pub host: String,
    #[serde(rename = "epPort")]
    pub port: u16,
}

/// Per-output logging parameters.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LoggingParams {
    #[serde(rename = "logRoot")]
    pub log_root: PathBuf,

    #[serde(rename = "logMode", default)]
    pub log_mode: LogMode,

    #[serde(rename = "logFormat", default)]
    pub log_format: LogFormat,
}

#[derive(Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub enum LogMode {
    FileMode,
    JournalMode,
}

impl Default for LogMode {
    fn default() -> Self {
        LogMode::FileMode
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub enum LogFormat {
    ForHuman,
    ForMachine,
}

impl Default for LogFormat {
    fn default() -> Self {
        LogFormat::ForMachine
    }
}

/// Log rotation parameters.
#[derive(Deserialize, Clone, Debug)]
pub struct RotationParams {
    /// How often to check for rotation, in seconds (default 60).
    #[serde(rename = "rpFrequencySecs", default = "default_rotation_freq")]
    pub frequency_secs: u32,

    /// Maximum log file size in bytes before rotation.
    #[serde(rename = "rpLogLimitBytes")]
    pub log_limit_bytes: u64,

    /// Maximum age in minutes (takes precedence over rpMaxAgeHours).
    #[serde(rename = "rpMaxAgeMinutes")]
    pub max_age_minutes: Option<u64>,

    /// Maximum age in hours (default 24). Used if rpMaxAgeMinutes is absent.
    #[serde(rename = "rpMaxAgeHours")]
    pub max_age_hours: Option<u64>,

    /// Minimum number of log files to keep, regardless of age.
    #[serde(rename = "rpKeepFilesNum")]
    pub keep_files_num: u32,
}

fn default_rotation_freq() -> u32 {
    60
}

impl RotationParams {
    /// Effective maximum age in minutes.
    pub fn max_age_minutes(&self) -> u64 {
        if let Some(m) = self.max_age_minutes {
            m
        } else if let Some(h) = self.max_age_hours {
            h * 60
        } else {
            24 * 60 // default: 24 hours
        }
    }
}

/// Tracer verbosity level.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Verbosity {
    Minimum,
    ErrorsOnly,
    Maximum,
}

impl Verbosity {
    /// Returns the minimum severity level (as a numeric index) that should be
    /// logged. Severity indices match the TraceObjects Severity enum:
    /// 0=Debug, 1=Info, 2=Notice, 3=Warning, 4=Error, 5=Critical, 6=Alert, 7=Emergency.
    pub fn min_severity_index(&self) -> u8 {
        match self {
            Verbosity::Maximum => 0,    // Debug and above
            Verbosity::Minimum => 3,    // Warning and above
            Verbosity::ErrorsOnly => 4, // Error and above
        }
    }
}

/// Load and parse a tracer configuration from a YAML file.
pub fn load_config(path: &Path) -> Result<TracerConfig, Box<dyn std::error::Error>> {
    let f = std::fs::File::open(path)?;
    let config: TracerConfig = serde_yaml::from_reader(f)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_config() {
        let yaml = r#"
networkMagic: 42
network:
  tag: AcceptAt
  contents: "/tmp/test.sock"
logging:
- logRoot: "/tmp/logs"
  logMode: FileMode
  logFormat: ForMachine
"#;
        let config: TracerConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.network_magic, 42);
        match &config.network {
            Network::AcceptAt(s) => assert_eq!(s, "/tmp/test.sock"),
            _ => panic!("Expected AcceptAt"),
        }
        assert_eq!(config.logging.len(), 1);
        assert_eq!(config.logging[0].log_root, PathBuf::from("/tmp/logs"));
        assert_eq!(config.logging[0].log_mode, LogMode::FileMode);
        assert_eq!(config.logging[0].log_format, LogFormat::ForMachine);
        assert!(config.rotation.is_none());
        assert!(config.has_prometheus.is_none());
        assert_eq!(config.request_num(), 100); // default
        assert!((config.ekg_freq_secs() - 1.0).abs() < f64::EPSILON); // default
    }

    #[test]
    fn test_parse_full_config() {
        let yaml = r#"
networkMagic: 764824073
network:
  tag: ConnectTo
  contents:
    - "/tmp/node1.sock"
    - "host1:3001"
loRequestNum: 200
ekgRequestFreq: 2.5
hasPrometheus:
  epHost: "0.0.0.0"
  epPort: 9090
hasEKG:
  epHost: "127.0.0.1"
  epPort: 8080
logging:
- logRoot: "/tmp/logs1"
  logMode: FileMode
  logFormat: ForMachine
- logRoot: "/tmp/logs2"
  logMode: FileMode
  logFormat: ForHuman
rotation:
  rpFrequencySecs: 30
  rpLogLimitBytes: 10000000
  rpMaxAgeHours: 2
  rpKeepFilesNum: 5
verbosity: Maximum
"#;
        let config: TracerConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.network_magic, 764824073);
        match &config.network {
            Network::ConnectTo(addrs) => {
                assert_eq!(addrs.len(), 2);
                assert_eq!(addrs[0], "/tmp/node1.sock");
                assert_eq!(addrs[1], "host1:3001");
            }
            _ => panic!("Expected ConnectTo"),
        }
        assert_eq!(config.request_num(), 200);
        assert!((config.ekg_freq_secs() - 2.5).abs() < f64::EPSILON);
        assert_eq!(config.has_prometheus.as_ref().unwrap().host, "0.0.0.0");
        assert_eq!(config.has_prometheus.as_ref().unwrap().port, 9090);
        assert_eq!(config.has_ekg.as_ref().unwrap().port, 8080);
        assert_eq!(config.logging.len(), 2);
        assert_eq!(config.logging[1].log_format, LogFormat::ForHuman);
        let rot = config.rotation.as_ref().unwrap();
        assert_eq!(rot.frequency_secs, 30);
        assert_eq!(rot.log_limit_bytes, 10000000);
        assert_eq!(rot.keep_files_num, 5);
        assert_eq!(config.verbosity, Some(Verbosity::Maximum));
    }

    #[test]
    fn test_parse_default_network_magic() {
        let yaml = r#"
network:
  tag: AcceptAt
  contents: "/tmp/test.sock"
logging:
- logRoot: "/tmp/logs"
"#;
        let config: TracerConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.network_magic, 764824073); // mainnet default
    }

    #[test]
    fn test_address_parse_unix() {
        match Address::parse("/tmp/test.sock") {
            Address::Unix(p) => assert_eq!(p, PathBuf::from("/tmp/test.sock")),
            _ => panic!("Expected Unix"),
        }
        match Address::parse("/run/cardano/node.socket") {
            Address::Unix(p) => assert_eq!(p, PathBuf::from("/run/cardano/node.socket")),
            _ => panic!("Expected Unix for path with slashes"),
        }
        // A path with colon but containing slashes should be Unix
        match Address::parse("/tmp/host:1234/test.sock") {
            Address::Unix(_) => {}
            _ => panic!("Expected Unix for path with colon in directory"),
        }
    }

    #[test]
    fn test_address_parse_tcp() {
        match Address::parse("localhost:3001") {
            Address::Tcp(host, port) => {
                assert_eq!(host, "localhost");
                assert_eq!(port, 3001);
            }
            _ => panic!("Expected Tcp"),
        }
        match Address::parse("192.168.1.1:9090") {
            Address::Tcp(host, port) => {
                assert_eq!(host, "192.168.1.1");
                assert_eq!(port, 9090);
            }
            _ => panic!("Expected Tcp"),
        }
    }

    #[test]
    fn test_rotation_max_age_precedence() {
        // minutes takes precedence
        let rot = RotationParams {
            frequency_secs: 60,
            log_limit_bytes: 1000,
            max_age_minutes: Some(120),
            max_age_hours: Some(5),
            keep_files_num: 3,
        };
        assert_eq!(rot.max_age_minutes(), 120);

        // hours used when minutes absent
        let rot2 = RotationParams {
            frequency_secs: 60,
            log_limit_bytes: 1000,
            max_age_minutes: None,
            max_age_hours: Some(3),
            keep_files_num: 3,
        };
        assert_eq!(rot2.max_age_minutes(), 180);

        // default 24h when both absent
        let rot3 = RotationParams {
            frequency_secs: 60,
            log_limit_bytes: 1000,
            max_age_minutes: None,
            max_age_hours: None,
            keep_files_num: 3,
        };
        assert_eq!(rot3.max_age_minutes(), 1440);
    }

    #[test]
    fn test_log_format_defaults() {
        assert_eq!(LogMode::default(), LogMode::FileMode);
        assert_eq!(LogFormat::default(), LogFormat::ForMachine);
    }

    #[test]
    fn test_verbosity_variants() {
        let yaml = r#"
network:
  tag: AcceptAt
  contents: "/tmp/test.sock"
logging:
- logRoot: "/tmp/logs"
verbosity: ErrorsOnly
"#;
        let config: TracerConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.verbosity, Some(Verbosity::ErrorsOnly));
    }
}
