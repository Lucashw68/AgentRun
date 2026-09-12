use super::{
    error::{Error, Result},
    paths::{Paths, private_file},
    types::RegistryData,
    validation,
};
use std::{
    fs::{self, File},
    io::Write,
    os::fd::AsRawFd,
    thread,
    time::{Duration, Instant},
};

pub struct Registry {
    pub paths: Paths,
}

/// Owns a kernel advisory lock. Drop closes the fd and releases the lock,
/// including on errors; the kernel releases it on process exit or SIGKILL.
pub struct Transaction<'a> {
    registry: &'a Registry,
    _lock: File,
    pub data: RegistryData,
}

impl Registry {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }
    pub fn transaction(&self) -> Result<Transaction<'_>> {
        self.paths.prepare()?;
        let lock = private_file(&self.paths.locks.join("registry.lock"), true, true, false)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            // SAFETY: flock only operates on the live fd owned by lock.
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                break;
            }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::WouldBlock
                && e.kind() != std::io::ErrorKind::Interrupted
            {
                return Err(e.into());
            }
            if Instant::now() >= deadline {
                return Err(Error::new(
                    "LOCK_TIMEOUT",
                    "Registry remained locked for 30 seconds.",
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
        let data = match private_file(&self.paths.registry, false, false, false) {
            Ok(file) => {
                let bytes = super::limits::read_bounded(
                    file,
                    super::limits::MAX_REGISTRY_BYTES,
                    "REGISTRY_TOO_LARGE",
                )?;
                let data: RegistryData = serde_json::from_slice(&bytes).map_err(|e| {
                    Error::new(
                        "REGISTRY_INVALID",
                        format!("Refusing to overwrite invalid registry: {e}"),
                    )
                })?;
                validation::registry(&data)?;
                data
            }
            Err(_)
                if fs::symlink_metadata(&self.paths.registry)
                    .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                RegistryData::default()
            }
            Err(e) => return Err(e),
        };
        Ok(Transaction {
            registry: self,
            _lock: lock,
            data,
        })
    }
}
impl Transaction<'_> {
    pub fn save(&self) -> Result<()> {
        validation::registry(&self.data)?;
        let bytes = serde_json::to_vec_pretty(&self.data)?;
        if bytes.len() + 1 > super::limits::MAX_REGISTRY_BYTES {
            return Err(Error::new("REGISTRY_TOO_LARGE", "Registry exceeds 8 MiB."));
        }
        let path = self
            .registry
            .paths
            .state
            .join(format!(".registry-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = private_file(&path, true, true, true)?;
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&path, &self.registry.paths.registry)?;
            File::open(&self.registry.paths.state)?.sync_all()?;
            Ok(())
        })();
        // Only remove this invocation's temporary file; never unlink the lock.
        let _ = fs::remove_file(path);
        result
    }
}
