use gadgetron_core::error::{GadgetronError, NodeErrorKind, Result};
use tokio::process::Child;

/// A running inference engine process owned by NodeAgent.
///
/// Dropping this struct does NOT kill the child process — explicit `stop()` is required.
/// The child handle is stored as `Option` so it can be consumed on graceful shutdown.
pub struct ManagedProcess {
    /// OS process ID of the spawned inference engine.
    pub pid: u32,
    /// Port this process is listening on, allocated from PortPool.
    pub port: u16,
    /// Model ID this process serves.
    pub model_id: String,
    /// Owned child handle. `None` after process has been awaited/killed.
    child: Option<Child>,
}

impl ManagedProcess {
    /// Construct from a successfully spawned child.
    ///
    /// Uses `child.id().unwrap_or(0)` so construction never panics even if the
    /// child already exited before we read its PID.
    pub fn new(model_id: String, port: u16, child: Child) -> Self {
        let pid = child.id().unwrap_or(0);
        Self {
            pid,
            port,
            model_id,
            child: Some(child),
        }
    }

    /// Send SIGTERM; wait up to `timeout` for clean exit; send SIGKILL if still alive.
    ///
    /// After this call the internal child handle is consumed and set to `None`.
    /// Returns `Ok(())` in all cases where the process is no longer running.
    pub async fn stop(&mut self, timeout: std::time::Duration) -> Result<()> {
        let Some(ref mut child) = self.child else {
            // Already stopped — idempotent.
            return Ok(());
        };

        // SIGTERM on unix; on non-unix fall straight through to kill().
        #[cfg(unix)]
        {
            use nix::sys::signal::{self, Signal};
            use nix::unistd::Pid;
            let _ = signal::kill(Pid::from_raw(self.pid as i32), Signal::SIGTERM);
        }
        #[cfg(not(unix))]
        {
            let _ = child.kill().await;
        }

        // Wait up to timeout for graceful exit.
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        tokio::select! {
            status = child.wait() => {
                match status {
                    Ok(_) => {}
                    Err(e) => tracing::warn!(
                        model_id = %self.model_id,
                        pid = self.pid,
                        "wait() error after SIGTERM: {e}"
                    ),
                }
            }
            _ = &mut deadline => {
                tracing::warn!(
                    model_id = %self.model_id,
                    pid = self.pid,
                    "SIGTERM timeout, sending SIGKILL"
                );
                if let Err(e) = child.kill().await {
                    return Err(GadgetronError::Node {
                        kind: NodeErrorKind::ProcessKillFailed,
                        message: format!("SIGKILL failed for pid {}: {e}", self.pid),
                    });
                }
                let _ = child.wait().await;
            }
        }

        self.child = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn managed_process_stop_kills_child() {
        // Spawn a real process that sleeps; stop() must terminate it.
        let child = tokio::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("sleep must be available on test host");

        let mut proc = ManagedProcess::new("test-model".to_string(), 9001, child);
        assert!(proc.pid > 0);

        proc.stop(Duration::from_secs(2))
            .await
            .expect("stop should succeed");

        // child handle consumed — calling stop again must be idempotent
        proc.stop(Duration::from_secs(1))
            .await
            .expect("second stop is idempotent");
    }
}
