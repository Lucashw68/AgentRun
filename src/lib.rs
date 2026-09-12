#[cfg(not(target_os = "linux"))]
compile_error!("AgentRun requires Linux 6.9 or newer.");

pub mod core;
pub mod mcp;
