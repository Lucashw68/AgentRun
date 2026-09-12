use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OwnerType {
    Agent,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    #[serde(rename = "type")]
    pub kind: OwnerType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
}
impl Default for Owner {
    fn default() -> Self {
        Self {
            kind: OwnerType::Manual,
            client: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Running,
    Dead,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedProcess {
    pub id: String,
    pub pid: i32,
    pub pgid: i32,
    pub cwd: String,
    pub command: Vec<String>,
    pub started_at: String,
    pub status: Status,
    pub ports: Vec<u16>,
    pub owner: Owner,
    pub process_start_time: String,
    pub boot_id: String,
    pub uid: u32,
    pub log_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dead_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryData {
    pub version: u32,
    pub processes: Vec<ManagedProcess>,
}
impl Default for RegistryData {
    fn default() -> Self {
        Self {
            version: 1,
            processes: vec![],
        }
    }
}

#[derive(Debug, Clone)]
pub struct StartRequest {
    pub id: String,
    pub cwd: Option<String>,
    pub command: Vec<String>,
    pub owner: Owner,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileRequest {
    pub id: String,
    pub cwd: String,
    pub profile: String,
}

#[derive(Debug, Serialize)]
pub struct LogsResult {
    pub id: String,
    pub text: String,
    pub truncated: bool,
}
#[derive(Debug, Serialize)]
pub struct CleanResult {
    pub removed: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct StopFailure {
    pub id: String,
    pub code: String,
    pub message: String,
}
#[derive(Debug, Serialize)]
pub struct StopAllResult {
    pub stopped: Vec<String>,
    pub errors: Vec<StopFailure>,
}
#[derive(Debug, Serialize)]
pub struct PortsResult {
    pub id: String,
    pub status: Status,
    pub ports: Vec<u16>,
}
