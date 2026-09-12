pub mod error;
mod launch;
pub mod limits;
pub mod logs;
pub mod paths;
mod port_detection;
pub mod proc;
mod process_manager;
pub mod registry;
mod security_policy;
pub mod types;
pub mod validation;

pub use error::{Error, Result};
pub use paths::Paths;
use registry::{Registry, Transaction};
use std::{path::Path, time::Duration};
pub use types::*;

#[derive(Clone)]
pub struct AgentRun {
    paths: Paths,
    grace: Duration,
}

impl AgentRun {
    pub fn from_environment() -> Result<Self> {
        Ok(Self::new(Paths::from_environment()?))
    }
    pub fn new(paths: Paths) -> Self {
        Self {
            paths,
            grace: Duration::from_millis(1500),
        }
    }
    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    fn transaction<'a>(&self, registry: &'a Registry) -> Result<Transaction<'a>> {
        let mut tx = registry.transaction()?;
        for p in &mut tx.data.processes {
            if proc::matches(p) {
                p.status = Status::Running;
                p.dead_reason = None;
                p.ports = port_detection::detect(p);
            } else {
                // Revalidation observes whether the recorded process is still
                // alive; it cannot explain an already recorded death better
                // than an explicit stop/restart outcome. Keep that history.
                if p.status != Status::Dead || p.dead_reason.is_none() {
                    p.dead_reason =
                        Some("Process absent, inaccessible, or kernel identity changed".into());
                }
                p.status = Status::Dead;
                p.ports.clear();
            }
        }
        tx.save()?;
        logs::prune(&self.paths, &tx.data.processes)?;
        Ok(tx)
    }
    pub fn list(&self) -> Result<Vec<ManagedProcess>> {
        let registry = Registry::new(self.paths.clone());
        let mut tx = self.transaction(&registry)?;
        tx.data.processes.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(tx.data.processes.clone())
    }
    pub fn get(&self, id: &str) -> Result<ManagedProcess> {
        validation::id(id)?;
        self.list()?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| not_found(id))
    }
    pub fn start(&self, request: StartRequest) -> Result<ManagedProcess> {
        let registry = Registry::new(self.paths.clone());
        self.start_inner(request, &mut self.transaction(&registry)?)
    }
    pub fn start_profile(&self, request: ProfileRequest, client: &str) -> Result<ManagedProcess> {
        // Resolve policy inside the same Core transaction as the launch.
        let registry = Registry::new(self.paths.clone());
        let mut tx = self.transaction(&registry)?;
        let profile = request.profile.clone();
        let request = security_policy::resolve(&self.paths.config, request, client)?;
        self.start_record(request, Some(profile), &mut tx)
    }
    fn start_inner(
        &self,
        request: StartRequest,
        tx: &mut Transaction<'_>,
    ) -> Result<ManagedProcess> {
        self.start_record(request, None, tx)
    }
    fn start_record(
        &self,
        request: StartRequest,
        profile: Option<String>,
        tx: &mut Transaction<'_>,
    ) -> Result<ManagedProcess> {
        validation::id(&request.id)?;
        validation::command(&request.command)?;
        validation::owner(&request.owner)?;
        if tx.data.processes.iter().any(|p| p.id == request.id) {
            return Err(Error::new(
                "ID_EXISTS",
                format!("ID already registered: {}. Clean before reuse.", request.id),
            ));
        }
        if tx.data.processes.len() >= limits::MAX_PROCESSES {
            return Err(Error::new(
                "PROCESS_LIMIT",
                "Registry is full (64 entries). Stop/clean unused entries.",
            ));
        }
        let cwd = match &request.cwd {
            Some(cwd) => paths::expand_home(cwd)?,
            None => std::env::current_dir()?,
        };
        let cwd = paths::canonical_directory(&cwd)?;
        let executable = security_policy::executable(&request.command[0], &cwd, false)?;
        let (log_path, log) = logs::capture(&self.paths, &request.id)?;
        let mut child = launch::PendingChild::spawn(&request, &cwd, &executable, log_path, &log)?;
        child.record.profile = profile;
        tx.data.processes.push(child.record.clone());
        // If this fails, Drop closes the launch gate: no command was executed.
        if let Err(e) = tx.save() {
            tx.data.processes.retain(|p| p.id != request.id);
            return Err(e);
        }
        if let Err(e) = child.commit() {
            tx.data.processes.retain(|p| p.id != request.id);
            tx.save()?;
            return Err(e);
        }
        Ok(child.record.clone())
    }
    pub fn restart(&self, id: &str) -> Result<ManagedProcess> {
        self.restart_inner(id, false)
    }
    pub fn restart_profile(&self, id: &str) -> Result<ManagedProcess> {
        self.restart_inner(id, true)
    }
    fn restart_inner(&self, id: &str, restricted: bool) -> Result<ManagedProcess> {
        validation::id(id)?;
        let registry = Registry::new(self.paths.clone());
        let mut tx = self.transaction(&registry)?;
        let index = tx
            .data
            .processes
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| not_found(id))?;
        let old = tx.data.processes[index].clone();
        // Recheck the current policy BEFORE stopping anything. A CLI command
        // cannot acquire MCP execution privileges through restart.
        let request = if restricted {
            let profile = old.profile.clone().ok_or_else(|| {
                Error::new(
                    "PROFILE_REQUIRED",
                    "MCP restart requires an entry started with an authorized profile.",
                )
            })?;
            let mut request = security_policy::resolve(
                &self.paths.config,
                ProfileRequest {
                    id: old.id.clone(),
                    cwd: old.cwd.clone(),
                    profile,
                },
                old.owner.client.as_deref().unwrap_or("mcp"),
            )?;
            request.owner = old.owner.clone();
            request
        } else {
            StartRequest {
                id: old.id.clone(),
                cwd: Some(old.cwd.clone()),
                command: old.command.clone(),
                owner: old.owner.clone(),
            }
        };
        let cwd = paths::canonical_directory(Path::new(request.cwd.as_deref().unwrap()))?;
        security_policy::executable(&request.command[0], &cwd, restricted)?;
        if old.status == Status::Running {
            process_manager::stop(&old, self.grace)?;
        } else if proc::inspect(old.pid).is_some_and(|p| p.alive())
            || !proc::group(old.pgid).is_empty()
        {
            return Err(Error::new(
                "IDENTITY_MISMATCH",
                "Refusing restart while the old PID/group exists with an unverifiable identity.",
            ));
        }
        tx.data.processes[index].status = Status::Dead;
        tx.data.processes[index].ports.clear();
        tx.data.processes[index].dead_reason = Some("Stopped for restart".into());
        tx.save()?;
        // The old dead record stays on disk until the replacement is ready.
        // Restore it on launch failure so status, logs and retry remain available.
        let previous = tx.data.processes.remove(index);
        let profile = old.profile;
        match self.start_record(request, profile, &mut tx) {
            Ok(p) => Ok(p),
            Err(error) => {
                if !tx.data.processes.iter().any(|p| p.id == id) {
                    tx.data.processes.push(previous);
                    tx.save()?;
                }
                Err(error)
            }
        }
    }
    pub fn stop(&self, id: &str) -> Result<ManagedProcess> {
        validation::id(id)?;
        let registry = Registry::new(self.paths.clone());
        let mut tx = self.transaction(&registry)?;
        let p = tx
            .data
            .processes
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| not_found(id))?;
        if p.status == Status::Running {
            process_manager::stop(p, self.grace)?;
            p.status = Status::Dead;
            p.ports.clear();
            p.dead_reason = Some("Stopped".into());
        }
        let result = p.clone();
        tx.save()?;
        Ok(result)
    }
    pub fn stop_all(&self) -> Result<StopAllResult> {
        let registry = Registry::new(self.paths.clone());
        let mut tx = self.transaction(&registry)?;
        let mut result = StopAllResult {
            stopped: vec![],
            errors: vec![],
        };
        for i in 0..tx.data.processes.len() {
            let p = &mut tx.data.processes[i];
            if p.status != Status::Running {
                continue;
            }
            match process_manager::stop(p, self.grace) {
                Ok(()) => {
                    p.status = Status::Dead;
                    p.ports.clear();
                    p.dead_reason = Some("Stopped".into());
                    result.stopped.push(p.id.clone());
                    tx.save()?;
                }
                Err(e) => result.errors.push(StopFailure {
                    id: p.id.clone(),
                    code: e.code,
                    message: e.message,
                }),
            }
        }
        Ok(result)
    }
    pub fn clean(&self) -> Result<CleanResult> {
        let registry = Registry::new(self.paths.clone());
        let mut tx = self.transaction(&registry)?;
        let removed = tx
            .data
            .processes
            .iter()
            .filter(|p| p.status == Status::Dead)
            .map(|p| p.id.clone())
            .collect();
        tx.data.processes.retain(|p| p.status == Status::Running);
        tx.save()?;
        Ok(CleanResult { removed })
    }
    pub fn get_logs(&self, id: &str, tail: usize) -> Result<LogsResult> {
        validation::id(id)?;
        validation::tail(tail)?;
        let registry = Registry::new(self.paths.clone());
        let tx = self.transaction(&registry)?;
        let p = tx
            .data
            .processes
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| not_found(id))?;
        logs::read(&self.paths, p, tail)
    }
    pub fn ports(&self) -> Result<Vec<PortsResult>> {
        Ok(self
            .list()?
            .into_iter()
            .map(|p| PortsResult {
                id: p.id,
                status: p.status,
                ports: p.ports,
            })
            .collect())
    }
    pub fn config_path(&self) -> &Path {
        &self.paths.config
    }
}
fn not_found(id: &str) -> Error {
    Error::new("NOT_FOUND", format!("No registered process: {id}"))
}
