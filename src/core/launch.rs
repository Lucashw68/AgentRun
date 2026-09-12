//! Fork/exec boundary. All allocation, path resolution and environment building
//! happens before fork. The child only uses async-signal-safe libc operations.
//! A pipe gates execve until the parent has fsynced the registered identity.
use super::{
    error::{Error, Result},
    paths,
    proc::{self, PidFd},
    types::{ManagedProcess, StartRequest, Status},
};
use std::{
    env,
    ffi::CString,
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    path::Path,
};

pub(crate) struct PendingChild {
    pub record: ManagedProcess,
    gate: Option<File>,
    status: File,
    pub pidfd: PidFd,
    released: bool,
    committed: bool,
}

fn pipe() -> Result<(File, File)> {
    let mut fds = [-1; 2];
    // SAFETY: fds is a valid two-element output buffer; pipe2 creates two owned
    // descriptors which are immediately transferred to File.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: each successful pipe2 descriptor is transferred exactly once.
    Ok(unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) })
}

fn wait(pid: i32) {
    loop {
        // SAFETY: waitpid with null status has no writable pointer; this PID is
        // a child of this process, never a signal target selected by PID alone.
        if unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) } >= 0 {
            break;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            break;
        }
    }
}

pub(crate) fn reap(pid: i32) -> Result<()> {
    std::thread::Builder::new()
        .name("agentrun-reap".into())
        .stack_size(64 * 1024)
        .spawn(move || wait(pid))
        .map_err(|e| Error::new("RESOURCE_LIMIT", format!("Cannot create child reaper: {e}")))?;
    Ok(())
}

pub(crate) fn readable(file: &impl AsRawFd) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let mut fd = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        // SAFETY: poll receives one live pollfd for the owned file.
        let result = unsafe { libc::poll(&mut fd, 1, remaining.as_millis().min(5000) as i32) };
        if result > 0 {
            return Ok(());
        }
        if result == 0 {
            return Err(Error::new(
                "LAUNCH_TIMEOUT",
                "Launch handshake exceeded five seconds.",
            ));
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(std::io::Error::last_os_error().into());
        }
    }
}

impl PendingChild {
    pub fn spawn(
        request: &StartRequest,
        cwd: &Path,
        executable: &Path,
        log_path: String,
        log: &File,
    ) -> Result<Self> {
        let boot_id = proc::boot_id()?;
        let executable = paths::cstring(executable)?;
        let args: Vec<CString> = request
            .command
            .iter()
            .map(|s| {
                CString::new(s.as_bytes())
                    .map_err(|_| Error::new("VALIDATION_ERROR", "NUL byte in command"))
            })
            .collect::<Result<_>>()?;
        let environment: Vec<CString> = env::vars_os()
            .filter(|(key, _)| {
                !matches!(
                    key.to_str(),
                    Some(
                        "MAKEFLAGS"
                            | "GNUMAKEFLAGS"
                            | "MAKEFILES"
                            | "MFLAGS"
                            | "MAKEOVERRIDES"
                            | "MAKELEVEL"
                    )
                )
            })
            .map(|(key, value)| {
                let mut bytes = key.as_bytes().to_vec();
                bytes.push(b'=');
                bytes.extend_from_slice(value.as_bytes());
                CString::new(bytes)
                    .map_err(|_| Error::new("VALIDATION_ERROR", "NUL byte in environment"))
            })
            .collect::<Result<_>>()?;
        let argv: Vec<_> = args
            .iter()
            .map(|s| s.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();
        let envp: Vec<_> = environment
            .iter()
            .map(|s| s.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();
        let directory = File::open(cwd)?;
        let null = File::open("/dev/null")?;
        let (gate_read, gate_write) = pipe()?;
        let (status_read, status_write) = pipe()?;
        // SAFETY: the child branch below performs only async-signal-safe libc
        // calls, uses preallocated buffers, never unwinds or returns into Rust,
        // and ends in execve or _exit. This is safe even from the MCP runtime.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if pid == 0 {
            // SAFETY: all descriptors and pointer arrays were constructed before
            // fork and remain live in the child. Only libc operations are used.
            unsafe {
                libc::close(gate_write.as_raw_fd());
                libc::close(status_read.as_raw_fd());
                if libc::setsid() < 0
                    || libc::fchdir(directory.as_raw_fd()) < 0
                    || libc::dup2(null.as_raw_fd(), 0) < 0
                    || libc::dup2(log.as_raw_fd(), 1) < 0
                    || libc::dup2(log.as_raw_fd(), 2) < 0
                {
                    child_error(status_write.as_raw_fd());
                }
                // Tell parent the dedicated group/session is ready to record.
                let ready: i32 = 0;
                if libc::write(status_write.as_raw_fd(), (&ready as *const i32).cast(), 4) != 4 {
                    libc::_exit(125);
                }
                let mut commit = 0u8;
                loop {
                    let n = libc::read(gate_read.as_raw_fd(), (&mut commit as *mut u8).cast(), 1);
                    if n == 1 && commit == 1 {
                        break;
                    }
                    if n < 0 && *libc::__errno_location() == libc::EINTR {
                        continue;
                    }
                    libc::_exit(125); // EOF: parent died or aborted before commit.
                }
                libc::close(gate_read.as_raw_fd());
                libc::signal(libc::SIGPIPE, libc::SIG_DFL);
                libc::execve(executable.as_ptr(), argv.as_ptr(), envp.as_ptr());
                child_error(status_write.as_raw_fd());
            }
        }
        drop(gate_read);
        drop(status_write);
        // On any setup error closing the gate prevents command execution.
        let prepared = (|| {
            let mut status = status_read;
            let mut ready = [0; 4];
            readable(&status)?;
            status.read_exact(&mut ready)?;
            let errno = i32::from_ne_bytes(ready);
            if errno != 0 {
                return Err(std::io::Error::from_raw_os_error(errno).into());
            }
            let identity = proc::inspect(pid).ok_or_else(|| {
                Error::new("PROC_UNAVAILABLE", "Could not capture launch identity.")
            })?;
            if identity.pgid != pid || identity.session != pid {
                return Err(Error::new(
                    "UNSAFE_GROUP",
                    "Child failed to establish its group.",
                ));
            }
            let pidfd = PidFd::open(pid)?;
            // Probe group pidfd support BEFORE allowing user code to execute.
            pidfd.signal(0, true)?;
            let started_at = time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|e| Error::new("TIME_ERROR", e.to_string()))?;
            let record = ManagedProcess {
                id: request.id.clone(),
                pid,
                pgid: pid,
                cwd: paths::path_string(cwd)?,
                command: request.command.clone(),
                started_at,
                status: Status::Running,
                ports: vec![],
                owner: request.owner.clone(),
                process_start_time: identity.start_time,
                boot_id,
                uid: paths::uid(),
                log_path,
                dead_reason: None,
                profile: None,
            };
            Ok((record, status, pidfd))
        })();
        match prepared {
            Ok((record, status, pidfd)) => Ok(Self {
                record,
                status,
                pidfd,
                gate: Some(gate_write),
                released: false,
                committed: false,
            }),
            Err(error) => {
                drop(gate_write);
                wait(pid);
                Err(error)
            }
        }
    }

    /// Called only AFTER registry commit/fsync. EOF on the status pipe means
    /// execve succeeded (CLOEXEC); an errno means no user program was executed.
    pub fn commit(&mut self) -> Result<()> {
        self.commit_with_reaper(reap)
    }
    fn commit_with_reaper(&mut self, reaper: impl FnOnce(i32) -> Result<()>) -> Result<()> {
        let gate = self
            .gate
            .as_mut()
            .ok_or_else(|| Error::new("LAUNCH_ERROR", "Launch already completed."))?;
        gate.write_all(&[1])?;
        self.released = true;
        self.gate.take();
        let mut bytes = Vec::new();
        readable(&self.status)?;
        Read::by_ref(&mut self.status)
            .take(4)
            .read_to_end(&mut bytes)?;
        if !bytes.is_empty() {
            let errno = i32::from_ne_bytes(
                bytes
                    .try_into()
                    .map_err(|_| Error::new("LAUNCH_ERROR", "Truncated exec status."))?,
            );
            return Err(Error::new(
                "EXEC_FAILED",
                std::io::Error::from_raw_os_error(errno).to_string(),
            ));
        }
        let pid = self.record.pid;
        // Reap children while a long-lived MCP client stays connected. The CLI
        // may exit; detached children then become the responsibility of init.
        reaper(pid)?;
        // On failure above, Drop still kills/reaps this registered child.
        self.committed = true;
        Ok(())
    }
}
impl Drop for PendingChild {
    fn drop(&mut self) {
        if !self.committed {
            self.gate.take();
            if self.released {
                let _ = self.pidfd.signal(libc::SIGKILL, true);
            }
            wait(self.record.pid);
        }
    }
}

unsafe fn child_error(fd: i32) -> ! {
    // SAFETY: called only in the post-fork child with its live status fd.
    // errno is copied to a fixed stack buffer; _exit avoids Rust destructors.
    unsafe {
        let errno = *libc::__errno_location();
        libc::write(fd, (&errno as *const i32).cast(), 4);
        libc::_exit(126);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Owner, Paths, logs};
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    fn pending(root: &Path) -> PendingChild {
        let paths = Paths::new(root.join("state"), root.join("config.json"));
        paths.prepare().unwrap();
        let request = StartRequest {
            id: "gated".into(),
            cwd: Some(root.to_str().unwrap().into()),
            command: vec![
                "/usr/bin/touch".into(),
                root.join("must-not-exist").to_str().unwrap().into(),
            ],
            owner: Owner::default(),
        };
        let (path, log) = logs::create(&paths, "gated").unwrap();
        PendingChild::spawn(&request, root, Path::new("/usr/bin/touch"), path, &log).unwrap()
    }
    #[test]
    fn abort_before_commit_never_executes_command() {
        let dir = tempfile::tempdir().unwrap();
        let child = pending(dir.path());
        let pid = child.record.pid;
        assert!(!dir.path().join("must-not-exist").exists());
        drop(child);
        assert!(!dir.path().join("must-not-exist").exists());
        assert!(proc::inspect(pid).is_none());
    }
    #[test]
    fn reaper_failure_rolls_back_and_reaps_child() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().join("state"), dir.path().join("config.json"));
        paths.prepare().unwrap();
        let request = StartRequest {
            id: "reaper-failure".into(),
            cwd: Some(dir.path().to_str().unwrap().into()),
            command: vec!["/usr/bin/sleep".into(), "30".into()],
            owner: Owner::default(),
        };
        let (path, log) = logs::create(&paths, &request.id).unwrap();
        let mut child = PendingChild::spawn(
            &request,
            dir.path(),
            Path::new("/usr/bin/sleep"),
            path,
            &log,
        )
        .unwrap();
        let pid = child.record.pid;
        let error = child
            .commit_with_reaper(|_| {
                Err(Error::new("RESOURCE_LIMIT", "simulated thread exhaustion"))
            })
            .unwrap_err();
        assert_eq!(error.code, "RESOURCE_LIMIT");
        drop(child);
        assert!(proc::inspect(pid).is_none());
    }
    #[test]
    fn crash_parent_fixture() {
        let Some(root) = std::env::var_os("AGENTRUN_TEST_CRASH_ROOT") else {
            return;
        };
        let child = pending(Path::new(&root));
        println!("GATED={}", child.record.pid);
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    #[test]
    fn parent_sigkill_before_commit_leaves_no_unregistered_command() {
        let dir = tempfile::tempdir().unwrap();
        let mut parent = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "core::launch::tests::crash_parent_fixture",
                "--nocapture",
            ])
            .env("AGENTRUN_TEST_CRASH_ROOT", dir.path())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(parent.stdout.take().unwrap());
        let mut line = String::new();
        let pid = loop {
            line.clear();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if let Some(pid) = line.trim().strip_prefix("GATED=") {
                break pid.parse::<i32>().unwrap();
            }
        };
        parent.kill().unwrap();
        parent.wait().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while proc::inspect(pid).is_some_and(|p| p.alive()) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!dir.path().join("must-not-exist").exists());
    }
}
