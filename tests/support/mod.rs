#![allow(dead_code)]
use agentrun::core::{AgentRun, ManagedProcess, Owner, Paths, StartRequest, proc};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

pub const CLI: &str = env!("CARGO_BIN_EXE_agentrun");
pub const MCP: &str = env!("CARGO_BIN_EXE_agentrun-mcp");

pub fn fixture() -> &'static Path {
    static FIXTURE: OnceLock<(TempDir, PathBuf)> = OnceLock::new();
    &FIXTURE
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let bin = dir.path().join("agentrun-test-worker");
            let result = Command::new("rustc")
                .args([
                    "--edition=2024",
                    concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/tests/support/process_fixture.rs"
                    ),
                    "-o",
                ])
                .arg(&bin)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            (dir, bin)
        })
        .1
}
pub struct Env {
    pub root: TempDir,
    pub paths: Paths,
    pub core: AgentRun,
    pub cwd: PathBuf,
}
impl Env {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("projects");
        fs::create_dir(&cwd).unwrap();
        let paths = Paths::new(
            root.path().join("state/agentrun"),
            root.path().join("config/agentrun/config.json"),
        );
        fs::create_dir_all(paths.config.parent().unwrap()).unwrap();
        let core = AgentRun::new(paths.clone()).with_grace(Duration::from_millis(200));
        Self {
            root,
            paths,
            core,
            cwd,
        }
    }
    pub fn request(&self, id: &str, mode: &str) -> StartRequest {
        StartRequest {
            id: id.into(),
            cwd: Some(self.cwd.to_str().unwrap().into()),
            command: vec![fixture().to_str().unwrap().into(), mode.into()],
            owner: Owner::default(),
        }
    }
    pub fn start(&self, id: &str, mode: &str) -> ManagedProcess {
        let p = self.core.start(self.request(id, mode)).unwrap();
        if mode != "exit" {
            ready(&p, "READY=");
        }
        p
    }
    pub fn command(&self, binary: &str) -> Command {
        let mut command = Command::new(binary);
        command
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"));
        command
    }
    pub fn cli(&self, args: &[&str]) -> Output {
        self.command(CLI).args(args).output().unwrap()
    }
    pub fn json(&self, args: &[&str]) -> serde_json::Value {
        let out = self.cli(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    pub fn config(&self, profiles: serde_json::Value) {
        fs::write(
            &self.paths.config,
            serde_json::to_vec(&serde_json::json!({"allowedRoots":[self.cwd],"profiles":profiles}))
                .unwrap(),
        )
        .unwrap();
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        if let Ok(result) = self.core.stop_all() {
            assert!(result.errors.is_empty(), "{:?}", result.errors);
        }
    }
}
pub fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < deadline, "Timed out waiting for condition");
        thread::sleep(Duration::from_millis(20));
    }
}
pub fn ready(p: &ManagedProcess, pattern: &str) -> String {
    let mut text = String::new();
    until(|| {
        text = fs::read_to_string(&p.log_path).unwrap_or_default();
        text.contains(pattern)
    });
    text
}
pub fn number(text: &str, prefix: &str) -> i32 {
    text.lines()
        .find_map(|line| line.strip_prefix(prefix))
        .unwrap()
        .parse()
        .unwrap()
}
pub fn alive(pid: i32) -> bool {
    proc::inspect(pid).is_some_and(|p| p.alive())
}
