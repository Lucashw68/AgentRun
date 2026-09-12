use super::{
    error::{Error, Result},
    paths,
    types::{Owner, OwnerType, ProfileRequest, StartRequest},
    validation,
};
use serde::{Deserialize, Serialize};
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
    #[serde(default)]
    command: Option<Vec<String>>,
    #[serde(default, rename = "make")]
    make_target: Option<MakeTarget>,
    #[serde(default)]
    compose: Option<ComposeProfile>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MakeTarget {
    pub file: String,
    pub target: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposeProfile {
    pub files: Vec<String>,
    #[serde(default)]
    pub env_files: Vec<String>,
    #[serde(default = "default_socket")]
    pub socket: String,
    #[serde(default)]
    pub prepare_make: Option<MakeTarget>,
    #[serde(default)]
    pub build: bool,
    #[serde(default)]
    pub pull: PullPolicy,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PullPolicy {
    #[default]
    Never,
    Missing,
}
fn default_socket() -> String {
    "/var/run/docker.sock".into()
}

pub fn project_file(cwd: &Path, name: &str) -> Result<String> {
    let file = fs::canonicalize(cwd.join(name))?;
    if !file.starts_with(cwd) || !file.is_file() {
        return Err(Error::new(
            "FILE_NOT_ALLOWED",
            "Recipe files must resolve to regular files within cwd.",
        ));
    }
    paths::path_string(&file)
}
fn make_command(cwd: &Path, target: &MakeTarget) -> Result<Vec<String>> {
    validation::id(&target.target)?;
    Ok(vec![
        paths::path_string(&executable("make", cwd, true)?)?,
        "--file".into(),
        project_file(cwd, &target.file)?,
        "--".into(),
        target.target.clone(),
    ])
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
fn read_profile(
    config_path: &Path,
    input: &ProfileRequest,
    client: &str,
) -> Result<(Profile, PathBuf)> {
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
        if usize::from(profile.command.is_some())
            + usize::from(profile.make_target.is_some())
            + usize::from(profile.compose.is_some())
            != 1
        {
            return Err(Error::new(
                "CONFIG_ERROR",
                "A profile must have exactly one of command, make or compose.",
            ));
        }
        if let Some(command) = &profile.command {
            validation::command(command)?;
        }
        if let Some(target) = &profile.make_target {
            validation::id(&target.target)?;
        }
    }
    let profile = config
        .profiles
        .into_iter()
        .find(|(name, _)| name == &input.profile)
        .map(|(_, p)| p)
        .ok_or_else(|| {
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
    Ok((profile, cwd))
}
pub fn resolve(config_path: &Path, input: ProfileRequest, client: &str) -> Result<StartRequest> {
    let (profile, cwd) = read_profile(config_path, &input, client)?;
    let mut command = if let Some(command) = profile.command {
        command
    } else if let Some(target) = profile.make_target {
        make_command(&cwd, &target)?
    } else {
        return Err(Error::new(
            "PROFILE_KIND",
            "Compose profiles require start_stack.",
        ));
    };
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
pub fn resolve_compose(
    config_path: &Path,
    input: &ProfileRequest,
    client: &str,
) -> Result<(ComposeProfile, String)> {
    let (profile, cwd) = read_profile(config_path, input, client)?;
    let mut recipe = profile
        .compose
        .ok_or_else(|| Error::new("PROFILE_KIND", "A Compose profile is required."))?;
    if recipe.files.is_empty() || recipe.files.len() > 8 || recipe.env_files.len() > 8 {
        return Err(Error::new(
            "CONFIG_ERROR",
            "Compose needs 1..8 files and at most 8 envFiles.",
        ));
    }
    for file in &mut recipe.files {
        *file = project_file(&cwd, file)?;
    }
    for file in &mut recipe.env_files {
        *file = if recipe.prepare_make.is_some() {
            future_project_file(&cwd, file)?
        } else {
            project_file(&cwd, file)?
        };
    }
    if let Some(target) = &mut recipe.prepare_make {
        validation::id(&target.target)?;
        target.file = project_file(&cwd, &target.file)?;
        make_command(&cwd, target)?;
    }
    recipe.socket = paths::path_string(&fs::canonicalize(paths::expand_home(&recipe.socket)?)?)?;
    use std::os::unix::fs::FileTypeExt;
    if !fs::metadata(&recipe.socket)?.file_type().is_socket() {
        return Err(Error::new(
            "LOCAL_ENGINE_REQUIRED",
            "Compose requires a local Unix socket.",
        ));
    }
    Ok((recipe, paths::path_string(&cwd)?))
}
pub fn preparation(cwd: &Path, target: &MakeTarget) -> Result<Vec<String>> {
    make_command(cwd, target)
}

// Preparation may generate env files. Resolve existing ancestors first, reject
// parent traversal, and recheck the complete realpath after preparation.
fn future_project_file(cwd: &Path, name: &str) -> Result<String> {
    let full = cwd.join(name);
    if full
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(Error::new(
            "FILE_NOT_ALLOWED",
            "Generated env file paths cannot contain parent traversal.",
        ));
    }
    let mut ancestor = full.as_path();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        suffix.push(
            ancestor
                .file_name()
                .ok_or_else(|| Error::new("FILE_NOT_ALLOWED", "Invalid env file path."))?
                .to_os_string(),
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| Error::new("FILE_NOT_ALLOWED", "Invalid env file path."))?;
    }
    let mut resolved = fs::canonicalize(ancestor)?;
    for component in suffix.iter().rev() {
        resolved.push(component);
    }
    if !resolved.starts_with(cwd) {
        return Err(Error::new(
            "FILE_NOT_ALLOWED",
            "Generated env files must remain inside cwd.",
        ));
    }
    paths::path_string(&resolved)
}
