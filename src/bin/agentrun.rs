use agentrun::core::{
    AgentRun, Error, ManagedProcess, Owner, OwnerType, ProfileRequest, Result, StartRequest,
    Status, logs::terminal_safe,
};
use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(
    version,
    about = "Local persistent development processes",
    after_help = "MCP server: agentrun-mcp (stdio only)"
)]
struct Cli {
    #[command(subcommand)]
    command: Action,
}
#[derive(Args)]
struct Output {
    #[arg(long)]
    json: bool,
}
#[derive(Args)]
struct ById {
    id: String,
    #[command(flatten)]
    output: Output,
}
#[derive(Subcommand)]
enum Action {
    /// Manage local Docker Compose containers through approved profiles.
    Stack {
        #[command(subcommand)]
        command: StackAction,
    },
    List(Output),
    Status(ById),
    /// Stop safely, then relaunch the recorded command and working directory.
    Restart(ById),
    Start {
        id: String,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long, value_parser = ["agent", "manual"])]
        owner: Option<String>,
        #[arg(long)]
        client: Option<String>,
        #[command(flatten)]
        output: Output,
        #[arg(long, conflicts_with = "command")]
        profile: Option<String>,
        #[arg(last = true, required_unless_present = "profile", num_args = 1..)]
        command: Vec<String>,
    },
    Stop {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        id: Option<String>,
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        output: Output,
    },
    Logs {
        id: String,
        #[arg(long, default_value_t = 100)]
        tail: usize,
        #[command(flatten)]
        output: Output,
    },
    Clean(Output),
    Ports(Output),
}

#[derive(Subcommand)]
enum StackAction {
    List(Output),
    Status(ById),
    Start {
        id: String,
        #[arg(long)]
        profile: String,
        #[arg(long)]
        cwd: Option<String>,
        #[command(flatten)]
        output: Output,
    },
    Stop(ById),
    Restart(ById),
    Logs {
        id: String,
        #[arg(long, default_value_t = 100)]
        tail: usize,
        #[command(flatten)]
        output: Output,
    },
}
fn stack_table(stacks: &[agentrun::core::compose::ManagedStack]) -> String {
    let mut rows = vec![vec![
        "STACK".into(),
        "STATUS".into(),
        "CONTAINERS".into(),
        "PORTS".into(),
    ]];
    rows.extend(stacks.iter().map(|s| {
        vec![
            s.id.clone(),
            serde_json::to_value(&s.status)
                .unwrap()
                .as_str()
                .unwrap()
                .into(),
            s.containers.len().to_string(),
            ports(
                &s.containers
                    .iter()
                    .flat_map(|c| c.ports.clone())
                    .collect::<Vec<_>>(),
            ),
        ]
    }));
    table(rows)
}

fn table(rows: Vec<Vec<String>>) -> String {
    let rows: Vec<Vec<String>> = rows
        .into_iter()
        .map(|row| row.into_iter().map(|s| terminal_safe(&s, false)).collect())
        .collect();
    let widths: Vec<_> = (0..rows[0].len())
        .map(|i| rows.iter().map(|row| row[i].len()).max().unwrap_or(0))
        .collect();
    rows.into_iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(i, cell)| format!("{cell:<width$}", width = widths[i]))
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn status(status: Status) -> &'static str {
    if status == Status::Running {
        "running"
    } else {
        "dead"
    }
}
fn ports(ports: &[u16]) -> String {
    if ports.is_empty() {
        "-".into()
    } else {
        ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}
fn process_table(processes: &[ManagedProcess]) -> String {
    let mut rows = vec![vec![
        "NAME".into(),
        "STATUS".into(),
        "PID".into(),
        "PORTS".into(),
        "OWNER".into(),
    ]];
    rows.extend(processes.iter().map(|p| {
        vec![
            p.id.clone(),
            status(p.status).into(),
            if p.status == Status::Running {
                p.pid.to_string()
            } else {
                "-".into()
            },
            ports(&p.ports),
            p.owner.client.clone().unwrap_or_else(|| {
                if p.owner.kind == OwnerType::Agent {
                    "agent"
                } else {
                    "manual"
                }
                .into()
            }),
        ]
    }));
    table(rows)
}
fn run(cli: Cli) -> Result<(Value, String, bool)> {
    let core = AgentRun::from_environment()?;
    let (data, human, failed) = match cli.command {
        Action::Stack { command } => match command {
            StackAction::List(_) => {
                let stacks = core.list_stacks()?;
                let human = stack_table(&stacks);
                (json!({"stacks": stacks}), human, false)
            }
            StackAction::Logs { id, tail, .. } => {
                let logs = core.stack_logs(&id, tail)?;
                let human = terminal_safe(&logs.text, true);
                (serde_json::to_value(logs)?, human, false)
            }
            action => {
                let stack = match action {
                    StackAction::Status(input) => core.get_stack(&input.id)?,
                    StackAction::Stop(input) => core.stop_stack(&input.id)?,
                    StackAction::Restart(input) => core.restart_stack(&input.id)?,
                    StackAction::Start {
                        id, cwd, profile, ..
                    } => core.start_stack(
                        ProfileRequest {
                            id,
                            profile,
                            cwd: cwd
                                .unwrap_or(std::env::current_dir()?.to_string_lossy().into_owned()),
                        },
                        None,
                    )?,
                    _ => unreachable!(),
                };
                let human = stack_table(std::slice::from_ref(&stack));
                (json!({"stack": stack}), human, false)
            }
        },
        Action::List(_) => {
            let processes = core.list()?;
            let stacks = core.list_stacks()?;
            let mut human = process_table(&processes);
            if !stacks.is_empty() {
                human.push_str(&format!("\n\n{}", stack_table(&stacks)));
            }
            (json!({"processes":processes,"stacks":stacks}), human, false)
        }
        Action::Status(input) => {
            let p = core.get(&input.id)?;
            let human = format!(
                "{}\nCWD: {}\nCOMMAND: {}\nLOG: {}",
                process_table(std::slice::from_ref(&p)),
                terminal_safe(&p.cwd, false),
                terminal_safe(&serde_json::to_string(&p.command)?, false),
                terminal_safe(&p.log_path, false)
            );
            (json!({"process":p}), human, false)
        }
        Action::Restart(input) => {
            let p = core.restart(&input.id)?;
            let human = process_table(std::slice::from_ref(&p));
            (json!({"process":p}), human, false)
        }
        Action::Start {
            id,
            cwd,
            owner,
            client,
            command,
            profile,
            ..
        } => {
            let kind = match owner.as_deref() {
                Some("manual") if client.is_some() => {
                    return Err(Error::new("USAGE", "--client requires agent ownership"));
                }
                Some("manual") => OwnerType::Manual,
                Some("agent") => OwnerType::Agent,
                _ if client.is_some() => OwnerType::Agent,
                _ => OwnerType::Manual,
            };
            let owner = Owner { kind, client };
            let p = if let Some(profile) = profile {
                core.start_configured(
                    ProfileRequest {
                        id,
                        profile,
                        cwd: cwd.unwrap_or(std::env::current_dir()?.to_string_lossy().into_owned()),
                    },
                    owner,
                )?
            } else {
                core.start(StartRequest {
                    id,
                    cwd,
                    command,
                    owner,
                })?
            };
            let human = process_table(std::slice::from_ref(&p));
            (json!({"process":p}), human, false)
        }
        Action::Stop { all: true, .. } => {
            let result = core.stop_all()?;
            let mut human = format!("Stopped: {}", result.stopped.join(", "));
            for e in &result.errors {
                human.push_str(&format!("\n{}: {}", e.id, terminal_safe(&e.message, false)));
            }
            let failed = !result.errors.is_empty();
            (serde_json::to_value(result)?, human, failed)
        }
        Action::Stop { id, .. } => {
            let p = core.stop(
                id.as_deref()
                    .ok_or_else(|| Error::new("USAGE", "Missing ID"))?,
            )?;
            let human = process_table(std::slice::from_ref(&p));
            (json!({"process":p}), human, false)
        }
        Action::Logs { id, tail, .. } => {
            let result = core.get_logs(&id, tail)?;
            let human = terminal_safe(&result.text, true);
            if result.truncated {
                eprintln!("Log read limited to the last 1 MiB.");
            }
            (serde_json::to_value(result)?, human, false)
        }
        Action::Clean(_) => {
            let result = core.clean()?;
            let human = format!("Removed: {}", result.removed.join(", "));
            (serde_json::to_value(result)?, human, false)
        }
        Action::Ports(_) => {
            let processes = core.ports()?;
            let mut rows = vec![vec!["NAME".into(), "STATUS".into(), "PORTS".into()]];
            rows.extend(
                processes
                    .iter()
                    .map(|p| vec![p.id.clone(), status(p.status).into(), ports(&p.ports)]),
            );
            let stacks = core.list_stacks()?;
            let stack_ports: Vec<_> = stacks
                .iter()
                .map(|s| {
                    let mut published: Vec<_> =
                        s.containers.iter().flat_map(|c| c.ports.clone()).collect();
                    published.sort_unstable();
                    published.dedup();
                    rows.push(vec![
                        s.id.clone(),
                        serde_json::to_value(&s.status)
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .into(),
                        ports(&published),
                    ]);
                    json!({"id":s.id,"status":s.status,"ports":published})
                })
                .collect();
            (
                json!({"processes":processes,"stacks":stack_ports}),
                table(rows),
                false,
            )
        }
    };
    Ok((data, human, failed))
}
fn main() -> std::process::ExitCode {
    let json_output = std::env::args()
        .take_while(|arg| arg != "--")
        .any(|arg| arg == "--json");
    let parsed = Cli::try_parse();
    let result = match parsed {
        Ok(cli) => run(cli),
        Err(e)
            if matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            print!("{e}");
            return std::process::ExitCode::SUCCESS;
        }
        Err(e) => Err(Error::new("USAGE", e.to_string())),
    };
    match result {
        Ok((mut data, human, failed)) => {
            data["version"] = json!(1);
            if json_output {
                println!("{data}");
            } else if human.ends_with('\n') {
                print!("{human}");
            } else {
                println!("{human}");
            }
            if failed {
                std::process::ExitCode::FAILURE
            } else {
                std::process::ExitCode::SUCCESS
            }
        }
        Err(e) => {
            if json_output {
                eprintln!("{}", json!({"version":1,"error":e}));
            } else {
                eprintln!("agentrun: {}", terminal_safe(&e.message, true));
            }
            std::process::ExitCode::FAILURE
        }
    }
}
