mod support;
use agentrun::core::{ProfileRequest, compose::StackStatus};
use serde_json::json;
use std::{fs, process::Command};
use support::*;
fn request(env: &Env, id: &str, profile: &str) -> ProfileRequest {
    ProfileRequest {
        id: id.into(),
        cwd: env.cwd.to_string_lossy().into_owned(),
        profile: profile.into(),
    }
}

#[test]
fn make_profile_runs_existing_target_without_rewriting_project() {
    let env = Env::new();
    let makefile = format!("dev:\n\t{} basic\n", fixture().display());
    fs::write(env.cwd.join("Makefile"), &makefile).unwrap();
    env.config(json!({"make-dev":{"make":{"file":"Makefile","target":"dev"}}}));
    let p = env
        .core
        .start_profile(request(&env, "make-worker", "make-dev"), "test")
        .unwrap();
    ready(&p, "READY=");
    assert_eq!(env.core.list().unwrap().len(), 1);
    assert_eq!(
        fs::read_to_string(env.cwd.join("Makefile")).unwrap(),
        makefile
    );
    env.core.stop(&p.id).unwrap();
    assert!(agentrun::core::proc::group(p.pgid).is_empty());
    assert_eq!(
        env.core.get(&p.id).unwrap().dead_reason.as_deref(),
        Some("Stopped")
    );
}

#[test]
fn make_profiles_reject_options_variables_symlinks_and_mixed_kinds() {
    let env = Env::new();
    fs::write(env.cwd.join("Makefile"), "dev:\n\ttrue\n").unwrap();
    for target in ["--eval=x", "SHELL=x", "../dev", "dev\nother"] {
        env.config(json!({"bad":{"make":{"file":"Makefile","target":target}}}));
        assert!(
            env.core
                .start_profile(request(&env, "bad", "bad"), "test")
                .is_err()
        );
    }
    let outside = env.root.path().join("outside");
    fs::write(&outside, "dev:\n\ttrue\n").unwrap();
    std::os::unix::fs::symlink(&outside, env.cwd.join("redirect")).unwrap();
    env.config(json!({"bad":{"make":{"file":"redirect","target":"dev"}}}));
    assert_eq!(
        env.core
            .start_profile(request(&env, "bad", "bad"), "test")
            .unwrap_err()
            .code,
        "FILE_NOT_ALLOWED"
    );
    env.config(json!({"bad":{"command":["make"],"make":{"file":"Makefile","target":"dev"}}}));
    assert_eq!(
        env.core
            .start_profile(request(&env, "bad", "bad"), "test")
            .unwrap_err()
            .code,
        "CONFIG_ERROR"
    );
}

#[test]
fn cli_make_profile_ignores_inherited_make_injection() {
    let env = Env::new();
    fs::write(
        env.cwd.join("Makefile"),
        format!("dev:\n\t{} basic\n", fixture().display()),
    )
    .unwrap();
    let malicious = env.cwd.join("injected.mk");
    fs::write(&malicious, "$(error inherited makefile executed)\n").unwrap();
    env.config(json!({"make-dev":{"make":{"file":"Makefile","target":"dev"}}}));
    let output = env
        .command(CLI)
        .env("MAKEFILES", malicious)
        .env("MAKEFLAGS", "--eval=$(error injected)")
        .args([
            "start",
            "worker",
            "--profile",
            "make-dev",
            "--cwd",
            env.cwd.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let p = env.core.get("worker").unwrap();
    ready(&p, "READY=");
    assert_eq!(p.owner.kind, agentrun::core::OwnerType::Manual);
}

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker").args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().into()
}
struct DockerCleanup {
    containers: Vec<String>,
    project: Option<String>,
}
impl Drop for DockerCleanup {
    fn drop(&mut self) {
        for id in &self.containers {
            let _ = Command::new("docker").args(["rm", "-f", id]).output();
        }
        if let Some(project) = &self.project {
            for kind in ["network", "volume"] {
                let out = Command::new("docker")
                    .args([
                        kind,
                        "ls",
                        "--filter",
                        &format!("label=com.docker.compose.project={project}"),
                        "--format",
                        "{{.Name}}",
                    ])
                    .output()
                    .unwrap();
                for name in String::from_utf8_lossy(&out.stdout).lines() {
                    let _ = Command::new("docker").args([kind, "rm", name]).output();
                }
            }
        }
    }
}

/// Run explicitly in CI/on Linux with a local Docker engine and alpine:3.23.
#[test]
#[ignore = "requires a local Docker engine and alpine:3.23; CI runs this explicitly"]
fn real_compose_lifecycle_identity_persistence_and_volume_protection() {
    let env = Env::new();
    let compose = "services:\n  first:\n    image: alpine:3.23\n    command: [sh, -c, 'echo FIRST; echo STDERR >&2; exec sleep 600']\n    volumes: ['data:/data']\n    healthcheck:\n      test: [CMD, 'true']\n      interval: 1s\n      timeout: 1s\n      retries: 5\n  second:\n    image: alpine:3.23\n    command: [busybox, nc, -l, -p, '8080']\n    ports: ['127.0.0.1::8080']\n    depends_on:\n      first:\n        condition: service_healthy\nvolumes:\n  data: {}\n";
    fs::write(env.cwd.join("compose.yml"), compose).unwrap();
    fs::write(
        env.cwd.join("Makefile"),
        "prepare:\n\t@printf prepared > prepared.txt\n\t@printf GENERATED=yes > generated.env\n",
    )
    .unwrap();
    env.config(json!({"stack":{"compose":{"files":["compose.yml"],"envFiles":["generated.env"],"prepareMake":{"file":"Makefile","target":"prepare"}}}}));
    let mut cleanup = DockerCleanup {
        containers: vec![],
        project: None,
    };
    let result = env
        .core
        .start_stack(request(&env, "stack", "stack"), Some("test"));
    if let Ok(recorded) = env.core.get_stack("stack") {
        cleanup.containers = recorded.containers.iter().map(|c| c.id.clone()).collect();
        cleanup.project = Some(recorded.project);
    }
    let stack = result.unwrap();
    assert_eq!(stack.status, StackStatus::Running);
    assert_eq!(stack.containers.len(), 2);
    assert!(stack.containers.iter().any(|c| !c.ports.is_empty()));
    assert_eq!(
        fs::read_to_string(env.cwd.join("compose.yml")).unwrap(),
        compose
    );
    assert_eq!(
        fs::read_to_string(env.cwd.join("prepared.txt")).unwrap(),
        "prepared"
    );
    let fresh = agentrun::core::AgentRun::new(env.paths.clone());
    assert_eq!(
        fresh.get_stack("stack").unwrap().containers[0].id,
        stack.containers[0].id
    );
    let logs = fresh.stack_logs("stack", 100).unwrap().text;
    assert!(logs.contains("FIRST") && logs.contains("STDERR"), "{logs}");
    assert_eq!(env.json(&["list", "--json"])["stacks"][0]["id"], "stack");
    assert_eq!(
        env.json(&["stack", "status", "stack", "--json"])["stack"]["status"],
        "running"
    );
    let volumes = docker(&[
        "volume",
        "ls",
        "--filter",
        &format!("label=com.docker.compose.project={}", stack.project),
        "--format",
        "{{.Name}}",
    ]);
    assert!(!volumes.is_empty());
    assert_eq!(
        fresh.stop_stack("stack").unwrap().status,
        StackStatus::Stopped
    );
    for name in volumes.lines() {
        docker(&["volume", "inspect", name]);
    }
    assert!(fresh.clean().unwrap().removed.is_empty()); // preserve stopped containers and restart metadata
    assert_eq!(
        fresh.restart_stack("stack").unwrap().status,
        StackStatus::Running
    );
    // Revocation is checked BEFORE stopping containers.
    env.config(json!({}));
    assert_eq!(
        fresh.restart_stack("stack").unwrap_err().code,
        "UNKNOWN_PROFILE"
    );
    assert_eq!(
        fresh.get_stack("stack").unwrap().status,
        StackStatus::Running
    );
    assert_eq!(fresh.stop_all().unwrap().stopped, ["stack"]);
    // A foreign container copying the project/service labels is never adopted or stopped.
    let foreign = docker(&[
        "run",
        "-d",
        "--label",
        &format!("com.docker.compose.project={}", stack.project),
        "--label",
        "com.docker.compose.service=foreign",
        "alpine:3.23",
        "sleep",
        "600",
    ]);
    cleanup.containers.push(foreign.clone());
    assert_eq!(
        fresh.get_stack("stack").unwrap().status,
        StackStatus::Unknown
    );
    assert_eq!(
        fresh.stop_stack("stack").unwrap_err().code,
        "UNREGISTERED_CONTAINERS"
    );
    assert_eq!(
        docker(&["inspect", "--format", "{{.State.Running}}", &foreign]),
        "true"
    );
    docker(&["rm", "-f", &foreign]);
    // Engine identity mismatch never becomes stopped/dead and clean preserves it.
    let original = fs::read(&env.paths.registry).unwrap();
    let mut registry: serde_json::Value = serde_json::from_slice(&original).unwrap();
    registry["stacks"][0]["engineId"] = json!("another-engine");
    fs::write(&env.paths.registry, serde_json::to_vec(&registry).unwrap()).unwrap();
    assert_eq!(
        fresh.get_stack("stack").unwrap().status,
        StackStatus::Unknown
    );
    assert_eq!(
        fresh.stop_stack("stack").unwrap_err().code,
        "ENGINE_IDENTITY"
    );
    assert!(fresh.clean().unwrap().removed.is_empty());
    registry["stacks"][0]["engineId"] = json!(stack.engine_id);
    registry["stacks"][0]["recipe"]["socket"] = json!(env.cwd.join("absent.sock"));
    fs::write(&env.paths.registry, serde_json::to_vec(&registry).unwrap()).unwrap();
    assert_eq!(
        fresh.get_stack("stack").unwrap().status,
        StackStatus::Unknown
    );
    assert!(fresh.stop_stack("stack").is_err());
    assert!(fresh.clean().unwrap().removed.is_empty());
    registry = serde_json::from_slice(&original).unwrap();
    registry["stacks"][0]["containers"] = json!([]);
    registry["stacks"][0]["status"] = json!("creating");
    fs::write(&env.paths.registry, serde_json::to_vec(&registry).unwrap()).unwrap();
    assert_eq!(
        fresh.get_stack("stack").unwrap().reason.as_deref(),
        Some("UNREGISTERED_CONTAINERS")
    );
    assert!(fresh.stop_stack("stack").is_err());
    assert!(fresh.clean().unwrap().removed.is_empty());
    fs::write(&env.paths.registry, original).unwrap();
    for id in &stack.containers {
        docker(&["rm", &id.id]);
    }
    assert_eq!(
        fresh.get_stack("stack").unwrap().status,
        StackStatus::Missing
    );
    assert_eq!(fresh.clean().unwrap().removed, ["stack"]);
}

#[test]
#[ignore = "requires Docker and alpine:3.23; CI runs this explicitly"]
fn compose_failed_creation_is_journalled_and_fixed_names_are_refused() {
    let env = Env::new();
    fs::write(env.cwd.join("compose.yml"), "services:\n  web:\n    image: alpine:3.23\n    container_name: agentrun-forbidden-fixed-name\n").unwrap();
    env.config(json!({"compose":{"compose":{"files":["compose.yml"]}}}));
    assert_eq!(
        env.core
            .start_stack(request(&env, "fixed", "compose"), None)
            .unwrap_err()
            .code,
        "UNSUPPORTED_COMPOSE"
    );
    assert!(env.core.list_stacks().unwrap().is_empty());
    fs::write(
        env.cwd.join("compose.yml"),
        "services:\n  web:\n    image: agentrun-test-image-that-does-not-exist:never\n",
    )
    .unwrap();
    assert_eq!(
        env.core
            .start_stack(request(&env, "failed", "compose"), None)
            .unwrap_err()
            .code,
        "BACKEND_FAILED"
    );
    let stack = env.core.get_stack("failed").unwrap();
    assert_eq!(stack.status, StackStatus::Unknown);
    assert!(stack.containers.is_empty());
    assert!(env.core.clean().unwrap().removed.is_empty());
    let _cleanup = DockerCleanup {
        containers: vec![],
        project: Some(stack.project),
    };
    // A failed pre-ID journal is deliberately not auto-forgotten by the Core.
    // Remove only this test's private empty journal after verifying Docker above.
    let registry = agentrun::core::registry::Registry::new(env.paths.clone());
    let mut tx = registry.transaction().unwrap();
    tx.data.stacks.clear();
    tx.save().unwrap();
}

#[test]
#[ignore = "requires Docker and alpine:3.23; CI runs this explicitly"]
fn compose_explicit_build_uses_existing_dockerfile() {
    let env = Env::new();
    let image = format!("agentrun-test-{}", uuid::Uuid::new_v4());
    fs::write(
        env.cwd.join("Dockerfile"),
        "FROM alpine:3.23\nCMD [\"sleep\", \"600\"]\n",
    )
    .unwrap();
    fs::write(
        env.cwd.join("compose.yml"),
        format!("services:\n  worker:\n    build: .\n    image: {image}\n"),
    )
    .unwrap();
    env.config(json!({"compose":{"compose":{"files":["compose.yml"],"build":true}}}));
    let stack = env
        .core
        .start_stack(request(&env, "build", "compose"), None)
        .unwrap();
    let cleanup = DockerCleanup {
        containers: stack.containers.iter().map(|c| c.id.clone()).collect(),
        project: Some(stack.project),
    };
    assert_eq!(stack.status, StackStatus::Running);
    env.core.stop_stack("build").unwrap();
    drop(cleanup);
    docker(&["image", "rm", &image]);
    assert_eq!(env.core.clean().unwrap().removed, ["build"]);
}
