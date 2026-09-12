use super::{
    error::{Error, Result},
    limits::{LOG_SEGMENT_BYTES, RETAINED_LOG_RUNS, read_bounded},
    paths::{Paths, private_directory, private_file},
    types::{LogsResult, ManagedProcess},
    validation,
};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub fn create(paths: &Paths, id: &str) -> Result<(String, File)> {
    validation::id(id)?;
    let path = paths
        .logs
        .join(format!("{id}-{}.log", uuid::Uuid::new_v4()));
    let file = private_file(&path, true, true, true)?;
    Ok((super::paths::path_string(&path)?, file))
}
fn backup(path: &Path) -> PathBuf {
    path.with_extension("log.1")
}
fn managed_name(path: &Path) -> bool {
    let Some(stem) = path
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_suffix(".log"))
    else {
        return false;
    };
    let Some(split) = stem.len().checked_sub(37) else {
        return false;
    };
    stem.is_char_boundary(split)
        && stem.as_bytes()[split] == b'-'
        && validation::id(&stem[..split]).is_ok()
        && uuid::Uuid::parse_str(&stem[split + 1..]).is_ok()
}
fn lock(file: &File, nonblocking: bool) -> Result<bool> {
    // SAFETY: flock borrows the live owned file descriptor; no pointers.
    if unsafe {
        libc::flock(
            file.as_raw_fd(),
            libc::LOCK_EX | if nonblocking { libc::LOCK_NB } else { 0 },
        )
    } == 0
    {
        return Ok(true);
    }
    let e = std::io::Error::last_os_error();
    if e.kind() == std::io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(e.into())
    }
}

/// Launch a minimal detached collector. Only the managed command receives its
/// write end; EOF makes the collector exit even after the original CLI exits.
/// Never search PATH or the project's cwd for the internal executable.
pub(crate) fn capture(paths: &Paths, id: &str) -> Result<(String, File)> {
    let executable = std::env::current_exe()?;
    let directory = executable
        .parent()
        .ok_or_else(|| Error::new("LOG_HELPER_MISSING", "No executable directory"))?;
    let mut helper = directory.join("agentrun-log");
    // Cargo places integration test executables in target/<profile>/deps.
    if directory.file_name().is_some_and(|s| s == "deps") {
        helper = directory.parent().unwrap().join("agentrun-log");
    }
    let (path, initial) = create(paths, id)?;
    drop(initial);
    let mut command = Command::new(helper);
    command
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // SAFETY: the pre-exec callback only calls async-signal-safe setsid and
    // constructs an OS error on failure, as required by CommandExt.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command.spawn().map_err(|e| {
        Error::new(
            "LOG_HELPER_MISSING",
            format!("Cannot launch sibling agentrun-log: {e}"),
        )
    })?;
    let prepared = (|| {
        let mut ready = child.stdout.take().unwrap();
        super::launch::readable(&ready)?;
        let mut byte = [0];
        ready.read_exact(&mut byte)?;
        if byte != [1] {
            return Err(Error::new("LOG_ERROR", "Collector failed to initialize"));
        }
        super::launch::reap(child.id() as i32)?;
        let fd: OwnedFd = child.stdin.take().unwrap().into();
        Ok((path, File::from(fd)))
    })();
    if prepared.is_err() {
        // This direct child has not been given to the reaper on any error path.
        let _ = child.kill();
        let _ = child.wait();
    }
    prepared
}

/// Core implementation of the internal collector, entered in a fresh process.
/// Keeps a lock on the current inode for its entire lifetime. Rotation copies
/// into one bounded backup, then truncates the same inode, preserving the lease.
pub fn collect(path: &Path) -> Result<()> {
    if !path.is_absolute() || !managed_name(path) {
        return Err(Error::new(
            "UNSAFE_LOG_PATH",
            "Expected an absolute AgentRun UUID log path.",
        ));
    }
    private_directory(path.parent().unwrap())?;
    let mut file = private_file(path, true, false, false)?;
    if !lock(&file, true)? {
        return Err(Error::new("LOG_BUSY", "Log already has a collector."));
    }
    let mut size = file.metadata()?.len();
    if size != 0 {
        return Err(Error::new(
            "LOG_ERROR",
            "Collector requires a newly created empty log.",
        ));
    }
    std::io::stdout().write_all(&[1])?;
    std::io::stdout().flush()?;
    let mut input = std::io::stdin().lock();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let n = match input.read(&mut buffer) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if n == 0 {
            return Ok(());
        }
        let mut bytes = &buffer[..n];
        while !bytes.is_empty() {
            if size == LOG_SEGMENT_BYTES {
                let mut previous = private_file(&backup(path), true, true, false)?;
                previous.set_len(0)?;
                file.seek(SeekFrom::Start(0))?;
                std::io::copy(
                    &mut Read::by_ref(&mut file).take(LOG_SEGMENT_BYTES),
                    &mut previous,
                )?;
                file.set_len(0)?;
                file.seek(SeekFrom::Start(0))?;
                size = 0;
            }
            let n = bytes.len().min((LOG_SEGMENT_BYTES - size) as usize);
            file.write_all(&bytes[..n])?;
            size += n as u64;
            bytes = &bytes[n..];
        }
    }
}

/// Remove only old, unreferenced, unlocked UUID log generations. An active
/// collector's flock prevents removal even if its registry entry was cleaned.
pub(crate) fn prune(paths: &Paths, records: &[ManagedProcess]) -> Result<()> {
    let referenced: HashSet<_> = records.iter().map(|p| Path::new(&p.log_path)).collect();
    let mut candidates = Vec::new();
    for entry in fs::read_dir(&paths.logs)? {
        let path = entry?.path();
        if !managed_name(&path) || referenced.contains(path.as_path()) {
            continue;
        }
        // Refuse suspicious files rather than following symlinks/hard links.
        let file = private_file(&path, true, false, false)?;
        candidates.push((file.metadata()?.modified()?, path));
    }
    candidates.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, path) in candidates.into_iter().skip(RETAINED_LOG_RUNS) {
        let file = private_file(&path, true, false, false)?;
        if !lock(&file, true)? {
            continue;
        }
        let previous = backup(&path);
        if fs::symlink_metadata(&previous).is_ok() {
            private_file(&previous, false, false, false)?;
            fs::remove_file(previous)?;
        }
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn read(paths: &Paths, record: &ManagedProcess, tail: usize) -> Result<LogsResult> {
    validation::tail(tail)?;
    let path = Path::new(&record.log_path);
    if path.parent() != Some(paths.logs.as_path())
        || !path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.starts_with(&format!("{}-", record.id)) && s.ends_with(".log"))
    {
        return Err(Error::new(
            "UNSAFE_LOG_PATH",
            "Log path is outside the managed log directory.",
        ));
    }
    let mut file = private_file(path, false, false, false)?;
    let size = file.metadata()?.len();
    let bytes = size.min(LOG_SEGMENT_BYTES);
    file.seek(SeekFrom::Start(size - bytes))?;
    let current = read_bounded(file.take(bytes), LOG_SEGMENT_BYTES as usize, "LOG_ERROR")?;
    let previous_path = backup(path);
    let mut previous_bytes = Vec::new();
    let mut rotated = false;
    match private_file(&previous_path, false, false, false) {
        Ok(mut previous) => {
            rotated = true;
            let size = previous.metadata()?.len();
            let take = size.min(LOG_SEGMENT_BYTES - current.len() as u64);
            previous.seek(SeekFrom::Start(size - take))?;
            previous_bytes =
                read_bounded(previous.take(take), LOG_SEGMENT_BYTES as usize, "LOG_ERROR")?;
        }
        Err(_)
            if fs::symlink_metadata(&previous_path)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) => {}
        Err(e) => return Err(e),
    }
    previous_bytes.extend_from_slice(&current);
    let truncated = size > bytes || rotated;
    let text = String::from_utf8_lossy(&previous_bytes);
    let text = if truncated {
        text.split_once('\n')
            .map_or(text.as_ref(), |(_, rest)| rest)
    } else {
        &text
    };
    let newline = text.ends_with('\n');
    let lines: Vec<_> = text
        .strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .collect();
    let mut result = if tail == 0 {
        String::new()
    } else {
        lines[lines.len().saturating_sub(tail)..].join("\n")
    };
    if newline && tail > 0 {
        result.push('\n');
    }
    Ok(LogsResult {
        id: record.id.clone(),
        text: result,
        truncated,
    })
}

/// Strip terminal escape/control sequences from human output. Raw JSON logs
/// retain their original content; they must be treated as untrusted data.
pub fn terminal_safe(text: &str, multiline: bool) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') | Some('P') | Some('^') | Some('_') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next() == Some('\\')) {
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if !c.is_control() || (multiline && (c == '\n' || c == '\t')) {
            out.push(c);
        } else if !multiline {
            out.push(' ');
        }
    }
    out
}
