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

/// Load and parse a tracer configuration from a YAML file.
pub fn load_config(path: &Path) -> Result<TracerConfig, Box<dyn std::error::Error>> {
    let f = std::fs::File::open(path)?;
    let config: TracerConfig = serde_yaml::from_reader(f)?;
    Ok(config)
}
