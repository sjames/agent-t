use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

/// Maximum output buffer size per process (100KB)
const MAX_OUTPUT_SIZE: usize = 100 * 1024;

/// Status of a background process
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessStatus {
    Running,
    Completed,
    Failed,
}

/// Information about a background process
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub id: String,
    pub command: String,
    pub working_dir: Option<String>,
    pub status: ProcessStatus,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub start_time: chrono::DateTime<chrono::Utc>,
}

/// Internal process tracking structure
struct TrackedProcess {
    info: ProcessInfo,
    #[allow(dead_code)]
    child_handle: Option<JoinHandle<()>>,
}

/// Global process manager for tracking background processes
pub struct ProcessManager {
    processes: Arc<RwLock<HashMap<String, TrackedProcess>>>,
}

impl ProcessManager {
    /// Create a new process manager
    pub fn new() -> Self {
        Self {
            processes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Spawn a background process
    pub async fn spawn_background(
        &self,
        command: String,
        working_dir: Option<String>,
    ) -> Result<String, String> {
        // Generate unique ID
        let id = uuid::Uuid::new_v4().to_string();

        // Create the command
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(&command);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        if let Some(ref dir) = working_dir {
            cmd.current_dir(dir);
        }

        // Spawn the process
        let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn process: {}", e))?;

        // Take stdout and stderr
        let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
        let stderr = child.stderr.take().ok_or("Failed to capture stderr")?;

        // Create process info
        let info = ProcessInfo {
            id: id.clone(),
            command: command.clone(),
            working_dir: working_dir.clone(),
            status: ProcessStatus::Running,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            start_time: chrono::Utc::now(),
        };

        // Clone for the monitoring task
        let processes = self.processes.clone();
        let process_id = id.clone();

        // Spawn monitoring task
        let handle = tokio::spawn(async move {
            Self::monitor_process(processes, process_id, child, stdout, stderr).await;
        });

        // Store process info
        let tracked = TrackedProcess {
            info,
            child_handle: Some(handle),
        };

        self.processes.write().await.insert(id.clone(), tracked);

        Ok(id)
    }

    /// Monitor a process and collect its output
    async fn monitor_process(
        processes: Arc<RwLock<HashMap<String, TrackedProcess>>>,
        id: String,
        mut child: Child,
        stdout: tokio::process::ChildStdout,
        stderr: tokio::process::ChildStderr,
    ) {
        let stdout_reader = BufReader::new(stdout);
        let stderr_reader = BufReader::new(stderr);

        let mut stdout_lines = stdout_reader.lines();
        let mut stderr_lines = stderr_reader.lines();

        // Read output streams concurrently
        let stdout_id = id.clone();
        let stderr_id = id.clone();
        let processes_stdout = processes.clone();
        let processes_stderr = processes.clone();

        let stdout_task = tokio::spawn(async move {
            while let Ok(Some(line)) = stdout_lines.next_line().await {
                let mut procs = processes_stdout.write().await;
                if let Some(tracked) = procs.get_mut(&stdout_id) {
                    tracked.info.stdout.push_str(&line);
                    tracked.info.stdout.push('\n');

                    // Trim if too large
                    if tracked.info.stdout.len() > MAX_OUTPUT_SIZE {
                        let trim_at = tracked.info.stdout.len() - MAX_OUTPUT_SIZE;
                        tracked.info.stdout = tracked.info.stdout[trim_at..].to_string();
                    }
                }
            }
        });

        let stderr_task = tokio::spawn(async move {
            while let Ok(Some(line)) = stderr_lines.next_line().await {
                let mut procs = processes_stderr.write().await;
                if let Some(tracked) = procs.get_mut(&stderr_id) {
                    tracked.info.stderr.push_str(&line);
                    tracked.info.stderr.push('\n');

                    // Trim if too large
                    if tracked.info.stderr.len() > MAX_OUTPUT_SIZE {
                        let trim_at = tracked.info.stderr.len() - MAX_OUTPUT_SIZE;
                        tracked.info.stderr = tracked.info.stderr[trim_at..].to_string();
                    }
                }
            }
        });

        // Wait for both output streams to complete
        let _ = tokio::join!(stdout_task, stderr_task);

        // Wait for process to complete
        if let Ok(status) = child.wait().await {
            let mut procs = processes.write().await;
            if let Some(tracked) = procs.get_mut(&id) {
                tracked.info.exit_code = status.code();
                tracked.info.status = if status.success() {
                    ProcessStatus::Completed
                } else {
                    ProcessStatus::Failed
                };
            }
        }
    }

    /// Get information about a process
    pub async fn get_process(&self, id: &str) -> Option<ProcessInfo> {
        let procs = self.processes.read().await;
        procs.get(id).map(|tracked| tracked.info.clone())
    }

    /// List all processes
    pub async fn list_processes(&self) -> Vec<ProcessInfo> {
        let procs = self.processes.read().await;
        procs.values().map(|tracked| tracked.info.clone()).collect()
    }

    /// Kill a background process
    pub async fn kill_process(&self, id: &str) -> Result<(), String> {
        let procs = self.processes.read().await;

        if let Some(tracked) = procs.get(id) {
            if tracked.info.status != ProcessStatus::Running {
                return Err(format!("Process {} is not running", id));
            }

            // Try to kill the process using system kill command
            // This is a best-effort approach since we don't have direct access to the child process
            let kill_result = std::process::Command::new("pkill")
                .arg("-P")
                .arg(id)
                .output();

            match kill_result {
                Ok(_) => Ok(()),
                Err(e) => Err(format!("Failed to kill process: {}", e)),
            }
        } else {
            Err(format!("Process {} not found", id))
        }
    }

    /// Clean up completed processes older than the specified duration
    pub async fn cleanup_old_processes(&self, max_age: chrono::Duration) {
        let mut procs = self.processes.write().await;
        let now = chrono::Utc::now();

        procs.retain(|_, tracked| {
            let age = now - tracked.info.start_time;
            tracked.info.status == ProcessStatus::Running || age < max_age
        });
    }
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
    }
}

// Global process manager instance
lazy_static::lazy_static! {
    pub static ref PROCESS_MANAGER: ProcessManager = ProcessManager::new();
}
