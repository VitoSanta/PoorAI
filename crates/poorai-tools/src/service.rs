//! Long-running processes: starting one, waiting for it, and being sure it is
//! gone.
//!
//! `LocalService` opened a port and nothing used it. Verifying a system rather
//! than a file means standing a service up and exercising it, and `run_command`
//! cannot do that: it waits for the process to exit, which is the one thing a
//! server does not do.
//!
//! The hard part is not starting them. It is that a run which crashes, is
//! killed, or simply forgets leaves a process holding a port on the developer's
//! machine -- so every service here belongs to a supervisor that kills what it
//! started when it is dropped, whatever route the run took out.

use crate::{Approval, ToolError, ToolPolicy};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// A service the run started.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHandle {
    pub id: u32,
    pub executable: String,
    pub args: Vec<String>,
    /// The port it was told to use, where the caller asked for one.
    pub port: Option<u16>,
    pub pid: Option<u32>,
}

/// How a service ended.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceOutcome {
    pub id: u32,
    /// `None` when it was killed rather than exiting on its own.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
    pub ran_for_ms: u128,
}

struct Running {
    child: tokio::process::Child,
    group: Option<u32>,
    handle: ServiceHandle,
    started: Instant,
}

/// Owns every service a run started.
///
/// Dropping it kills them. That is the whole design: a supervisor that relies
/// on the caller remembering to stop things is a supervisor that leaves a
/// server running on someone's laptop the first time a run panics.
pub struct ServiceSupervisor {
    running: BTreeMap<u32, Running>,
    next_id: u32,
}

impl Default for ServiceSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceSupervisor {
    pub fn new() -> Self {
        Self {
            running: BTreeMap::new(),
            next_id: 1,
        }
    }

    /// A port nothing else is listening on, right now.
    ///
    /// Asked of the operating system rather than picked from a range: a range
    /// collides with whatever else the developer is running, and this project
    /// has no business choosing 8080 on their machine.
    ///
    /// It is a reservation with a race in it, and saying so matters -- the
    /// socket is closed before the child binds, so something else can take the
    /// port in between. That window is unavoidable without handing the child a
    /// bound descriptor, which needs the child's cooperation. A caller that
    /// fails to bind should ask again rather than assume the port is wrong.
    pub fn reserve_port() -> Result<u16, ToolError> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        drop(listener);
        Ok(port)
    }

    /// Starts a service under the run's policy.
    ///
    /// Every guard `run_command` applies applies here: the executable must be
    /// allowlisted, the sandbox confines it, and its home and scratch stay
    /// inside the workspace. The one addition is the approval -- a process that
    /// outlives the action that started it needs the grant that says local
    /// services are allowed at all.
    pub async fn start(
        &mut self,
        policy: &ToolPolicy,
        executable: &str,
        args: &[String],
        port: Option<u16>,
    ) -> Result<ServiceHandle, ToolError> {
        policy.require(Approval::LocalService)?;
        let mut command = policy.prepare_command(executable, args)?;
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn()?;
        let id = self.next_id;
        self.next_id += 1;
        let handle = ServiceHandle {
            id,
            executable: executable.to_string(),
            args: args.to_vec(),
            port,
            pid: child.id(),
        };
        self.running.insert(
            id,
            Running {
                group: child.id(),
                child,
                handle: handle.clone(),
                started: Instant::now(),
            },
        );
        Ok(handle)
    }

    /// Waits until something answers on a port, or gives up.
    ///
    /// A service is ready when it accepts a connection, not when its process
    /// exists: a server that has been spawned and has not yet bound will refuse
    /// every request sent to it, and a test that races it fails for reasons
    /// that have nothing to do with the code.
    pub async fn wait_until_ready(
        &mut self,
        id: u32,
        port: u16,
        timeout: Duration,
    ) -> Result<Duration, ToolError> {
        let deadline = Instant::now() + timeout;
        loop {
            // A process that has already exited will never bind, and waiting
            // the full timeout to discover that wastes the time the caller
            // gave for starting rather than for failing.
            if let Some(running) = self.running.get_mut(&id)
                && running.child.try_wait().ok().flatten().is_some()
            {
                return Err(ToolError::Denied(format!(
                    "service {id} exited before it accepted a connection"
                )));
            }
            if std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
                Duration::from_millis(100),
            )
            .is_ok()
            {
                let waited =
                    timeout.saturating_sub(deadline.saturating_duration_since(Instant::now()));
                return Ok(waited);
            }
            if Instant::now() >= deadline {
                return Err(ToolError::Timeout);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Stops a service and reports what it did.
    ///
    /// The process group, not the process: a server that forked workers leaves
    /// them holding the port, which is the failure this is here to prevent.
    pub async fn stop(
        &mut self,
        id: u32,
        output_limit: usize,
    ) -> Result<ServiceOutcome, ToolError> {
        let Some(mut running) = self.running.remove(&id) else {
            return Err(ToolError::Denied(format!("no service {id} is running")));
        };
        kill_group(running.group);
        let _ = running.child.start_kill();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let Some(pipe) = running.child.stdout.as_mut() {
            let _ = read_capped(pipe, output_limit, &mut stdout).await;
        }
        if let Some(pipe) = running.child.stderr.as_mut() {
            let _ = read_capped(pipe, output_limit, &mut stderr).await;
        }
        let status = running.child.wait().await.ok();
        let truncated = stdout.len() >= output_limit || stderr.len() >= output_limit;
        let (stdout, _) = policy_redact(&String::from_utf8_lossy(&stdout));
        let (stderr, _) = policy_redact(&String::from_utf8_lossy(&stderr));
        Ok(ServiceOutcome {
            id,
            exit_code: status.and_then(|status| status.code()),
            stdout,
            stderr,
            truncated,
            ran_for_ms: running.started.elapsed().as_millis(),
        })
    }

    pub fn running(&self) -> Vec<ServiceHandle> {
        self.running
            .values()
            .map(|running| running.handle.clone())
            .collect()
    }
}

impl Drop for ServiceSupervisor {
    fn drop(&mut self) {
        // Whatever route the run took out -- returned, errored, panicked --
        // nothing it started is still holding a port.
        for (_, running) in std::mem::take(&mut self.running) {
            kill_group(running.group);
        }
    }
}

fn kill_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        // `output()` rather than `status()`: killing a group that has already
        // exited prints to stderr, and the run's stderr is where `--json`
        // output goes.
        let _ = std::process::Command::new("/bin/kill")
            .arg("-KILL")
            .arg(format!("-{pid}"))
            .output();
    }
    #[cfg(not(unix))]
    let _ = pid;
}

async fn read_capped(
    pipe: &mut (impl tokio::io::AsyncRead + Unpin),
    limit: usize,
    into: &mut Vec<u8>,
) -> std::io::Result<()> {
    use tokio::io::AsyncReadExt as _;
    let mut buffer = [0u8; 8192];
    while into.len() < limit {
        // The process is already killed, so a read that would block means
        // there is nothing more coming.
        let read =
            match tokio::time::timeout(Duration::from_millis(200), pipe.read(&mut buffer)).await {
                Ok(Ok(0)) | Err(_) => break,
                Ok(Ok(read)) => read,
                Ok(Err(error)) => return Err(error),
            };
        let room = limit - into.len();
        into.extend_from_slice(&buffer[..read.min(room)]);
    }
    Ok(())
}

/// Service output is untrusted text like any other, and is redacted before it
/// can reach a prompt.
fn policy_redact(text: &str) -> (String, bool) {
    ToolPolicy {
        root: std::path::PathBuf::from("/"),
        extra_readable: Vec::new(),
        allow_commands: Vec::new(),
        output_limit: 0,
        timeout: Duration::from_secs(0),
        sandbox: crate::SandboxPolicy::Disabled,
        approvals: Vec::new(),
    }
    .redact(text)
}
