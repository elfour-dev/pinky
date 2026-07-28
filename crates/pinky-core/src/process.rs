use std::{
    io,
    os::unix::{process::CommandExt, process::ExitStatusExt},
    process::ExitStatus,
    time::Duration,
};

use thiserror::Error;
use tokio::{process::Command, time::timeout};
use tokio_util::sync::CancellationToken;

const COOPERATIVE_GRACE: Duration = Duration::from_secs(2);
const TERM_GRACE: Duration = Duration::from_secs(8);
const REAP_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessTermination {
    Exited,
    CancelledCooperatively,
    Sigterm,
    Sigkill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessOutcome {
    pub pid: u32,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub termination: ProcessTermination,
}

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("failed to spawn supervised process: {0}")]
    Spawn(#[source] io::Error),
    #[error("failed while waiting for supervised process {pid}: {source}")]
    Wait {
        pid: u32,
        #[source]
        source: io::Error,
    },
    #[error("failed to signal supervised process group {pid}: {source}")]
    Signal {
        pid: u32,
        #[source]
        source: io::Error,
    },
    #[error(
        "process group {pid} did not exit after SIGKILL; it may be in kernel-level uninterruptible sleep"
    )]
    Uninterruptible { pid: u32 },
}

#[derive(Debug, Clone, Copy)]
pub struct ProcessSupervisor {
    cooperative_grace: Duration,
    term_grace: Duration,
    reap_grace: Duration,
}

impl Default for ProcessSupervisor {
    fn default() -> Self {
        Self {
            cooperative_grace: COOPERATIVE_GRACE,
            term_grace: TERM_GRACE,
            reap_grace: REAP_GRACE,
        }
    }
}

impl ProcessSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    #[doc(hidden)]
    pub fn with_deadlines(
        cooperative_grace: Duration,
        term_grace: Duration,
        reap_grace: Duration,
    ) -> Self {
        Self {
            cooperative_grace,
            term_grace,
            reap_grace,
        }
    }

    /// Runs a command in a new process group. No descendant is allowed to
    /// outlive the supervised leader, including on future cancellation or
    /// task abortion.
    pub async fn run(
        &self,
        mut command: Command,
        cancellation: CancellationToken,
    ) -> Result<ProcessOutcome, ProcessError> {
        command.kill_on_drop(true);
        command.as_std_mut().process_group(0);
        let child = command.spawn().map_err(ProcessError::Spawn)?;
        let pid = child
            .id()
            .expect("a newly spawned child always has a process identifier");
        let mut process = ProcessGuard::new(child, pid);

        tokio::select! {
            status = process.wait() => {
                return finish(&mut process, status?, ProcessTermination::Exited);
            }
            _ = cancellation.cancelled() => {}
        }

        tokio::select! {
            status = process.wait() => {
                return finish(
                    &mut process,
                    status?,
                    ProcessTermination::CancelledCooperatively,
                );
            }
            _ = tokio::time::sleep(self.cooperative_grace) => {}
        }

        process.signal(libc::SIGTERM)?;
        tokio::select! {
            status = process.wait() => {
                return finish(&mut process, status?, ProcessTermination::Sigterm);
            }
            _ = tokio::time::sleep(self.term_grace) => {}
        }

        process.signal(libc::SIGKILL)?;
        let status = timeout(self.reap_grace, process.wait())
            .await
            .map_err(|_| ProcessError::Uninterruptible { pid })??;
        finish(&mut process, status, ProcessTermination::Sigkill)
    }
}

fn finish(
    process: &mut ProcessGuard,
    status: ExitStatus,
    termination: ProcessTermination,
) -> Result<ProcessOutcome, ProcessError> {
    // The leader may have launched background descendants. A supervised task
    // never leaves those running after the leader has finished.
    process.signal(libc::SIGKILL)?;
    process.disarm();
    Ok(ProcessOutcome {
        pid: process.pid,
        exit_code: status.code(),
        signal: status.signal(),
        termination,
    })
}

struct ProcessGuard {
    child: Option<tokio::process::Child>,
    pid: u32,
    armed: bool,
}

impl ProcessGuard {
    fn new(child: tokio::process::Child, pid: u32) -> Self {
        Self {
            child: Some(child),
            pid,
            armed: true,
        }
    }

    async fn wait(&mut self) -> Result<ExitStatus, ProcessError> {
        self.child
            .as_mut()
            .expect("process guard is armed")
            .wait()
            .await
            .map_err(|source| ProcessError::Wait {
                pid: self.pid,
                source,
            })
    }

    fn signal(&self, signal: libc::c_int) -> Result<(), ProcessError> {
        // The child is created as the leader of a fresh process group. A
        // negative PID targets that entire group and cannot reach Pinky.
        let result = unsafe { libc::kill(-(self.pid as i32), signal) };
        if result == 0 {
            return Ok(());
        }
        let source = io::Error::last_os_error();
        if source.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        Err(ProcessError::Signal {
            pid: self.pid,
            source,
        })
    }

    fn disarm(&mut self) {
        self.armed = false;
        self.child.take();
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if self.armed {
            // SAFETY: the negative identifier is the dedicated child process
            // group created before spawn, never Pinky's own process group.
            let _ = unsafe { libc::kill(-(self.pid as i32), libc::SIGKILL) };
            if let Some(child) = self.child.as_mut() {
                let _ = child.start_kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};

    fn fast_supervisor() -> ProcessSupervisor {
        ProcessSupervisor::with_deadlines(
            Duration::from_millis(20),
            Duration::from_millis(80),
            Duration::from_millis(250),
        )
    }

    async fn wait_for_file(path: &Path) {
        timeout(Duration::from_secs(1), async {
            while !path.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn returns_a_normal_exit_status() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 7"]);
        let outcome = fast_supervisor()
            .run(command, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(outcome.termination, ProcessTermination::Exited);
        assert_eq!(outcome.exit_code, Some(7));
    }

    #[tokio::test]
    async fn escalates_cancellation_to_sigterm() {
        let mut command = Command::new("/usr/bin/tail");
        command.args(["-f", "/dev/null"]);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let outcome = fast_supervisor().run(command, cancellation).await.unwrap();
        assert_eq!(outcome.termination, ProcessTermination::Sigterm);
        assert_eq!(outcome.signal, Some(libc::SIGTERM));
    }

    #[tokio::test]
    async fn escalates_an_ignored_sigterm_to_sigkill() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "trap '' TERM; exec /usr/bin/tail -f /dev/null"]);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let outcome = fast_supervisor().run(command, cancellation).await.unwrap();
        assert_eq!(outcome.termination, ProcessTermination::Sigkill);
        assert_eq!(outcome.signal, Some(libc::SIGKILL));
    }

    #[tokio::test]
    async fn cancellation_stops_the_entire_descendant_process_group() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("descendant.pid");
        let mut command = Command::new("/bin/sh");
        command.env("PINKY_TEST_PID_FILE", &pid_file).args([
            "-c",
            "sleep 60 & child=$!; echo $child > \"$PINKY_TEST_PID_FILE\"; wait",
        ]);
        let cancellation = CancellationToken::new();
        let supervisor = fast_supervisor();
        let future = supervisor.run(command, cancellation.clone());
        tokio::pin!(future);
        tokio::select! {
            outcome = &mut future => panic!("process exited before cancellation: {outcome:?}"),
            _ = wait_for_file(&pid_file) => {}
        }
        let descendant: i32 = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        cancellation.cancel();
        let outcome = future.await.unwrap();
        assert!(matches!(
            outcome.termination,
            ProcessTermination::Sigterm | ProcessTermination::Sigkill
        ));
        timeout(Duration::from_secs(1), async {
            loop {
                // Signal zero probes existence without changing process state.
                let result = unsafe { libc::kill(descendant, 0) };
                if result == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn aborting_the_supervision_future_kills_descendants_on_drop() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("abort-descendant.pid");
        let mut command = Command::new("/bin/sh");
        command.env("PINKY_TEST_PID_FILE", &pid_file).args([
            "-c",
            "sleep 60 & child=$!; echo $child > \"$PINKY_TEST_PID_FILE\"; wait",
        ]);
        let supervisor = fast_supervisor();
        let mut future = Box::pin(supervisor.run(command, CancellationToken::new()));
        tokio::select! {
            outcome = &mut future => panic!("process exited before abort: {outcome:?}"),
            _ = wait_for_file(&pid_file) => {}
        }
        let descendant: i32 = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        drop(future);

        timeout(Duration::from_secs(1), async {
            loop {
                let result = unsafe { libc::kill(descendant, 0) };
                if result == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
