mod support;
use std::{fs, process::Stdio, thread};
use support::*;

#[test]
fn json_contract_and_detached_process_across_cli_invocations() {
    let env = Env::new();
    let started = env.json(&[
        "start",
        "cli-worker",
        "--cwd",
        env.cwd.to_str().unwrap(),
        "--client",
        "codex",
        "--json",
        "--",
        fixture().to_str().unwrap(),
    ]);
    let p: agentrun::core::ManagedProcess =
        serde_json::from_value(started["process"].clone()).unwrap();
    ready(&p, "STDERR ready");
    assert_eq!(started["version"], 1);
    let listed = env.json(&["list", "--json"]);
    assert_eq!(listed["processes"][0]["pid"], p.pid);
    assert_eq!(listed["processes"][0]["owner"]["client"], "codex");
    assert_eq!(
        env.json(&["status", "cli-worker", "--json"])["process"]["status"],
        "running"
    );
    assert!(
        String::from_utf8(env.cli(&["list"]).stdout)
            .unwrap()
            .contains("OWNER")
    );
    assert!(
        String::from_utf8(env.cli(&["logs", "cli-worker", "--tail", "1"]).stdout)
            .unwrap()
            .contains("STDERR ready")
    );
    assert_eq!(
        env.json(&["ports", "--json"])["processes"][0]["id"],
        "cli-worker"
    );
    assert_eq!(
        env.json(&["stop", "cli-worker", "--json"])["process"]["status"],
        "dead"
    );
    assert_eq!(
        env.json(&["list", "--json"])["processes"][0]["deadReason"],
        "Stopped"
    );
    assert_eq!(
        env.json(&["status", "cli-worker", "--json"])["process"]["deadReason"],
        "Stopped"
    );
    assert_eq!(
        env.json(&["clean", "--json"])["removed"],
        serde_json::json!(["cli-worker"])
    );
}

#[test]
fn concurrent_cli_writers_and_stop_all() {
    let env = Env::new();
    let records = thread::scope(|scope| {
        let threads: Vec<_> = (0..6)
            .map(|i| {
                let env = &env;
                scope.spawn(move || {
                    env.json(&[
                        "start",
                        &format!("cli-{i}"),
                        "--json",
                        "--",
                        fixture().to_str().unwrap(),
                    ])
                })
            })
            .collect();
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        env.json(&["list", "--json"])["processes"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    assert_eq!(
        env.json(&["stop", "--all", "--json"])["stopped"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    assert!(
        records
            .iter()
            .all(|record| !alive(record["process"]["pid"].as_i64().unwrap() as i32))
    );
}

#[test]
fn argv_is_literal_and_errors_are_structured() {
    let env = Env::new();
    let marker = env.root.path().join("must-not-exist");
    let arg = format!("$(touch {}); echo injected", marker.display());
    let start = env.json(&[
        "start",
        "literal",
        "--json",
        "--",
        fixture().to_str().unwrap(),
        "literal",
        &arg,
    ]);
    let p = serde_json::from_value(start["process"].clone()).unwrap();
    assert!(ready(&p, "LITERAL=").contains(&arg));
    assert!(!marker.exists());
    for args in [
        vec!["status", "absent"],
        vec!["start", "bad"],
        vec!["stop", "id", "--all"],
        vec!["list", "--tail", "1"],
        vec!["logs", "literal", "--tail", "-1"],
        vec!["logs", "literal", "--tail", "10001"],
    ] {
        let mut args = args;
        args.push("--json");
        let out = env.cli(&args);
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["version"], 1);
        assert!(error["error"]["code"].is_string());
    }
}

#[test]
fn native_lock_released_after_client_is_killed() {
    use std::{
        io::{BufRead, BufReader},
        time::Duration,
    };
    let env = Env::new();
    env.core.list().unwrap();
    // Run a dedicated test helper process; no production debug command exists.
    let mut holder = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lock_holder_fixture", "--nocapture"])
        .env("AGENTRUN_TEST_LOCK_STATE", &env.paths.state)
        .env("AGENTRUN_TEST_LOCK_CONFIG", &env.paths.config)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(holder.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        if line.contains("LOCKED") {
            break;
        }
    }
    let mut contender = env
        .command(CLI)
        .args(["list", "--json"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(contender.try_wait().unwrap().is_none());
    holder.kill().unwrap();
    holder.wait().unwrap();
    assert!(contender.wait_with_output().unwrap().status.success());
}

#[test]
fn lock_holder_fixture() {
    let Some(state) = std::env::var_os("AGENTRUN_TEST_LOCK_STATE") else {
        return;
    };
    let registry = agentrun::core::registry::Registry::new(agentrun::core::Paths::new(
        state.into(),
        std::env::var_os("AGENTRUN_TEST_LOCK_CONFIG")
            .unwrap()
            .into(),
    ));
    let _tx = registry.transaction().unwrap();
    println!("LOCKED");
    loop {
        thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[test]
fn xdg_defaults_and_relative_variable_fallback() {
    let env = Env::new();
    let output = env
        .command(CLI)
        .args(["list", "--json"])
        .env("HOME", env.root.path())
        .env("XDG_STATE_HOME", "relative")
        .env("XDG_CONFIG_HOME", "")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        env.root
            .path()
            .join(".local/state/agentrun/registry.json")
            .exists()
    );
    assert!(!env.root.path().join("relative").exists());
    let registry: serde_json::Value = serde_json::from_slice(
        &fs::read(env.root.path().join(".local/state/agentrun/registry.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(registry["version"], 1);
}

#[test]
fn restart_json_survives_terminal_sessions() {
    let env = Env::new();
    let first = env.json(&[
        "start",
        "restart-cli",
        "--json",
        "--",
        fixture().to_str().unwrap(),
    ]);
    let old: agentrun::core::ManagedProcess =
        serde_json::from_value(first["process"].clone()).unwrap();
    ready(&old, "READY=");
    let second = env.json(&["restart", "restart-cli", "--json"]);
    let new: agentrun::core::ManagedProcess =
        serde_json::from_value(second["process"].clone()).unwrap();
    ready(&new, "READY=");
    assert_eq!(second["version"], 1);
    assert_ne!(old.pid, new.pid);
    assert!(!alive(old.pid));
    assert_eq!(old.command, new.command);
}
