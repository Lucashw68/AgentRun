use super::{
    error::{Error, Result},
    types::*,
};
use std::{collections::HashSet, path::Path};

pub fn id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 80
        || !id.as_bytes()[0].is_ascii_alphanumeric()
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return Err(Error::new(
            "VALIDATION_ERROR",
            "IDs must match [a-zA-Z0-9][a-zA-Z0-9._-]* (1..80 characters).",
        ));
    }
    Ok(())
}
pub fn command(command: &[String]) -> Result<()> {
    if command.is_empty()
        || command
            .iter()
            .map(|a| a.len().saturating_add(1))
            .sum::<usize>()
            > super::limits::MAX_COMMAND_BYTES
        || command.len() > 256
        || command[0].is_empty()
        || command.iter().any(|a| a.len() > 131072 || a.contains('\0'))
    {
        return Err(Error::new("VALIDATION_ERROR", "Invalid executable/argv."));
    }
    Ok(())
}
pub fn owner(owner: &Owner) -> Result<()> {
    if let Some(client) = &owner.client {
        id(client)?;
    }
    Ok(())
}
pub fn tail(tail: usize) -> Result<()> {
    if tail > 10000 {
        return Err(Error::new(
            "VALIDATION_ERROR",
            "tail must be between 0 and 10000.",
        ));
    }
    Ok(())
}
pub fn registry(data: &RegistryData) -> Result<()> {
    let mut ids = HashSet::new();
    let valid = data.version == 1
        && data.processes.len() <= super::limits::MAX_PROCESSES
        && data.processes.iter().all(|p| {
            id(&p.id).is_ok()
                && p.profile.as_deref().is_none_or(|name| id(name).is_ok())
                && command(&p.command).is_ok()
                && owner(&p.owner).is_ok()
                && ids.insert(&p.id)
                && p.pid >= 2
                && p.pid == p.pgid
                && Path::new(&p.cwd).is_absolute()
                && Path::new(&p.log_path).is_absolute()
                && p.process_start_time.parse::<u64>().is_ok()
                && uuid::Uuid::parse_str(&p.boot_id).is_ok()
                && !p.ports.contains(&0)
                && time::OffsetDateTime::parse(
                    &p.started_at,
                    &time::format_description::well_known::Rfc3339,
                )
                .is_ok()
        });
    let valid = valid
        && data.stacks.len() <= 16
        && data
            .stacks
            .iter()
            .all(|s| super::compose::validate(s) && ids.insert(&s.id));
    if !valid {
        return Err(Error::new(
            "REGISTRY_INVALID",
            "Invalid registry schema; refusing to replace it.",
        ));
    }
    Ok(())
}
