use crate::cbor_json;
use crate::config::{LogFormat, LogMode, LoggingParams, RotationParams};
use pallas::network::miniprotocols::traceobjects::TraceObject;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

const LOG_PREFIX: &str = "node-";
const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H-%M-%S";

fn log_extension(format: &LogFormat) -> &str {
    match format {
        LogFormat::ForHuman => ".log",
        LogFormat::ForMachine => ".json",
    }
}

fn symlink_name(format: &LogFormat) -> String {
    format!("node{}", log_extension(format))
}

/// Generate a timestamped log file name: `node-YYYY-MM-DDTHH-MM-SS.json`
fn log_file_name(format: &LogFormat) -> String {
    let now = chrono::Utc::now();
    format!(
        "{}{}{}",
        LOG_PREFIX,
        now.format(TIMESTAMP_FORMAT),
        log_extension(format)
    )
}

/// Key for looking up a log file handle: (node_name, logging_params_index).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct HandleKey {
    node_name: String,
    params_idx: usize,
}

/// An open log file handle with its path.
struct LogHandle {
    writer: BufWriter<File>,
    path: PathBuf,
}

/// Manages all log file outputs across all logging configurations and nodes.
pub struct LogManager {
    params: Vec<LoggingParams>,
    rotation: Option<RotationParams>,
    handles: HashMap<HandleKey, LogHandle>,
}

pub type SharedLogManager = Arc<Mutex<LogManager>>;

impl LogManager {
    pub fn new(params: &[LoggingParams], rotation: Option<RotationParams>) -> Self {
        LogManager {
            params: params.to_vec(),
            rotation,
            handles: HashMap::new(),
        }
    }

    /// Write a batch of trace objects to all configured log outputs for a node.
    pub fn write_trace_objects(&mut self, node_name: &str, objects: &[TraceObject]) {
        for (idx, lp) in self.params.clone().iter().enumerate() {
            if lp.log_mode == LogMode::JournalMode {
                // JournalMode not yet implemented; skip.
                continue;
            }

            let key = HandleKey {
                node_name: node_name.to_string(),
                params_idx: idx,
            };

            // Get or create the log file handle.
            if !self.handles.contains_key(&key) {
                match self.create_log_file(node_name, lp) {
                    Ok(handle) => {
                        self.handles.insert(key.clone(), handle);
                    }
                    Err(e) => {
                        error!("Failed to create log file for {}: {:?}", node_name, e);
                        continue;
                    }
                }
            }

            if let Some(handle) = self.handles.get_mut(&key) {
                for obj in objects {
                    let line = match lp.log_format {
                        LogFormat::ForMachine => cbor_json::trace_object_to_log_line(obj),
                        LogFormat::ForHuman => cbor_json::trace_object_to_human_line(obj),
                    };
                    if let Err(e) = writeln!(handle.writer, "{}", line) {
                        error!("Failed to write log line: {:?}", e);
                    }
                }
                if let Err(e) = handle.writer.flush() {
                    error!("Failed to flush log: {:?}", e);
                }
            }
        }
    }

    /// Create a new timestamped log file and update the symlink.
    fn create_log_file(
        &self,
        node_name: &str,
        params: &LoggingParams,
    ) -> Result<LogHandle, Box<dyn std::error::Error>> {
        let node_dir = params.log_root.join(node_name);
        fs::create_dir_all(&node_dir)?;

        let file_name = log_file_name(&params.log_format);
        let file_path = node_dir.join(&file_name);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)?;

        // Write an empty initial line to satisfy the Haskell test harness
        // expectation (lineLength - 1).
        let mut writer = BufWriter::new(file);
        writeln!(writer)?;
        writer.flush()?;

        // Update symlink atomically.
        let link_name = symlink_name(&params.log_format);
        update_symlink_atomically(&node_dir, &file_name, &link_name)?;

        info!("Created log file: {:?}", file_path);

        Ok(LogHandle {
            writer,
            path: file_path,
        })
    }

    /// Check all open log files for rotation.
    pub fn check_rotation(&mut self, rotation: &RotationParams) {
        // Collect keys that need rotation.
        let mut to_rotate = Vec::new();

        for (key, handle) in &self.handles {
            if let Ok(meta) = fs::metadata(&handle.path) {
                if meta.len() >= rotation.log_limit_bytes {
                    to_rotate.push(key.clone());
                }
            }
        }

        // Rotate each file that exceeded the size limit.
        for key in to_rotate {
            let params = &self.params[key.params_idx].clone();

            // Close old handle by removing it.
            self.handles.remove(&key);

            // Create new file.
            match self.create_log_file(&key.node_name, params) {
                Ok(handle) => {
                    self.handles.insert(key, handle);
                }
                Err(e) => {
                    error!("Failed to rotate log: {:?}", e);
                }
            }
        }

        // Age-based cleanup for all node directories.
        let max_age = chrono::Duration::minutes(rotation.max_age_minutes() as i64);
        let now = chrono::Utc::now();

        for lp in &self.params {
            if lp.log_mode != LogMode::FileMode {
                continue;
            }
            let ext = log_extension(&lp.log_format);

            // Scan all node subdirectories under this logRoot.
            let Ok(entries) = fs::read_dir(&lp.log_root) else {
                continue;
            };
            for entry in entries.flatten() {
                let node_dir = entry.path();
                if !node_dir.is_dir() {
                    continue;
                }
                cleanup_old_logs(&node_dir, ext, max_age, now, rotation.keep_files_num);
            }
        }
    }
}

/// Delete old log files that exceed the max age, keeping at least `keep_num` files.
fn cleanup_old_logs(
    dir: &Path,
    ext: &str,
    max_age: chrono::Duration,
    now: chrono::DateTime<chrono::Utc>,
    keep_num: u32,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    // Collect log files (not symlinks) sorted by name (oldest first).
    let mut log_files: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_name()?.to_str()?;
            if name.starts_with(LOG_PREFIX) && name.ends_with(ext) && !path.is_symlink() {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    log_files.sort();

    let total = log_files.len();
    if total <= keep_num as usize {
        return;
    }

    // Check all files except the newest one (current log).
    for path in &log_files[..total - 1] {
        if log_files.len() <= keep_num as usize {
            break;
        }

        // Extract timestamp from filename.
        if let Some(ts) = extract_timestamp_from_filename(path, ext) {
            if now - ts > max_age {
                if let Err(e) = fs::remove_file(path) {
                    error!("Failed to delete old log {:?}: {:?}", path, e);
                }
            }
        }
    }
}

/// Parse a timestamp from a log filename like `node-2024-01-15T14-30-00.json`.
fn extract_timestamp_from_filename(
    path: &Path,
    ext: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let name = path.file_name()?.to_str()?;
    let ts_str = name
        .strip_prefix(LOG_PREFIX)?
        .strip_suffix(ext)?;
    chrono::NaiveDateTime::parse_from_str(ts_str, TIMESTAMP_FORMAT)
        .ok()
        .map(|ndt| ndt.and_utc())
}

/// Atomically update a symlink to point to a new target file.
fn update_symlink_atomically(
    dir: &Path,
    target_name: &str,
    link_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let link_path = dir.join(link_name);
    let tmp_path = dir.join(format!("{}.tmp", link_name));

    // Remove any existing temp symlink.
    let _ = fs::remove_file(&tmp_path);

    // Create temp symlink -> target.
    #[cfg(unix)]
    std::os::unix::fs::symlink(target_name, &tmp_path)?;

    // Atomically rename temp -> actual link.
    fs::rename(&tmp_path, &link_path)?;

    Ok(())
}

/// Background task that periodically checks for log rotation.
pub async fn run_rotation_loop(
    log_manager: SharedLogManager,
    rotation: RotationParams,
    shutdown: CancellationToken,
) {
    let mut interval =
        tokio::time::interval(std::time::Duration::from_secs(rotation.frequency_secs as u64));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let mut mgr = log_manager.lock().await;
                mgr.check_rotation(&rotation);
            }
            _ = shutdown.cancelled() => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LogFormat, LogMode, LoggingParams, RotationParams};

    fn make_logging_params(dir: &std::path::Path, format: LogFormat) -> LoggingParams {
        LoggingParams {
            log_root: dir.to_path_buf(),
            log_mode: LogMode::FileMode,
            log_format: format,
        }
    }

    #[test]
    fn test_log_file_name_format() {
        let name = log_file_name(&LogFormat::ForMachine);
        assert!(name.starts_with("node-"));
        assert!(name.ends_with(".json"));
        // Should contain date separator
        assert!(name.contains('T'));
        // Length should be: "node-" (5) + "YYYY-MM-DDTHH-MM-SS" (19) + ".json" (5) = 29
        assert_eq!(name.len(), 29);
    }

    #[test]
    fn test_log_file_name_human() {
        let name = log_file_name(&LogFormat::ForHuman);
        assert!(name.starts_with("node-"));
        assert!(name.ends_with(".log"));
    }

    #[test]
    fn test_symlink_name_machine() {
        assert_eq!(symlink_name(&LogFormat::ForMachine), "node.json");
    }

    #[test]
    fn test_symlink_name_human() {
        assert_eq!(symlink_name(&LogFormat::ForHuman), "node.log");
    }

    #[test]
    fn test_log_extension() {
        assert_eq!(log_extension(&LogFormat::ForMachine), ".json");
        assert_eq!(log_extension(&LogFormat::ForHuman), ".log");
    }

    #[test]
    fn test_initial_empty_line() {
        // Critical: the Haskell stress test expects lineLength - 1 === numMsg,
        // meaning the first line must be empty.
        let dir = tempfile::tempdir().unwrap();
        let lp = make_logging_params(dir.path(), LogFormat::ForMachine);
        let mgr = LogManager::new(&[lp.clone()], None);

        // Trigger file creation by writing 0 objects (forces handle creation path)
        // Actually, write_trace_objects only creates handle when called, so we
        // need to use create_log_file directly.
        let handle = mgr.create_log_file("test-node", &lp).unwrap();

        // Read the file and verify first line is empty.
        let contents = std::fs::read_to_string(&handle.path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert!(!lines.is_empty());
        assert_eq!(lines[0], "", "First line must be empty");
    }

    #[test]
    fn test_symlink_creation() {
        let dir = tempfile::tempdir().unwrap();
        let lp = make_logging_params(dir.path(), LogFormat::ForMachine);
        let mgr = LogManager::new(&[lp.clone()], None);

        let _handle = mgr.create_log_file("test-node", &lp).unwrap();

        let node_dir = dir.path().join("test-node");
        let symlink = node_dir.join("node.json");
        assert!(symlink.exists(), "Symlink must exist");
        assert!(symlink.is_symlink(), "Must be a symlink");
    }

    #[test]
    fn test_directory_creation() {
        let dir = tempfile::tempdir().unwrap();
        let lp = make_logging_params(dir.path(), LogFormat::ForMachine);
        let mgr = LogManager::new(&[lp.clone()], None);

        let node_dir = dir.path().join("new-node");
        assert!(!node_dir.exists());

        let _handle = mgr.create_log_file("new-node", &lp).unwrap();
        assert!(node_dir.exists());
        assert!(node_dir.is_dir());
    }

    #[test]
    fn test_rotation_by_size() {
        let dir = tempfile::tempdir().unwrap();
        let lp = make_logging_params(dir.path(), LogFormat::ForMachine);
        let rotation = RotationParams {
            frequency_secs: 1,
            log_limit_bytes: 50, // very small limit
            max_age_minutes: None,
            max_age_hours: Some(24),
            keep_files_num: 10,
        };

        let mut mgr = LogManager::new(&[lp.clone()], Some(rotation.clone()));

        // Create initial file and write enough data to exceed limit.
        let handle = mgr.create_log_file("rot-node", &lp).unwrap();
        let initial_path = handle.path.clone();
        mgr.handles.insert(
            HandleKey {
                node_name: "rot-node".to_string(),
                params_idx: 0,
            },
            handle,
        );

        // Write data to exceed the limit.
        {
            let key = HandleKey {
                node_name: "rot-node".to_string(),
                params_idx: 0,
            };
            if let Some(h) = mgr.handles.get_mut(&key) {
                for _ in 0..10 {
                    writeln!(h.writer, "some log line that is reasonably long").unwrap();
                }
                h.writer.flush().unwrap();
            }
        }

        // Small delay so the new file gets a different timestamp.
        std::thread::sleep(std::time::Duration::from_millis(1100));

        // Trigger rotation check.
        mgr.check_rotation(&rotation);

        // The handle should now point to a different file.
        let key = HandleKey {
            node_name: "rot-node".to_string(),
            params_idx: 0,
        };
        let new_handle = mgr.handles.get(&key).unwrap();
        assert_ne!(
            new_handle.path, initial_path,
            "Rotation should create a new file"
        );

        // Both files should exist.
        assert!(initial_path.exists(), "Old file should still exist");
        assert!(new_handle.path.exists(), "New file should exist");

        // Symlink should point to the new file.
        let node_dir = dir.path().join("rot-node");
        let symlink = node_dir.join("node.json");
        let target = std::fs::read_link(&symlink).unwrap();
        assert_eq!(
            target.to_str().unwrap(),
            new_handle.path.file_name().unwrap().to_str().unwrap()
        );
    }

    #[test]
    fn test_extract_timestamp_from_filename() {
        let path = std::path::PathBuf::from("/logs/node-2024-01-15T14-30-00.json");
        let ts = extract_timestamp_from_filename(&path, ".json").unwrap();
        assert_eq!(ts.format("%Y-%m-%d %H:%M:%S").to_string(), "2024-01-15 14:30:00");
    }

    #[test]
    fn test_extract_timestamp_from_filename_invalid() {
        let path = std::path::PathBuf::from("/logs/node-not-a-date.json");
        assert!(extract_timestamp_from_filename(&path, ".json").is_none());
    }

    #[test]
    fn test_multiple_logging_params() {
        let dir = tempfile::tempdir().unwrap();
        let machine_dir = dir.path().join("machine-logs");
        let human_dir = dir.path().join("human-logs");

        let lp_machine = LoggingParams {
            log_root: machine_dir.clone(),
            log_mode: LogMode::FileMode,
            log_format: LogFormat::ForMachine,
        };
        let lp_human = LoggingParams {
            log_root: human_dir.clone(),
            log_mode: LogMode::FileMode,
            log_format: LogFormat::ForHuman,
        };

        let mgr = LogManager::new(&[lp_machine, lp_human], None);

        // Create files for a node in both outputs.
        let h1 = mgr.create_log_file("multi-node", &mgr.params[0].clone()).unwrap();
        let h2 = mgr.create_log_file("multi-node", &mgr.params[1].clone()).unwrap();

        assert!(h1.path.to_str().unwrap().ends_with(".json"));
        assert!(h2.path.to_str().unwrap().ends_with(".log"));
        assert!(machine_dir.join("multi-node").exists());
        assert!(human_dir.join("multi-node").exists());
    }
}
