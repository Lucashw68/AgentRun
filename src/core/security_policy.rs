use super::{
    error::{Error, Result},
    paths,
    types::{Owner, OwnerType, ProfileRequest, StartRequest},
    validation,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    env, fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    allowed_roots: Vec<String>,
    profiles: HashMap<String, Profile>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    command: Vec<String>,
}
const FORBIDDEN: &[&str] = &[
    "sudo", "su", "doas", "pkexec", "bash", "sh", "dash", "ash", "zsh", "ksh", "fish", "env",
    "busybox",
];
fn forbidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|name| FORBIDDEN.contains(&name))
}

/// Resolve executable before fork; execve never invokes a fallback shell.
pub fn executable(name: &str, cwd: &Path, restricted: bool) -> Result<PathBuf> {
    if restricted
        && (forbidden(Path::new(name)) || (name.contains('/') && !Path::new(name).is_absolute()))
    {
        return Err(Error::new(
            "FORBIDDEN_PROFILE",
            "Shell, privilege/wrapper executables and relative executable paths are forbidden in MCP profiles.",
        ));
    }
    let candidates = if name.contains('/') {
        vec![cwd.join(name)]
    } else {
        env::split_paths(&env::var_os("PATH").unwrap_or_default())
            .filter(|p| !restricted || p.is_absolute())
            .map(|p| cwd.join(p).join(name))
            .collect()
    };
    for candidate in candidates {
        if let Ok(path) = fs::canonicalize(candidate)
            && let Ok(meta) = fs::metadata(&path)
            && meta.is_file()
            && meta.permissions().mode() & 0o111 != 0
        {
            if restricted && forbidden(&path) {
                return Err(Error::new(
                    "FORBIDDEN_PROFILE",
                    "Executable resolves to a forbidden shell or privilege tool.",
                ));
            }
            return Ok(path);
        }
    }
    Err(Error::new(
        "EXECUTABLE_NOT_FOUND",
        format!("Executable not found: {name}"),
    ))
}
pub fn resolve(config_path: &Path, input: ProfileRequest, client: &str) -> Result<StartRequest> {
    validation::id(&input.id)?;
    validation::id(&input.profile)?;
    validation::id(client)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(config_path)
        .map_err(|_| {
            Error::new(
                "CONFIG_ERROR",
                format!("Create a valid MCP policy at {}", config_path.display()),
            )
        })?;
    if !file.metadata()?.is_file() {
        return Err(Error::new("CONFIG_ERROR", "Policy must be a regular file."));
    }
    let bytes =
        super::limits::read_bounded(file, super::limits::MAX_CONFIG_BYTES, "CONFIG_TOO_LARGE")?;
    let config: Config =
        serde_json::from_slice(&bytes).map_err(|e| Error::new("CONFIG_ERROR", e.to_string()))?;
    for (name, profile) in &config.profiles {
        validation::id(name)?;
        validation::command(&profile.command)?;
    }
    let profile = config.profiles.get(&input.profile).ok_or_else(|| {
        Error::new(
            "UNKNOWN_PROFILE",
            format!("Unknown profile: {}", input.profile),
        )
    })?;
    if !Path::new(&input.cwd).is_absolute() {
        return Err(Error::new("INVALID_CWD", "MCP cwd must be absolute."));
    }
    let cwd = paths::canonical_directory(Path::new(&input.cwd))?;
    let mut allowed = false;
    for root in config.allowed_roots {
        let root = paths::expand_home(&root)?;
        if !root.is_absolute() {
            return Err(Error::new(
                "CONFIG_ERROR",
                "allowedRoots must be absolute or begin with ~/.",
            ));
        }
        if let Ok(root) = fs::canonicalize(root) {
            allowed |= root.is_dir() && cwd.starts_with(root);
        }
    }
    if !allowed {
        return Err(Error::new(
            "CWD_NOT_ALLOWED",
            "cwd is outside allowedRoots after realpath resolution.",
        ));
    }
    let mut command = profile.command.clone();
    command[0] = paths::path_string(&executable(&command[0], &cwd, true)?)?;
    Ok(StartRequest {
        id: input.id,
        cwd: Some(paths::path_string(&cwd)?),
        command,
        owner: Owner {
            kind: OwnerType::Agent,
            client: Some(client.to_owned()),
        },
    })
}
