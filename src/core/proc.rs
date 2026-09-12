use super::{
    error::{Error, Result},
    paths,
    types::ManagedProcess,
};
use std::{
    collections::HashSet,
    fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::MetadataExt,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub pid: i32,
    pub ppid: i32,
    pub pgid: i32,
    pub session: i32,
    pub start_time: String,
    pub uid: u32,
    pub state: char,
}
impl Identity {
    pub fn same_process(&self, other: &Self) -> bool {
        self.pid == other.pid
            && self.pgid == other.pgid
            && self.session == other.session
            && self.start_time == other.start_time
            && self.uid == other.uid
    }
    pub fn alive(&self) -> bool {
        !matches!(self.state, 'Z' | 'X' | 'x')
    }
}
pub fn boot_id() -> Result<String> {
    Ok(fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned())
}
pub fn inspect(pid: i32) -> Option<Identity> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<_> = text
        .get(text.rfind(')')? + 2..)?
        .split_whitespace()
        .collect();
    Some(Identity {
        pid,
        ppid: fields.get(1)?.parse().ok()?,
        pgid: fields.get(2)?.parse().ok()?,
        session: fields.get(3)?.parse().ok()?,
        start_time: fields.get(19)?.to_string(),
        uid: fs::metadata(format!("/proc/{pid}")).ok()?.uid(),
        state: fields.first()?.chars().next()?,
    })
}
pub fn matches(record: &ManagedProcess) -> bool {
    record.pid >= 2
        && record.pid == record.pgid
        && record.uid == paths::uid()
        && boot_id().is_ok_and(|id| id == record.boot_id)
        && inspect(record.pid).is_some_and(|p| {
            p.alive()
                && p.pgid == record.pgid
                && p.session == record.pgid
                && p.uid == record.uid
                && p.start_time == record.process_start_time
        })
}
pub fn table() -> Vec<Identity> {
    fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .ok()?
                .file_name()
                .to_str()?
                .parse()
                .ok()
                .and_then(inspect)
        })
        .filter(Identity::alive)
        .collect()
}
pub fn group(pgid: i32) -> Vec<Identity> {
    table()
        .into_iter()
        .filter(|p| p.pgid == pgid && p.session == pgid)
        .collect()
}
pub fn socket_inodes(pid: i32) -> HashSet<String> {
    fs::read_dir(format!("/proc/{pid}/fd"))
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let link = fs::read_link(entry.ok()?.path()).ok()?;
            let text = link.to_str()?;
            Some(text.strip_prefix("socket:[")?.strip_suffix(']')?.to_owned())
        })
        .collect()
}

/// A kernel reference, not a reusable integer PID. Never serialized.
pub(crate) struct PidFd(OwnedFd);
impl PidFd {
    pub fn open(pid: i32) -> Result<Self> {
        // SAFETY: pidfd_open receives only integer arguments. A successful
        // return is a newly owned CLOEXEC descriptor, transferred to OwnedFd.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0u32) };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: the successful syscall above created this fd for us.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd as i32) }))
    }
    pub fn verified(record: &ManagedProcess) -> Result<Self> {
        let fd = Self::open(record.pid)?;
        // Validate AFTER opening, then ensure the pinned process still exists.
        if !matches(record) {
            return Err(Error::new(
                "IDENTITY_MISMATCH",
                "Refusing to signal a missing or mismatched process.",
            ));
        }
        fd.signal(0, false)?;
        Ok(fd)
    }
    pub fn signal(&self, signal: i32, group: bool) -> Result<()> {
        const PIDFD_SIGNAL_PROCESS_GROUP: u32 = 1 << 2;
        // SAFETY: self owns a valid pidfd; null siginfo asks the kernel to
        // construct it. The group flag resolves the pinned kernel pid object,
        // avoiding a numeric kill(-pgid) lookup and PID reuse races.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.0.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                if group { PIDFD_SIGNAL_PROCESS_GROUP } else { 0 },
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if group && error.raw_os_error() == Some(libc::EINVAL) {
                return Err(Error::new(
                    "UNSUPPORTED_KERNEL",
                    "Safe group signals require Linux >= 6.9 with pidfd support.",
                ));
            }
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Err(Error::new(
                    "PROCESS_GONE",
                    "Pinned process/group has exited.",
                ));
            }
            return Err(error.into());
        }
        Ok(())
    }
}
