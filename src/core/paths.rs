use super::error::{Error, Result};
use std::{
    env,
    fs::{self, File, OpenOptions},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct Paths {
    pub state: PathBuf,
    pub registry: PathBuf,
    pub logs: PathBuf,
    pub locks: PathBuf,
    pub config: PathBuf,
}
impl Paths {
    pub fn from_environment() -> Result<Self> {
        let home = home()?;
        let state_base = env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local/state"));
        let config_base = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        Ok(Self::new(
            state_base.join("agentrun"),
            config_base.join("agentrun/config.json"),
        ))
    }
    pub fn new(state: PathBuf, config: PathBuf) -> Self {
        Self {
            registry: state.join("registry.json"),
            logs: state.join("logs"),
            locks: state.join("locks"),
            state,
            config,
        }
    }
    pub fn prepare(&self) -> Result<()> {
        for p in [&self.state, &self.logs, &self.locks] {
            private_directory(p)?;
        }
        Ok(())
    }
}
pub fn uid() -> u32 {
    // SAFETY: getuid has no arguments or memory-safety requirements.
    unsafe { libc::getuid() }
}
pub fn home() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| Error::new("CONFIG_ERROR", "HOME must be an absolute directory."))
}
pub fn expand_home(path: &str) -> Result<PathBuf> {
    if path == "~" {
        home()
    } else if let Some(suffix) = path.strip_prefix("~/") {
        Ok(home()?.join(suffix))
    } else {
        Ok(PathBuf::from(path))
    }
}
pub fn path_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::new("VALIDATION_ERROR", "AgentRun paths must be valid UTF-8."))
}
pub fn canonical_directory(path: &Path) -> Result<PathBuf> {
    let path = fs::canonicalize(path)?;
    if !path.is_dir() {
        return Err(Error::new("INVALID_CWD", "cwd must be a directory."));
    }
    Ok(path)
}
pub fn private_directory(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    let st = fs::symlink_metadata(path)?;
    if !st.is_dir() || st.uid() != uid() || st.mode() & 0o077 != 0 {
        return Err(Error::new(
            "UNSAFE_STATE",
            format!(
                "Expected a private owned directory (0700): {}",
                path.display()
            ),
        ));
    }
    Ok(())
}
pub fn private_file(path: &Path, write: bool, create: bool, exclusive: bool) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(write)
        .create(create)
        .create_new(exclusive)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let st = file.metadata()?;
    if !st.is_file() || st.uid() != uid() || st.mode() & 0o077 != 0 || st.nlink() != 1 {
        return Err(Error::new(
            "UNSAFE_FILE",
            format!("Expected a private owned regular file: {}", path.display()),
        ));
    }
    Ok(file)
}
pub(crate) fn cstring(path: &Path) -> Result<std::ffi::CString> {
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::new("VALIDATION_ERROR", "NUL byte in path"))
}
