mod support;
use agentrun::core::{
    AgentRun, Owner, OwnerType, ProfileRequest, Status, proc, registry::Registry, validation,
};
use serde_json::json;
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

#[test]
fn reads_legacy_node_registry_without_schema_migration() {
    let env = Env::new();
    env.core.list().unwrap();
    // Synthetic legacy data: never copy identifiers from a developer's machine.
    let legacy = json!({"version":1,"processes":[{
        "id":"legacy-node", "pid":2147483000, "pgid":2147483000,
        "cwd":env.cwd, "command":["pnpm","dev"],
        "startedAt":"2024-01-01T00:00:00.000Z", "status":"running", "ports":[3000],
        "owner":{"type":"agent","client":"test-agent"}, "processStartTime":"12345",
        "bootId":"00000000-0000-4000-8000-000000000001", "uid":1000,
        "logPath":env.paths.logs.join("legacy-node-00000000-0000-4000-8000-000000000000.log")
    }]});
    fs::write(&env.paths.registry, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let p = env.core.get("legacy-node").unwrap();
    assert_eq!(p.status, Status::Dead);
    assert_eq!(p.command, vec!["pnpm", "dev"]);
    assert_eq!(env.core.clean().unwrap().removed, vec!["legacy-node"]);
}
use support::*;

#[test]
fn initial_catalog_allows_a_profile_without_installing_other_runtimes() {
    let env = Env::new();
    let mut config: serde_json::Value =
        serde_json::from_str(include_str!("../examples/config.json")).unwrap();
    assert_eq!(config["allowedRoots"], json!(["~/.codex/worktrees"]));
    config["allowedRoots"] = json!([env.cwd]);
    // Keep the shipped catalog: all entries must pass the real policy parser.
    // Only the selected runtime is resolved, even with unavailable tools present.
    config["profiles"]["uninstalled-runtime"] =
        json!({"command":["agentrun-test-runtime-that-does-not-exist"]});
    config["profiles"]["integration-worker"] = json!({"command":[fixture(), "basic"]});
    fs::write(&env.paths.config, serde_json::to_vec(&config).unwrap()).unwrap();
    let request = |profile: &str| ProfileRequest {
        id: "catalog-worker".into(),
        cwd: env.cwd.to_str().unwrap().into(),
        profile: profile.into(),
    };
    assert_eq!(
        env.core
            .start_profile(request("uninstalled-runtime"), "test-agent")
            .unwrap_err()
            .code,
        "EXECUTABLE_NOT_FOUND"
    );
    let process = env
        .core
        .start_profile(request("integration-worker"), "test-agent")
        .unwrap();
    ready(&process, "READY=");
    assert_eq!(process.profile.as_deref(), Some("integration-worker"));
    env.core.stop(&process.id).unwrap();
    assert!(!alive(process.pid));
}

#[test]
fn start_persistence_owner_logs_and_stop() {
    let env = Env::new();
    let mut request = env.request("worker", "basic");
    request.owner = Owner {
        kind: OwnerType::Agent,
        client: Some("codex".into()),
    };
    let p = env.core.start(request).unwrap();
    ready(&p, "STDERR ready");
    assert_eq!(p.pid, p.pgid);
    assert_eq!(proc::inspect(p.pid).unwrap().session, p.pgid);
    let reopened = AgentRun::new(env.paths.clone());
    assert_eq!(reopened.list().unwrap()[0].pid, p.pid);
    assert_eq!(
        reopened.get("worker").unwrap().owner.client.as_deref(),
        Some("codex")
    );
    assert!(
        reopened
            .get_logs("worker", 100)
            .unwrap()
            .text
            .contains("READY=")
    );
    assert_eq!(
        reopened.get_logs("worker", 1).unwrap().text.trim(),
        "STDERR ready"
    );
    assert_eq!(reopened.get_logs("worker", 0).unwrap().text, "");
    assert_eq!(
        fs::metadata(&env.paths.registry).unwrap().mode() & 0o777,
        0o600
    );
    assert_eq!(fs::metadata(&env.paths.logs).unwrap().mode() & 0o777, 0o700);
    assert_eq!(reopened.stop("worker").unwrap().status, Status::Dead);
    assert!(!alive(p.pid));
}

#[test]
fn registry_is_durable_before_command_executes() {
    let env = Env::new();
    let mut req = env.request("committed", "verify-registry");
    req.command
        .push(env.paths.registry.to_str().unwrap().into());
    let p = env.core.start(req).unwrap();
    ready(&p, "REGISTERED_BEFORE_EXEC");
}

#[test]
fn group_stop_detects_child_ports_and_preserves_unregistered_processes() {
    let env = Env::new();
    let mut external = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let p = env.start("tree", "tree");
    let output = ready(&p, "PORT=");
    let child = number(&output, "CHILD=");
    let port = number(&output, "PORT=") as u16;
    assert!(env.core.get("tree").unwrap().ports.contains(&port));
    env.core.stop("tree").unwrap();
    assert!(!alive(p.pid));
    assert!(!alive(child));
    assert!(alive(external.id() as i32));
    external.kill().unwrap();
    external.wait().unwrap();
}

#[test]
fn pidfd_escalation_handles_stubborn_group_and_exited_leader() {
    let env = Env::new();
    for mode in ["stubborn-tree", "leader-exits"] {
        let p = env.start(mode, mode);
        until(|| {
            fs::read_to_string(&p.log_path)
                .unwrap()
                .matches("READY=")
                .count()
                == 2
        });
        let child = number(&fs::read_to_string(&p.log_path).unwrap(), "CHILD=");
        env.core.stop(mode).unwrap();
        assert!(!alive(child));
        assert!(proc::group(p.pgid).is_empty());
    }
}

#[test]
fn natural_death_clean_and_collision_free_logs() {
    let env = Env::new();
    let p = env.start("short", "exit");
    let live = env.start("live", "basic");
    until(|| env.core.get("short").unwrap().status == Status::Dead);
    assert_eq!(env.core.clean().unwrap().removed, vec!["short"]);
    assert!(alive(live.pid));
    assert_eq!(env.core.get("short").unwrap_err().code, "NOT_FOUND");
    assert_ne!(env.start("short", "basic").log_path, p.log_path);
}

#[test]
fn simultaneous_processes_and_stop_all() {
    let env = Env::new();
    let records = thread::scope(|scope| {
        let threads: Vec<_> = (0..6)
            .map(|i| {
                let env = &env;
                scope.spawn(move || env.start(&format!("worker-{i}"), "basic"))
            })
            .collect();
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(env.core.list().unwrap().len(), 6);
    let result = env.core.stop_all().unwrap();
    assert_eq!(result.stopped.len(), 6);
    assert!(result.errors.is_empty());
    assert!(records.iter().all(|p| !alive(p.pid)));
}

#[test]
fn ids_commands_duplicates_and_unknown_ids() {
    let env = Env::new();
    for id in [
        "",
        ".",
        "..",
        "../escape",
        "a/b",
        "a\\b",
        "\0",
        "-option",
        " space",
        &"a".repeat(81),
    ] {
        assert_eq!(
            env.core.start(env.request(id, "basic")).unwrap_err().code,
            "VALIDATION_ERROR"
        );
    }
    assert!(validation::command(&[]).is_err());
    assert!(validation::command(&["".into()]).is_err());
    assert!(validation::command(&["a\0b".into()]).is_err());
    assert_eq!(env.core.stop("unknown").unwrap_err().code, "NOT_FOUND");
    env.start("same", "basic");
    assert_eq!(
        env.core
            .start(env.request("same", "basic"))
            .unwrap_err()
            .code,
        "ID_EXISTS"
    );
    let mut bad = env.request("missing", "basic");
    bad.command = vec!["/nonexistent-agentrun-executable".into()];
    assert_eq!(
        env.core.start(bad).unwrap_err().code,
        "EXECUTABLE_NOT_FOUND"
    );
}

#[test]
fn refuses_start_time_boot_uid_and_external_pid_mismatches() {
    let env = Env::new();
    let p = env.start("identity", "basic");
    let original = fs::read(&env.paths.registry).unwrap();
    for patch in [
        json!({"processStartTime":"0"}),
        json!({"bootId":"00000000-0000-4000-8000-000000000000"}),
        json!({"uid":p.uid+1}),
        json!({"pid":std::process::id(),"pgid":std::process::id(),"processStartTime":proc::inspect(std::process::id() as i32).unwrap().start_time}),
    ] {
        let mut altered: serde_json::Value = serde_json::from_slice(&original).unwrap();
        for (key, value) in patch.as_object().unwrap() {
            altered["processes"][0][key] = value.clone();
        }
        fs::write(&env.paths.registry, serde_json::to_vec(&altered).unwrap()).unwrap();
        assert_eq!(env.core.get("identity").unwrap().status, Status::Dead);
        env.core.stop("identity").unwrap();
        assert!(alive(p.pid));
        assert!(alive(std::process::id() as i32));
        assert!(env.core.stop_all().unwrap().stopped.is_empty());
        assert_eq!(env.core.clean().unwrap().removed, vec!["identity"]);
    }
    fs::write(&env.paths.registry, original).unwrap();
}

#[test]
fn atomic_snapshots_and_native_lock() {
    let env = Env::new();
    env.core.list().unwrap();
    let done = Arc::new(AtomicBool::new(false));
    thread::scope(|scope| {
        let done_reader = done.clone();
        let file = env.paths.registry.clone();
        let reader = scope.spawn(move || {
            let mut reads = 0;
            while !done_reader.load(Ordering::Acquire) {
                let json: serde_json::Value =
                    serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
                assert_eq!(json["version"], 1);
                reads += 1;
            }
            reads
        });
        let writers: Vec<_> = (0..12)
            .map(|_| {
                let paths = env.paths.clone();
                scope.spawn(move || {
                    let r = Registry::new(paths);
                    r.transaction().unwrap().save().unwrap();
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        done.store(true, Ordering::Release);
        assert!(reader.join().unwrap() > 0);
    });
    assert!(
        !fs::read_dir(&env.paths.state).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp"))
    );
}

#[test]
fn corrupt_registry_and_unsafe_paths_fail_closed() {
    let env = Env::new();
    env.core.list().unwrap();
    fs::write(&env.paths.registry, "{broken").unwrap();
    assert_eq!(env.core.list().unwrap_err().code, "REGISTRY_INVALID");
    assert_eq!(fs::read_to_string(&env.paths.registry).unwrap(), "{broken");
    fs::write(&env.paths.registry, r#"{"version":1,"processes":[]}"#).unwrap();
    fs::set_permissions(&env.paths.state, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(env.core.list().unwrap_err().code, "UNSAFE_STATE");
    fs::set_permissions(&env.paths.state, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn logs_are_bounded_and_symlinks_are_refused() {
    let env = Env::new();
    let p = env.start("logs", "exit");
    until(|| !alive(p.pid));
    fs::write(&p.log_path, "x".repeat(2 * 1024 * 1024)).unwrap();
    let logs = env.core.get_logs("logs", 100).unwrap();
    assert!(logs.truncated);
    assert!(logs.text.len() <= 1024 * 1024);
    fs::remove_file(&p.log_path).unwrap();
    symlink("/etc/passwd", &p.log_path).unwrap();
    assert!(env.core.get_logs("logs", 100).is_err());
}

#[test]
fn failed_exec_does_not_leave_a_running_registry_entry() {
    let env = Env::new();
    let file = env.cwd.join("not-executable-format");
    fs::write(&file, "plain text without shebang").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o700)).unwrap();
    let mut request = env.request("bad-format", "basic");
    request.command = vec![file.to_str().unwrap().into()];
    assert_eq!(env.core.start(request).unwrap_err().code, "EXEC_FAILED");
    assert!(env.core.list().unwrap().is_empty());
}

#[test]
fn mcp_policy_roots_profiles_and_shell_aliases() {
    let env = Env::new();
    let alias = env.cwd.join("alias");
    symlink("/bin/sh", &alias).unwrap();
    env.config(json!({"worker":{"command":[fixture()]},"shell":{"command":["bash","-c","true"]},"alias":{"command":[alias,"-c","true"]},"sudo":{"command":["sudo","true"]}}));
    let request = |id: &str, cwd: &std::path::Path, profile: &str| ProfileRequest {
        id: id.into(),
        cwd: cwd.to_str().unwrap().into(),
        profile: profile.into(),
    };
    for (cwd, profile, code) in [
        (env.root.path(), "worker", "CWD_NOT_ALLOWED"),
        (&env.cwd, "unknown", "UNKNOWN_PROFILE"),
        (&env.cwd, "shell", "FORBIDDEN_PROFILE"),
        (&env.cwd, "alias", "FORBIDDEN_PROFILE"),
        (&env.cwd, "sudo", "FORBIDDEN_PROFILE"),
    ] {
        assert_eq!(
            env.core
                .start_profile(request("denied", cwd, profile), "codex")
                .unwrap_err()
                .code,
            code
        );
    }
    let escape = env.cwd.join("escape");
    symlink(env.root.path(), &escape).unwrap();
    assert_eq!(
        env.core
            .start_profile(request("escape", &escape, "worker"), "codex")
            .unwrap_err()
            .code,
        "CWD_NOT_ALLOWED"
    );
    let sibling = env.root.path().join("projects-evil");
    fs::create_dir(&sibling).unwrap();
    assert_eq!(
        env.core
            .start_profile(request("prefix", &sibling, "worker"), "codex")
            .unwrap_err()
            .code,
        "CWD_NOT_ALLOWED"
    );
    let p = env
        .core
        .start_profile(request("allowed", &env.cwd, "worker"), "codex")
        .unwrap();
    ready(&p, "READY=");
    assert_eq!(p.owner.client.as_deref(), Some("codex"));
}

#[test]
fn restart_replaces_the_whole_group_and_preserves_recipe() {
    let env = Env::new();
    let old = env.start("restart", "stubborn-tree");
    let child = number(&ready(&old, "CHILD="), "CHILD=");
    let new = env.core.restart(&old.id).unwrap();
    ready(&new, "READY=");
    assert_ne!(old.pid, new.pid);
    assert!(!alive(old.pid));
    assert!(!alive(child));
    assert_eq!(old.cwd, new.cwd);
    assert_eq!(old.command, new.command);
    assert_eq!(old.owner, new.owner);
    assert_ne!(old.log_path, new.log_path);
    assert!(std::path::Path::new(&old.log_path).exists());
    assert_eq!(env.core.list().unwrap().len(), 1);
    env.core.stop(&new.id).unwrap();
    let revived = env.core.restart(&new.id).unwrap();
    ready(&revived, "READY=");
}

#[test]
fn restart_refuses_mismatched_identity_and_preserves_failed_recipe() {
    let env = Env::new();
    let old = env.start("restart", "basic");
    let original = fs::read(&env.paths.registry).unwrap();
    let mut data: serde_json::Value = serde_json::from_slice(&original).unwrap();
    data["processes"][0]["processStartTime"] = json!("0");
    fs::write(&env.paths.registry, serde_json::to_vec(&data).unwrap()).unwrap();
    assert_eq!(
        env.core.restart(&old.id).unwrap_err().code,
        "IDENTITY_MISMATCH"
    );
    assert!(alive(old.pid));
    fs::write(&env.paths.registry, original).unwrap();
    env.core.stop(&old.id).unwrap();
    let bad = env.cwd.join("bad-exec");
    fs::write(&bad, "no executable format").unwrap();
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o700)).unwrap();
    let registry = Registry::new(env.paths.clone());
    {
        let mut tx = registry.transaction().unwrap();
        tx.data.processes[0].command = vec![bad.to_str().unwrap().into()];
        tx.save().unwrap();
    }
    assert_eq!(env.core.restart(&old.id).unwrap_err().code, "EXEC_FAILED");
    let dead = env.core.get(&old.id).unwrap();
    assert_eq!(dead.status, Status::Dead);
    assert_eq!(dead.log_path, old.log_path);
    assert_eq!(dead.command, vec![bad.to_str().unwrap()]);
}

#[test]
fn mcp_restart_revalidates_policy_before_stopping() {
    let env = Env::new();
    env.start("manual", "basic");
    assert_eq!(
        env.core.restart_profile("manual").unwrap_err().code,
        "PROFILE_REQUIRED"
    );
    env.config(json!({"worker":{"command":[fixture(), "basic"]}}));
    let old = env
        .core
        .start_profile(
            ProfileRequest {
                id: "profile".into(),
                cwd: env.cwd.to_str().unwrap().into(),
                profile: "worker".into(),
            },
            "test-agent",
        )
        .unwrap();
    ready(&old, "READY=");
    env.config(json!({}));
    assert_eq!(
        env.core.restart_profile(&old.id).unwrap_err().code,
        "UNKNOWN_PROFILE"
    );
    assert!(alive(old.pid));
    env.config(json!({"worker":{"command":["sh", "-c", "true"]}}));
    assert_eq!(
        env.core.restart_profile(&old.id).unwrap_err().code,
        "FORBIDDEN_PROFILE"
    );
    assert!(alive(old.pid));
    env.config(json!({"worker":{"command":[fixture(), "literal", "updated-profile"]}}));
    let new = env.core.restart_profile(&old.id).unwrap();
    ready(&new, "updated-profile");
    assert!(!alive(old.pid));
    assert_eq!(new.profile.as_deref(), Some("worker"));
    assert_eq!(new.owner, old.owner);
}

#[test]
fn oversized_registry_policy_and_commands_are_rejected() {
    use agentrun::core::limits::*;
    let env = Env::new();
    env.core.list().unwrap();
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&env.paths.registry)
        .unwrap();
    file.set_len(MAX_REGISTRY_BYTES as u64 + 1).unwrap();
    assert_eq!(env.core.list().unwrap_err().code, "REGISTRY_TOO_LARGE");
    assert_eq!(
        file.metadata().unwrap().len(),
        MAX_REGISTRY_BYTES as u64 + 1
    );
    fs::write(&env.paths.registry, r#"{"version":1,"processes":[]}"#).unwrap();
    fs::write(&env.paths.config, " ".repeat(MAX_CONFIG_BYTES + 1)).unwrap();
    assert_eq!(
        env.core
            .start_profile(
                ProfileRequest {
                    id: "large".into(),
                    cwd: env.cwd.to_str().unwrap().into(),
                    profile: "worker".into()
                },
                "test-agent"
            )
            .unwrap_err()
            .code,
        "CONFIG_TOO_LARGE"
    );
    assert!(validation::command(&["true".into(), "x".repeat(MAX_COMMAND_BYTES)]).is_err());
}

#[test]
fn registry_capacity_denies_launch_but_allows_clean() {
    use agentrun::core::limits::MAX_PROCESSES;
    let env = Env::new();
    let dead = env.start("seed", "exit");
    until(|| !alive(dead.pid));
    let registry = Registry::new(env.paths.clone());
    {
        let mut tx = registry.transaction().unwrap();
        tx.data.processes = (0..MAX_PROCESSES)
            .map(|i| {
                let mut p = dead.clone();
                p.id = format!("dead-{i}");
                p
            })
            .collect();
        tx.save().unwrap();
    }
    assert_eq!(
        env.core
            .start(env.request("overflow", "basic"))
            .unwrap_err()
            .code,
        "PROCESS_LIMIT"
    );
    assert_eq!(env.core.clean().unwrap().removed.len(), MAX_PROCESSES);
    env.start("room", "basic");
}

#[test]
fn logs_rotate_while_cli_is_gone_and_retention_is_bounded() {
    use agentrun::core::limits::{LOG_SEGMENT_BYTES, RETAINED_LOG_RUNS};
    let env = Env::new();
    let value = env.json(&[
        "start",
        "flood",
        "--json",
        "--",
        fixture().to_str().unwrap(),
        "flood",
    ]);
    let p: agentrun::core::ManagedProcess =
        serde_json::from_value(value["process"].clone()).unwrap();
    ready(&p, "FLOOD_DONE");
    let backup = std::path::Path::new(&p.log_path).with_extension("log.1");
    assert!(fs::metadata(&p.log_path).unwrap().len() <= LOG_SEGMENT_BYTES);
    assert!(fs::metadata(&backup).unwrap().len() <= LOG_SEGMENT_BYTES);
    let logs = env.core.get_logs(&p.id, 10).unwrap();
    assert!(logs.text.contains("FLOOD_DONE"));
    assert!(logs.truncated);
    // Simulate old generations, including one still held by a live collector.
    for i in 0..RETAINED_LOG_RUNS + 5 {
        let (_, file) = agentrun::core::logs::create(&env.paths, &format!("old-{i}")).unwrap();
        drop(file);
    }
    env.core.list().unwrap();
    let remaining = fs::read_dir(&env.paths.logs)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|s| s == "log")
        })
        .count();
    assert_eq!(remaining, RETAINED_LOG_RUNS + 1);
    assert!(std::path::Path::new(&p.log_path).exists());
}

#[test]
fn retention_does_not_delete_a_collectors_open_log() {
    use std::os::fd::AsRawFd;
    let env = Env::new();
    env.core.list().unwrap();
    let (path, file) = agentrun::core::logs::create(&env.paths, "leased").unwrap();
    // SAFETY: flock borrows this live test file descriptor.
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(locked, 0);
    for i in 0..25 {
        agentrun::core::logs::create(&env.paths, &format!("new-{i}")).unwrap();
    }
    env.core.list().unwrap();
    assert!(std::path::Path::new(&path).exists());
    drop(file);
    env.core.list().unwrap();
    assert!(!std::path::Path::new(&path).exists());
}
