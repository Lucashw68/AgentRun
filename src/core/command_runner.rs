//! Bounded, non-shell execution of short lived backend clients.
use super::{Error, Result, proc::PidFd};
use std::{
    io::Read,
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub const OUTPUT_LIMIT: usize = 1024 * 1024;
pub fn run(command: Command, timeout: Duration) -> Result<Vec<u8>> {
    capture(command, timeout, false)
}
pub fn logs(command: Command, timeout: Duration) -> Result<Vec<u8>> {
    capture(command, timeout, true)
}
fn capture(mut command: Command, timeout: Duration, merge: bool) -> Result<Vec<u8>> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn()?;
    let result = (|| {
        let fd = PidFd::open(child.id() as i32)?;
        fd.signal(0, true)?; // Fail closed on kernels without safe group signalling.
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        for pipe in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
            // SAFETY: these are live owned pipe descriptors; fcntl changes only their status flags.
            if unsafe { libc::fcntl(pipe, libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        let result = (|| {
            let deadline = Instant::now() + timeout;
            let mut out = Vec::new();
            let mut err = Vec::new();
            loop {
                for (pipe, bytes) in [
                    (&mut stdout as &mut dyn Read, &mut out),
                    (&mut stderr as &mut dyn Read, &mut err),
                ] {
                    let mut buffer = [0; 8192];
                    // Bound work per poll too: an infinite writer cannot starve the deadline.
                    for _ in 0..32 {
                        match pipe.read(&mut buffer) {
                            Ok(0) => break,
                            Ok(n) => {
                                if bytes.len() + n > OUTPUT_LIMIT {
                                    return Err(Error::new(
                                        "BACKEND_OUTPUT_LIMIT",
                                        "Backend output exceeds 1 MiB.",
                                    ));
                                }
                                bytes.extend_from_slice(&buffer[..n]);
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(e) => return Err(e.into()),
                        }
                    }
                }
                if let Some(status) = child.try_wait()? {
                    // Drain final pipe bytes without waiting on descendants retaining the pipe.
                    for (pipe, bytes) in [
                        (&mut stdout as &mut dyn Read, &mut out),
                        (&mut stderr as &mut dyn Read, &mut err),
                    ] {
                        let mut rest = Vec::new();
                        let _ = pipe.take((OUTPUT_LIMIT + 1) as u64).read_to_end(&mut rest);
                        if bytes.len() + rest.len() > OUTPUT_LIMIT {
                            return Err(Error::new(
                                "BACKEND_OUTPUT_LIMIT",
                                "Backend output exceeds 1 MiB.",
                            ));
                        }
                        bytes.extend(rest);
                    }
                    if !status.success() {
                        // CLI output may contain expanded secrets. Never expose it in errors.
                        return Err(Error::new(
                            "BACKEND_FAILED",
                            format!(
                                "Backend command failed ({status}); inspect the configured recipe and Docker locally."
                            ),
                        ));
                    }
                    if merge {
                        out.extend(err);
                    }
                    return Ok(out);
                }
                if Instant::now() >= deadline {
                    return Err(Error::new(
                        "BACKEND_TIMEOUT",
                        "Backend command timed out; inspect the recorded resource before retrying.",
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })();
        // Only the pinned group created by this invocation. No integer-PID group fallback.
        let _ = fd.signal(libc::SIGKILL, true);
        result
    })();
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}

pub fn remove_control_environment(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("DOCKER_")
            || name.starts_with("COMPOSE_")
            || matches!(
                name.as_ref(),
                "MAKEFLAGS"
                    | "GNUMAKEFLAGS"
                    | "MAKEFILES"
                    | "MFLAGS"
                    | "MAKEOVERRIDES"
                    | "MAKELEVEL"
            )
        {
            command.env_remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeout_and_output_budget_reap_created_helpers() {
        let mut cmd = Command::new("sleep");
        cmd.arg("60");
        let before = Instant::now();
        assert_eq!(
            run(cmd, Duration::from_millis(50)).unwrap_err().code,
            "BACKEND_TIMEOUT"
        );
        assert!(before.elapsed() < Duration::from_secs(2));
        let cmd = Command::new("yes");
        assert_eq!(
            run(cmd, Duration::from_secs(2)).unwrap_err().code,
            "BACKEND_OUTPUT_LIMIT"
        );
    }
    #[test]
    fn backend_error_does_not_disclose_output() {
        let mut cmd = Command::new("cat");
        cmd.arg("/missing-test-secret-value");
        let error = run(cmd, Duration::from_secs(2)).unwrap_err();
        assert_eq!(error.code, "BACKEND_FAILED");
        assert!(!error.message.contains("test-secret-value"));
    }
}
