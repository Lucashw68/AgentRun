//! Internal pipe collector; no commands, registry operations or network.
fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    let result = if args.len() == 2 {
        agentrun::core::logs::collect(std::path::Path::new(&args[1]))
    } else {
        Err(agentrun::core::Error::new(
            "USAGE",
            "Internal helper: agentrun-log <managed-log-path>",
        ))
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("agentrun-log: {}", e.message);
            std::process::ExitCode::FAILURE
        }
    }
}
