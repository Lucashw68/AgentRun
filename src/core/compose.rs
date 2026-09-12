//! Native local Compose inventory. Container IDs, never the Docker client PID,
//! are the destructive-operation authority. No down/rm/prune operation exists.
use super::{Error, Result, command_runner, paths, security_policy, validation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, path::Path, process::Command, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Container {
    pub id: String,
    pub service: String,
    pub state: String,
    pub health: Option<String>,
    pub exit_code: Option<i64>,
    pub ports: Vec<u16>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StackStatus {
    Creating,
    Running,
    Partial,
    Stopped,
    Missing,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedStack {
    pub id: String,
    pub cwd: String,
    pub profile: String,
    pub owner: super::Owner,
    pub started_at: String,
    pub project: String,
    pub engine_id: String,
    pub docker: String,
    pub recipe: security_policy::ComposeProfile,
    pub containers: Vec<Container>,
    pub status: StackStatus,
    pub reason: Option<String>,
}
const TIMEOUT: Duration = Duration::from_secs(20);
pub fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn validate(stack: &ManagedStack) -> bool {
    validation::id(&stack.id).is_ok()
        && validation::id(&stack.profile).is_ok()
        && validation::owner(&stack.owner).is_ok()
        && time::OffsetDateTime::parse(
            &stack.started_at,
            &time::format_description::well_known::Rfc3339,
        )
        .is_ok()
        && stack.recipe.prepare_make.as_ref().is_none_or(|m| {
            validation::id(&m.target).is_ok()
                && Path::new(&m.file).is_absolute()
                && Path::new(&m.file).starts_with(&stack.cwd)
        })
        && stack
            .project
            .strip_prefix("agentrun-")
            .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
        && Path::new(&stack.cwd).is_absolute()
        && Path::new(&stack.docker).is_absolute()
        && Path::new(&stack.recipe.socket).is_absolute()
        && !stack.engine_id.is_empty()
        && stack.engine_id.len() < 256
        && stack.containers.len() <= 64
        && stack.recipe.files.len() <= 8
        && !stack.recipe.files.is_empty()
        && stack.recipe.env_files.len() <= 8
        && stack
            .recipe
            .files
            .iter()
            .chain(&stack.recipe.env_files)
            .all(|p| Path::new(p).is_absolute() && Path::new(p).starts_with(&stack.cwd))
        && stack
            .containers
            .iter()
            .all(|c| valid_id(&c.id) && validation::id(&c.service).is_ok())
        && stack
            .containers
            .iter()
            .map(|c| &c.id)
            .collect::<HashSet<_>>()
            .len()
            == stack.containers.len()
}
impl ManagedStack {
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.docker);
        command_runner::remove_control_environment(&mut command);
        command
            .current_dir("/")
            .arg("--host")
            .arg(format!("unix://{}", self.recipe.socket))
            .args(args);
        command
    }
    fn run(&self, args: &[&str]) -> Result<Vec<u8>> {
        command_runner::run(self.command(args), TIMEOUT)
    }
    pub fn engine(&self) -> Result<String> {
        let bytes = self.run(&["info", "--format", "{{.ID}}"])?;
        let id = String::from_utf8_lossy(&bytes).trim().to_owned();
        if id.is_empty() || id.len() > 255 {
            return Err(Error::new(
                "ENGINE_IDENTITY",
                "Docker returned no valid engine identity.",
            ));
        }
        Ok(id)
    }
    fn verify_engine(&self) -> Result<()> {
        if self.engine()? != self.engine_id {
            return Err(Error::new(
                "ENGINE_IDENTITY",
                "Docker engine identity changed; refusing operation.",
            ));
        }
        Ok(())
    }
    fn compose(&self, args: &[&str]) -> Result<Vec<u8>> {
        let mut command = self.command(&[
            "compose",
            "--project-name",
            &self.project,
            "--project-directory",
            &self.cwd,
        ]);
        command.current_dir(&self.cwd);
        for file in &self.recipe.files {
            command.args(["--file", file]);
        }
        for file in &self.recipe.env_files {
            command.args(["--env-file", file]);
        }
        command.args(args);
        command_runner::run(
            command,
            if args.first() == Some(&"create") {
                Duration::from_secs(300)
            } else {
                TIMEOUT
            },
        )
    }
    fn project_ids(&self) -> Result<Vec<String>> {
        let filter = format!("label=com.docker.compose.project={}", self.project);
        let bytes = self.run(&[
            "container",
            "ls",
            "--all",
            "--no-trunc",
            "--filter",
            &filter,
            "--format",
            "{{.ID}}",
        ])?;
        let ids: Vec<String> = String::from_utf8_lossy(&bytes)
            .lines()
            .map(str::to_owned)
            .collect();
        if ids.len() > 64 || ids.iter().any(|id| !valid_id(id)) {
            return Err(Error::new(
                "CONTAINER_LIMIT",
                "Invalid container IDs or more than 64 containers.",
            ));
        }
        Ok(ids)
    }
    fn inspect(&self, id: &str) -> Result<(Container, Value)> {
        if !valid_id(id) {
            return Err(Error::new(
                "CONTAINER_IDENTITY",
                "A full container ID is required.",
            ));
        }
        let values: Vec<Value> =
            serde_json::from_slice(&self.run(&["container", "inspect", id])?)?;
        let v = values
            .first()
            .ok_or_else(|| Error::new("CONTAINER_IDENTITY", "Container missing."))?;
        let labels = &v["Config"]["Labels"];
        let service = labels["com.docker.compose.service"]
            .as_str()
            .unwrap_or_default();
        if v["Id"] != id
            || labels["com.docker.compose.project"] != self.project
            || validation::id(service).is_err()
        {
            return Err(Error::new(
                "CONTAINER_IDENTITY",
                "Container identity or Compose labels do not match.",
            ));
        }
        let mut ports = Vec::new();
        if let Some(bindings) = v["NetworkSettings"]["Ports"].as_object() {
            for (key, values) in bindings {
                if key.ends_with("/tcp") {
                    for value in values.as_array().into_iter().flatten() {
                        if let Some(port) = value["HostPort"]
                            .as_str()
                            .and_then(|p| p.parse::<u16>().ok())
                            .filter(|p| *p > 0)
                        {
                            ports.push(port);
                        }
                    }
                }
            }
        }
        ports.sort_unstable();
        ports.dedup();
        Ok((
            Container {
                id: id.into(),
                service: service.into(),
                state: v["State"]["Status"].as_str().unwrap_or("unknown").into(),
                exit_code: v["State"]["ExitCode"].as_i64(),
                health: v["State"]["Health"]["Status"].as_str().map(str::to_owned),
                ports,
            },
            labels.clone(),
        ))
    }
    /// No adoption during reads, even after an interrupted create.
    pub fn refresh(&mut self) {
        if let Err(e) = self.refresh_inner() {
            self.status = StackStatus::Unknown;
            self.reason = Some(e.code);
            for container in &mut self.containers {
                container.ports.clear();
            }
        }
    }
    fn refresh_inner(&mut self) -> Result<()> {
        self.verify_engine()?;
        let ids = self.project_ids()?;
        if ids
            .iter()
            .any(|id| !self.containers.iter().any(|c| &c.id == id))
        {
            return Err(Error::new(
                "UNREGISTERED_CONTAINERS",
                "Project contains unregistered containers; refusing adoption.",
            ));
        }
        if self.containers.is_empty() {
            self.status = StackStatus::Unknown;
            self.reason = Some("Creation incomplete; no container identities committed. Inspect Docker before cleaning.".into());
            return Ok(());
        }
        for c in &mut self.containers {
            if !ids.contains(&c.id) {
                c.state = "missing".into();
                c.health = None;
                c.ports.clear();
            }
        }
        for index in 0..self.containers.len() {
            if ids.contains(&self.containers[index].id) {
                let (current, _) = self.inspect(&self.containers[index].id)?;
                if current.service != self.containers[index].service {
                    return Err(Error::new(
                        "CONTAINER_IDENTITY",
                        "Compose service identity changed.",
                    ));
                }
                self.containers[index] = current;
            }
        }
        self.status = if self.containers.iter().all(|c| c.state == "missing") {
            StackStatus::Missing
        } else if self
            .containers
            .iter()
            .all(|c| c.state == "running" && c.health.as_deref().is_none_or(|h| h == "healthy"))
        {
            StackStatus::Running
        } else if self
            .containers
            .iter()
            .all(|c| matches!(c.state.as_str(), "exited" | "created" | "dead"))
        {
            StackStatus::Stopped
        } else {
            StackStatus::Partial
        };
        self.reason = None;
        Ok(())
    }
    pub fn prepare(&self) -> Result<()> {
        self.verify_engine()?;
        if !self.project_ids()?.is_empty() {
            return Err(Error::new(
                "PROJECT_EXISTS",
                "Refusing an existing Compose project.",
            ));
        }
        if let Some(target) = &self.recipe.prepare_make {
            let args = security_policy::preparation(Path::new(&self.cwd), target)?;
            let mut cmd = Command::new(&args[0]);
            cmd.current_dir(&self.cwd).args(&args[1..]);
            command_runner::remove_control_environment(&mut cmd);
            command_runner::run(cmd, TIMEOUT)?;
        }
        for file in self.recipe.files.iter().chain(&self.recipe.env_files) {
            if security_policy::project_file(Path::new(&self.cwd), file)? != *file {
                return Err(Error::new(
                    "FILE_NOT_ALLOWED",
                    "Recipe path changed during preparation.",
                ));
            }
        }
        // Validate rendered configuration without saving/printing interpolated secrets.
        let config: Value =
            serde_json::from_slice(&self.compose(&["config", "--format", "json"])?)?;
        let services = config["services"]
            .as_object()
            .ok_or_else(|| Error::new("COMPOSE_CONFIG", "No Compose services."))?;
        if services.is_empty() || services.len() > 64 {
            return Err(Error::new("CONTAINER_LIMIT", "Expected 1..64 services."));
        }
        for service in services.values() {
            // Explicitly reject lifecycle features whose ownership/readiness is not implemented.
            if [
                "container_name",
                "profiles",
                "develop",
                "post_start",
                "pre_stop",
            ]
            .iter()
            .any(|k| service.get(k).is_some())
                || service["deploy"]
                    .get("replicas")
                    .is_some_and(|n| n.as_u64() != Some(1))
                || service.get("scale").is_some_and(|n| n.as_u64() != Some(1))
            {
                return Err(Error::new(
                    "UNSUPPORTED_COMPOSE",
                    "Fixed container names, profiles, scaling, watch and lifecycle hooks are not supported.",
                ));
            }
            if let Some(deps) = service["depends_on"].as_object() {
                for dep in deps.values() {
                    if dep["condition"].as_str().is_some_and(|c| {
                        !matches!(
                            c,
                            "service_started"
                                | "service_healthy"
                                | "service_completed_successfully"
                        )
                    }) {
                        return Err(Error::new(
                            "UNSUPPORTED_COMPOSE",
                            "Unsupported dependency condition.",
                        ));
                    }
                }
            }
        }
        self.verify_engine()?;
        if !self.project_ids()?.is_empty() {
            return Err(Error::new(
                "PROJECT_EXISTS",
                "Compose project appeared during preparation.",
            ));
        }
        Ok(())
    }
    pub fn create(&mut self) -> Result<()> {
        // Pull/build require explicit profile authorization. Never start here.
        let result = self.compose(&[
            "create",
            if self.recipe.build {
                "--build"
            } else {
                "--no-build"
            },
            "--pull",
            if self.recipe.pull == security_policy::PullPolicy::Never {
                "never"
            } else {
                "missing"
            },
            "--no-recreate",
        ]);
        // Capture partial creation too. These containers have not been started.
        self.verify_engine()?;
        self.containers = self
            .project_ids()?
            .iter()
            .map(|id| self.inspect(id).map(|(c, _)| c))
            .collect::<Result<_>>()?;
        result?;
        if self.containers.is_empty() {
            return Err(Error::new(
                "COMPOSE_CONFIG",
                "Compose created no containers.",
            ));
        }
        Ok(())
    }
    fn checked(&mut self) -> Result<()> {
        self.refresh_inner()?;
        if self.containers.is_empty()
            || matches!(self.status, StackStatus::Unknown | StackStatus::Missing)
        {
            return Err(Error::new(
                "CONTAINER_IDENTITY",
                "Stack has no verifiable container identities.",
            ));
        }
        Ok(())
    }
    pub fn start(&mut self) -> Result<()> {
        self.checked()?;
        if self.containers.iter().any(|c| c.state == "missing") {
            return Err(Error::new(
                "CONTAINER_IDENTITY",
                "A container was removed; restart never recreates containers.",
            ));
        }
        let mut pending = self.containers.clone();
        let mut started = HashSet::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while !pending.is_empty() {
            let mut progress = false;
            for index in (0..pending.len()).rev() {
                let (current, labels) = self.inspect(&pending[index].id)?;
                let dependencies = labels["com.docker.compose.depends_on"]
                    .as_str()
                    .unwrap_or_default();
                let mut ready = true;
                for dependency in dependencies.split(',').filter(|s| !s.is_empty()) {
                    let fields: Vec<_> = dependency.split(':').collect();
                    let service = fields[0];
                    if !started.contains(service) {
                        ready = false;
                        continue;
                    }
                    let container = self
                        .containers
                        .iter()
                        .find(|c| c.service == service)
                        .ok_or_else(|| {
                            Error::new("COMPOSE_DEPENDENCIES", "Dependency container missing.")
                        })?;
                    let (dependency, _) = self.inspect(&container.id)?;
                    ready &= match fields.get(1).copied().unwrap_or("service_started") {
                        "service_started" => true,
                        "service_healthy" => dependency.health.as_deref() == Some("healthy"),
                        "service_completed_successfully" => {
                            dependency.state == "exited" && dependency.exit_code == Some(0)
                        }
                        _ => {
                            return Err(Error::new(
                                "COMPOSE_DEPENDENCIES",
                                "Unsupported dependency condition.",
                            ));
                        }
                    };
                }
                if ready {
                    self.verify_engine()?;
                    self.run(&["container", "start", &current.id])?;
                    started.insert(current.service);
                    pending.remove(index);
                    progress = true;
                }
            }
            if !progress {
                if std::time::Instant::now() >= deadline {
                    return Err(Error::new(
                        "COMPOSE_DEPENDENCIES",
                        "Dependencies did not become ready within 60 seconds; inspect the partial stack.",
                    ));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        self.refresh_inner()
    }
    pub fn stop(&mut self) -> Result<()> {
        self.checked()?;
        for c in self.containers.clone() {
            if matches!(c.state.as_str(), "missing" | "exited" | "dead" | "created") {
                continue;
            }
            self.verify_engine()?;
            let (current, _) = self.inspect(&c.id)?;
            if current.service != c.service {
                return Err(Error::new(
                    "CONTAINER_IDENTITY",
                    "Service identity changed.",
                ));
            }
            self.run(&["container", "stop", "--timeout", "5", &c.id])?;
        }
        self.refresh_inner()?;
        if self
            .containers
            .iter()
            .any(|c| !matches!(c.state.as_str(), "missing" | "exited" | "dead" | "created"))
        {
            return Err(Error::new(
                "STOP_TIMEOUT",
                "Docker did not confirm container shutdown.",
            ));
        }
        Ok(())
    }
    pub fn logs(&mut self, tail: usize) -> Result<super::LogsResult> {
        validation::tail(tail)?;
        self.checked()?;
        let mut text = String::new();
        let mut truncated = false;
        for c in &self.containers {
            if c.state == "missing" {
                continue;
            }
            // Docker's log streams use stdout and stderr; merge them via CLI's
            // inspect-independent log formatter below in command_runner.
            let command = self.command(&["container", "logs", "--tail", &tail.to_string(), &c.id]);
            let bytes = command_runner::logs(command, TIMEOUT)?;
            let line = format!(
                "[{} {}]\n{}",
                c.service,
                &c.id[..12],
                String::from_utf8_lossy(&bytes)
            );
            let remaining = command_runner::OUTPUT_LIMIT.saturating_sub(text.len());
            if line.len() > remaining {
                truncated = true;
                let mut end = remaining.min(line.len());
                while !line.is_char_boundary(end) {
                    end -= 1;
                }
                text.push_str(&line[..end]);
                break;
            }
            text.push_str(&line);
        }
        Ok(super::LogsResult {
            id: self.id.clone(),
            text,
            truncated,
        })
    }
}
pub fn docker(cwd: &str) -> Result<String> {
    paths::path_string(&security_policy::executable(
        "docker",
        Path::new(cwd),
        true,
    )?)
}
