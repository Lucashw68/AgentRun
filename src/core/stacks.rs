use super::{
    AgentRun, Error, LogsResult, Owner, OwnerType, ProfileRequest, Result,
    compose::{self, ManagedStack, StackStatus},
    registry::Registry,
    security_policy, validation,
};

impl AgentRun {
    pub fn list_stacks(&self) -> Result<Vec<ManagedStack>> {
        let registry = Registry::new(self.paths.clone());
        let mut tx = self.transaction(&registry)?;
        for stack in &mut tx.data.stacks {
            stack.refresh();
        }
        tx.save()?;
        tx.data.stacks.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(tx.data.stacks.clone())
    }
    pub fn get_stack(&self, id: &str) -> Result<ManagedStack> {
        validation::id(id)?;
        self.list_stacks()?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| Error::new("NOT_FOUND", "No registered stack with this ID."))
    }
    pub fn start_stack(&self, input: ProfileRequest, client: Option<&str>) -> Result<ManagedStack> {
        let registry = Registry::new(self.paths.clone());
        let mut tx = self.transaction(&registry)?;
        let (recipe, cwd) = security_policy::resolve_compose(
            &self.paths.config,
            &input,
            client.unwrap_or("manual"),
        )?;
        if tx.data.processes.iter().any(|p| p.id == input.id)
            || tx.data.stacks.iter().any(|s| s.id == input.id)
        {
            return Err(Error::new(
                "ID_EXISTS",
                "ID already registered; use stack restart for existing containers.",
            ));
        }
        if tx.data.stacks.len() >= 16 {
            return Err(Error::new(
                "STACK_LIMIT",
                "At most 16 stacks may be registered.",
            ));
        }
        let mut stack = ManagedStack {
            id: input.id,
            docker: compose::docker(&cwd)?,
            cwd,
            profile: input.profile,
            owner: Owner {
                kind: if client.is_some() {
                    OwnerType::Agent
                } else {
                    OwnerType::Manual
                },
                client: client.map(str::to_owned),
            },
            project: format!("agentrun-{}", uuid::Uuid::new_v4()),
            engine_id: String::new(),
            recipe,
            started_at: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|e| Error::new("CLOCK_ERROR", e.to_string()))?,
            containers: vec![],
            status: StackStatus::Creating,
            reason: None,
        };
        stack.engine_id = stack.engine()?;
        stack.prepare()?;
        // Journal intent before Docker creates anything. A crash before the ID
        // commit leaves an unknown record, never automatically adopted resources.
        tx.data.stacks.push(stack.clone());
        tx.save()?;
        let index = tx.data.stacks.len() - 1;
        let create = stack.create();
        if let Err(e) = &create {
            stack.reason = Some(e.code.clone());
            stack.status = StackStatus::Unknown;
        }
        tx.data.stacks[index] = stack.clone();
        tx.save()?; // Full IDs durable before any container start.
        create?;
        let start = stack.start();
        if let Err(e) = &start {
            stack.reason = Some(e.code.clone());
            stack.status = StackStatus::Unknown;
        }
        tx.data.stacks[index] = stack.clone();
        tx.save()?;
        start?;
        Ok(stack)
    }
    pub fn stop_stack(&self, id: &str) -> Result<ManagedStack> {
        self.stack_mutation(id, false)
    }
    pub fn restart_stack(&self, id: &str) -> Result<ManagedStack> {
        self.stack_mutation(id, true)
    }
    fn stack_mutation(&self, id: &str, restart: bool) -> Result<ManagedStack> {
        validation::id(id)?;
        let registry = Registry::new(self.paths.clone());
        let mut tx = self.transaction(&registry)?;
        let stack = tx
            .data
            .stacks
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| Error::new("NOT_FOUND", "No registered stack with this ID."))?;
        if restart {
            let (recipe, cwd) = security_policy::resolve_compose(
                &self.paths.config,
                &ProfileRequest {
                    id: stack.id.clone(),
                    cwd: stack.cwd.clone(),
                    profile: stack.profile.clone(),
                },
                stack.owner.client.as_deref().unwrap_or("manual"),
            )?;
            if recipe != stack.recipe || cwd != stack.cwd {
                return Err(Error::new(
                    "RECIPE_CHANGED",
                    "Restart reuses container IDs; changed recipes require a separate stack.",
                ));
            }
        }
        let result = stack
            .stop()
            .and_then(|()| if restart { stack.start() } else { Ok(()) });
        if let Err(e) = &result {
            stack.status = StackStatus::Unknown;
            stack.reason = Some(e.code.clone());
        }
        let stack = stack.clone();
        tx.save()?;
        result?;
        Ok(stack)
    }
    pub fn stack_logs(&self, id: &str, tail: usize) -> Result<LogsResult> {
        self.get_stack(id)?.logs(tail)
    }
}
