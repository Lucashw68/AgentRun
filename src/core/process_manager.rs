use super::{
    error::{Error, Result},
    proc::{self, PidFd},
    types::ManagedProcess,
};
use std::{
    thread,
    time::{Duration, Instant},
};

pub fn stop(record: &ManagedProcess, grace: Duration) -> Result<()> {
    let group_fd = PidFd::verified(record)?;
    group_fd.signal(libc::SIGTERM, true)?;
    if wait_group(record.pgid, grace) {
        return Ok(());
    }
    // The pidfd still refers to the original kernel group, even when the
    // leader has exited during the grace period. No numeric kill fallback.
    match group_fd.signal(libc::SIGKILL, true) {
        Ok(()) => {}
        Err(e) if e.code == "PROCESS_GONE" && proc::group(record.pgid).is_empty() => return Ok(()),
        Err(e) => return Err(e),
    }
    if wait_group(record.pgid, Duration::from_millis(1500)) {
        Ok(())
    } else {
        Err(Error::new(
            "STOP_TIMEOUT",
            format!("Group {} has not exited.", record.pgid),
        ))
    }
}
fn wait_group(pgid: i32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if proc::group(pgid).is_empty() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(30));
    }
}
