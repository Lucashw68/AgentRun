//! Deliberately fixed MVP budgets, shared by both interfaces.
use super::{Error, Result};
use std::io::Read;

pub const MAX_PROCESSES: usize = 64;
pub const MAX_REGISTRY_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_CONFIG_BYTES: usize = 256 * 1024;
pub const MAX_COMMAND_BYTES: usize = 64 * 1024;
pub const LOG_SEGMENT_BYTES: u64 = 1024 * 1024;
pub const RETAINED_LOG_RUNS: usize = 20;

pub fn read_bounded(reader: impl Read, limit: usize, code: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(Error::new(code, format!("Input exceeds {limit} bytes.")));
    }
    Ok(bytes)
}
