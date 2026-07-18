use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const OUTPUT_COLLECTION_GRACE: Duration = Duration::from_millis(250);
const WAIT_POLL: Duration = Duration::from_millis(20);

#[derive(Clone, Copy, Debug)]
pub struct Deadline(Instant);

impl Deadline {
    pub fn after(duration: Duration) -> Self {
        Self(Instant::now() + duration)
    }

    pub fn remaining(self) -> Option<Duration> {
        self.0
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
    }

    pub fn is_expired(self) -> bool {
        self.remaining().is_none()
    }
}

#[derive(Debug)]
pub struct CommandOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandError {
    NotFound,
    TimedOut,
    Failed,
}

/// Run one child under an absolute deadline. Output pipes are drained on
/// dedicated threads and retained only up to the requested bounds. Platform
/// process groups ensure timeout cleanup includes Git's credential and
/// transport helpers rather than killing only the top-level `git` process.
pub fn run_command(
    mut command: Command,
    deadline: Deadline,
    stdout_limit: usize,
    stderr_limit: usize,
) -> Result<CommandOutput, CommandError> {
    if deadline.is_expired() {
        return Err(CommandError::TimedOut);
    }

    command.stdin(Stdio::null());
    if stdout_limit > 0 {
        command.stdout(Stdio::piped());
    } else {
        command.stdout(Stdio::null());
    }
    if stderr_limit > 0 {
        command.stderr(Stdio::piped());
    } else {
        command.stderr(Stdio::null());
    }
    platform::configure(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CommandError::NotFound);
        }
        Err(_) => return Err(CommandError::Failed),
    };
    let stdout = child
        .stdout
        .take()
        .map(|pipe| spawn_capture(pipe, stdout_limit));
    let stderr = child
        .stderr
        .take()
        .map(|pipe| spawn_capture(pipe, stderr_limit));
    let tree = platform::ProcessTree::attach(&child);

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                let Some(remaining) = deadline.remaining() else {
                    break Err(CommandError::TimedOut);
                };
                std::thread::sleep(WAIT_POLL.min(remaining));
            }
            Err(_) => break Err(CommandError::Failed),
        }
    };

    match status {
        Ok(status) => {
            // A helper must never outlive the command whose pipes and
            // credentials it inherited, even when the top-level process
            // reports success first.
            if let Some(tree) = tree.as_ref() {
                tree.terminate();
            }
            drop(tree);
            Ok(CommandOutput {
                status,
                stdout: collect(stdout, deadline)?,
                stderr: collect(stderr, deadline)?,
            })
        }
        Err(error) => {
            if let Some(tree) = tree.as_ref() {
                tree.terminate();
            }
            let _ = child.kill();
            let _ = child.wait();
            drop(tree);
            drop(stdout);
            drop(stderr);
            Err(error)
        }
    }
}

fn spawn_capture(reader: impl Read + Send + 'static, limit: usize) -> Receiver<String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = sender.send(read_bounded(reader, limit));
    });
    receiver
}

fn read_bounded(mut reader: impl Read, limit: usize) -> String {
    let mut captured = Vec::with_capacity(limit.min(4096));
    let mut buffer = [0_u8; 4096];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let remaining = limit.saturating_sub(captured.len());
        if remaining > 0 {
            captured.extend_from_slice(&buffer[..read.min(remaining)]);
        }
    }
    String::from_utf8_lossy(&captured).into_owned()
}

fn collect(capture: Option<Receiver<String>>, deadline: Deadline) -> Result<String, CommandError> {
    let Some(capture) = capture else {
        return Ok(String::new());
    };
    let Some(remaining) = deadline.remaining() else {
        return Err(CommandError::TimedOut);
    };
    match capture.recv_timeout(OUTPUT_COLLECTION_GRACE.min(remaining)) {
        Ok(output) => Ok(output),
        Err(_) if deadline.is_expired() => Err(CommandError::TimedOut),
        Err(_) => Ok(String::new()),
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};

    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub fn configure(command: &mut Command) {
        command.creation_flags(CREATE_NO_WINDOW);
    }

    pub struct ProcessTree {
        job: HANDLE,
    }

    impl ProcessTree {
        pub fn attach(child: &Child) -> Option<Self> {
            let job = unsafe { CreateJobObjectW(None, None) }.ok()?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast::<c_void>(),
                    std::mem::size_of_val(&limits) as u32,
                )
            }
            .is_ok();
            let process = HANDLE(child.as_raw_handle());
            let assigned = configured && unsafe { AssignProcessToJobObject(job, process) }.is_ok();
            if !assigned {
                let _ = unsafe { CloseHandle(job) };
                return None;
            }
            Some(Self { job })
        }

        pub fn terminate(&self) {
            let _ = unsafe { TerminateJobObject(self.job, 1) };
        }
    }

    impl Drop for ProcessTree {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.job) };
        }
    }
}

#[cfg(unix)]
mod platform {
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    pub fn configure(command: &mut Command) {
        command.process_group(0);
    }

    pub struct ProcessTree {
        process_group: libc::pid_t,
    }

    impl ProcessTree {
        pub fn attach(child: &Child) -> Option<Self> {
            Some(Self {
                process_group: child.id() as libc::pid_t,
            })
        }

        pub fn terminate(&self) {
            let _ = unsafe { libc::kill(-self.process_group, libc::SIGKILL) };
        }
    }

    impl Drop for ProcessTree {
        fn drop(&mut self) {
            self.terminate();
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    use std::process::{Child, Command};

    pub fn configure(_command: &mut Command) {}

    pub struct ProcessTree;

    impl ProcessTree {
        pub fn attach(_child: &Child) -> Option<Self> {
            None
        }

        pub fn terminate(&self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELPER_ROLE: &str = "GITWINK_PROCESS_TREE_HELPER";

    #[test]
    fn deadline_terminates_the_descendant_process_tree() {
        let root = tempfile::tempdir().unwrap();
        let started = root.path().join("descendant-started");
        let escaped = root.path().join("descendant-escaped");
        let command = helper_command("parent", &started, &escaped);

        let result = run_command(
            command,
            Deadline::after(Duration::from_millis(450)),
            4096,
            4096,
        );
        assert!(matches!(result, Err(CommandError::TimedOut)));
        std::thread::sleep(Duration::from_millis(1_100));
        assert!(started.exists(), "descendant must start before the timeout");
        assert!(
            !escaped.exists(),
            "a descendant survived the process-tree timeout"
        );
    }

    #[test]
    fn process_tree_helper() {
        let Ok(role) = std::env::var(HELPER_ROLE) else {
            return;
        };
        let started = std::path::PathBuf::from(std::env::var("GITWINK_STARTED").unwrap());
        let escaped = std::path::PathBuf::from(std::env::var("GITWINK_ESCAPED").unwrap());
        match role.as_str() {
            "parent" => {
                // Give the caller time to attach this process to its Job Object
                // or process group before the descendant is created.
                std::thread::sleep(Duration::from_millis(100));
                let mut child = helper_command("child", &started, &escaped).spawn().unwrap();
                std::thread::sleep(Duration::from_secs(10));
                let _ = child.wait();
            }
            "child" => {
                std::fs::write(&started, b"started").unwrap();
                std::thread::sleep(Duration::from_millis(800));
                std::fs::write(&escaped, b"escaped").unwrap();
                std::thread::sleep(Duration::from_secs(10));
            }
            _ => panic!("unknown helper role"),
        }
    }

    fn helper_command(role: &str, started: &std::path::Path, escaped: &std::path::Path) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "fetch::child_process::tests::process_tree_helper",
                "--nocapture",
            ])
            .env(HELPER_ROLE, role)
            .env("GITWINK_STARTED", started)
            .env("GITWINK_ESCAPED", escaped);
        command
    }
}
