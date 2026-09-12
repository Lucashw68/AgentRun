mod support;
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};
use support::*;

struct Client {
    child: Child,
    stdin: ChildStdin,
    responses: mpsc::Receiver<Value>,
    next_id: u64,
}
impl Client {
    fn new(env: &Env) -> Self {
        let mut child = env
            .command(MCP)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, responses) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                let Ok(value) = serde_json::from_str(&line) else {
                    panic!("Non-JSON MCP stdout: {line}");
                };
                if send.send(value).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            stdin,
            responses,
            next_id: 0,
        };
        let init = client.request("initialize",json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"codex","version":"test"}}));
        assert_eq!(init["result"]["serverInfo"]["name"], "agentrun");
        client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        client
    }
    fn send(&mut self, message: Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        loop {
            let value = self
                .responses
                .recv_timeout(Duration::from_secs(15))
                .expect("MCP response timed out");
            if value["id"] == id {
                return value;
            }
        }
    }
    fn call(&mut self, tool: &str, args: Value) -> Value {
        let response = self.request("tools/call", json!({"name":tool,"arguments":args}));
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }
    fn denied(&mut self, args: Value, code: &str) {
        let result = self.call("start_process", args);
        assert_eq!(result["isError"], true, "{result}");
        assert_eq!(
            result["structuredContent"]["error"]["code"], code,
            "{result}"
        );
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn real_stdio_protocol_narrow_tools_policy_and_no_network() {
    let env = Env::new();
    env.config(json!({"worker":{"command":[fixture()]}}));
    let mut client = Client::new(&env);
    let tools = client.request("tools/list", json!({}))["result"]["tools"]
        .as_array()
        .unwrap()
        .clone();
    let mut names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "clean_registry",
            "get_logs",
            "get_process",
            "list_processes",
            "restart_process",
            "start_process",
            "stop_process"
        ]
    );
    let schema = &tools.iter().find(|t| t["name"] == "start_process").unwrap()["inputSchema"];
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"].as_object().unwrap().len(), 3);
    client.denied(
        json!({"id":"outside","cwd":env.root.path(),"profile":"worker"}),
        "CWD_NOT_ALLOWED",
    );
    client.denied(
        json!({"id":"unknown","cwd":env.cwd,"profile":"unknown"}),
        "UNKNOWN_PROFILE",
    );
    client.denied(
        json!({"id":"free","cwd":env.cwd,"profile":"worker","command":["sh","-c","true"]}),
        "VALIDATION_ERROR",
    );
    client.denied(
        json!({"id":"args","cwd":env.cwd,"profile":"worker","args":["arbitrary"]}),
        "VALIDATION_ERROR",
    );
    assert_eq!(
        client.call("run_command", json!({"command":"true"}))["isError"],
        true
    );
    let start = client.call(
        "start_process",
        json!({"id":"mcp-worker","cwd":env.cwd,"profile":"worker"}),
    );
    assert_ne!(start["isError"], true, "{start}");
    let p: agentrun::core::ManagedProcess =
        serde_json::from_value(start["structuredContent"]["process"].clone()).unwrap();
    ready(&p, "READY=");
    assert_eq!(p.owner.client.as_deref(), Some("codex"));
    assert_eq!(env.core.get(&p.id).unwrap().pid, p.pid);
    assert_eq!(
        client.call("list_processes", json!({}))["structuredContent"]["processes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        client.call("get_process", json!({"id":p.id}))["structuredContent"]["process"]["status"],
        "running"
    );
    assert!(
        client.call("get_logs", json!({"id":p.id,"tail":100}))["structuredContent"]["text"]
            .as_str()
            .unwrap()
            .contains("READY=")
    );
    // A Rust stdio-only MCP must not own any socket, including Unix listeners.
    for fd in fs::read_dir(format!("/proc/{}/fd", client.child.id())).unwrap() {
        if let Ok(link) = fs::read_link(fd.unwrap().path()) {
            assert!(
                !link.to_string_lossy().starts_with("socket:"),
                "Unexpected MCP socket: {}",
                link.display()
            );
        }
    }
    assert_eq!(
        client.call("stop_process", json!({"id":p.id}))["structuredContent"]["process"]["status"],
        "dead"
    );
    assert!(!alive(p.pid));
    assert_eq!(
        client.call("list_processes", json!({}))["structuredContent"]["processes"][0]["deadReason"],
        "Stopped"
    );
    assert_eq!(
        client.call("get_process", json!({"id":p.id}))["structuredContent"]["process"]["deadReason"],
        "Stopped"
    );
    assert_eq!(
        client.call("clean_registry", json!({}))["structuredContent"]["removed"],
        json!([p.id])
    );
}

#[test]
fn disconnect_preserves_managed_process_and_missing_config_denies_start() {
    let env = Env::new();
    let mut client = Client::new(&env);
    client.denied(
        json!({"id":"missing","cwd":env.cwd,"profile":"worker"}),
        "CONFIG_ERROR",
    );
    assert_eq!(
        client.call("list_processes", json!({}))["structuredContent"]["processes"],
        json!([])
    );
    env.config(json!({"worker":{"command":[fixture()]}}));
    let result = client.call(
        "start_process",
        json!({"id":"persistent","cwd":env.cwd,"profile":"worker"}),
    );
    let p: agentrun::core::ManagedProcess =
        serde_json::from_value(result["structuredContent"]["process"].clone()).unwrap();
    ready(&p, "READY=");
    drop(client);
    assert!(alive(p.pid));
    env.core.stop(&p.id).unwrap();
}

#[test]
fn mcp_refuses_transport_options() {
    let output = std::process::Command::new(MCP)
        .args(["--port", "9090"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn mcp_restart_cannot_relaunch_a_manual_command() {
    let env = Env::new();
    let old = env.start("manual", "basic");
    let mut client = Client::new(&env);
    assert_eq!(
        client.call("restart_process", json!({"id":old.id}))["structuredContent"]["error"]["code"],
        "PROFILE_REQUIRED"
    );
    assert!(alive(old.pid));
    env.config(json!({"worker":{"command":[fixture()]}}));
    let first = client.call(
        "start_process",
        json!({"id":"profile","cwd":env.cwd,"profile":"worker"}),
    );
    let old: agentrun::core::ManagedProcess =
        serde_json::from_value(first["structuredContent"]["process"].clone()).unwrap();
    ready(&old, "READY=");
    let restarted = client.call("restart_process", json!({"id":"profile"}));
    assert_ne!(restarted["isError"], true, "{restarted}");
    let new: agentrun::core::ManagedProcess =
        serde_json::from_value(restarted["structuredContent"]["process"].clone()).unwrap();
    ready(&new, "READY=");
    assert_ne!(old.pid, new.pid);
    assert!(!alive(old.pid));
    env.config(json!({}));
    assert_eq!(
        client.call("restart_process", json!({"id":"profile"}))["structuredContent"]["error"]["code"],
        "UNKNOWN_PROFILE"
    );
    assert!(alive(new.pid));
}

#[test]
fn oversized_unterminated_stdio_frame_closes_connection() {
    let env = Env::new();
    let mut client = Client::new(&env);
    // A missing newline must not permit unlimited read_until buffering.
    let _ = client.stdin.write_all(&vec![b'x'; 65 * 1024]);
    let _ = client.stdin.flush();
    until(|| client.child.try_wait().unwrap().is_some());
}

#[test]
fn mcp_core_concurrency_budget_refuses_excess_work() {
    let env = Env::new();
    let mut client = Client::new(&env);
    let registry = agentrun::core::registry::Registry::new(env.paths.clone());
    let tx = registry.transaction().unwrap();
    // Hold the real kernel registry lock so eight blocking calls stay active.
    for id in 100..109 {
        client.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"list_processes","arguments":{}}}));
    }
    let response = client
        .responses
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert_eq!(
        response["result"]["structuredContent"]["error"]["code"],
        "SERVER_BUSY"
    );
    drop(tx);
    for _ in 0..8 {
        let response = client
            .responses
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_ne!(response["result"]["isError"], true, "{response}");
    }
    assert_eq!(
        client.call("list_processes", json!({}))["structuredContent"]["processes"],
        json!([])
    );
}
