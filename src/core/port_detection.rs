use super::{proc, types::ManagedProcess};
use std::{
    collections::{BTreeSet, HashSet},
    fs,
};

/// Best effort, local /proc only. Permission errors and disappearing fds do not
/// prevent registry operations. Includes group members and visible descendants.
pub fn detect(record: &ManagedProcess) -> Vec<u16> {
    if !proc::matches(record) {
        return vec![];
    }
    let table = proc::table();
    let mut pids: HashSet<i32> = table
        .iter()
        .filter(|p| p.pgid == record.pgid && p.session == record.pgid)
        .map(|p| p.pid)
        .collect();
    pids.insert(record.pid);
    loop {
        let len = pids.len();
        for p in &table {
            if pids.contains(&p.ppid) {
                pids.insert(p.pid);
            }
        }
        if pids.len() == len {
            break;
        }
    }
    let mut ports = BTreeSet::new();
    for p in table.iter().filter(|p| pids.contains(&p.pid)) {
        let inodes = proc::socket_inodes(p.pid);
        if !proc::inspect(p.pid).is_some_and(|current| current.same_process(p)) {
            continue;
        }
        for protocol in ["tcp", "tcp6"] {
            let Ok(text) = fs::read_to_string(format!("/proc/{}/net/{protocol}", p.pid)) else {
                continue;
            };
            for line in text.lines().skip(1) {
                let fields: Vec<_> = line.split_whitespace().collect();
                if fields.get(3) == Some(&"0A")
                    && fields.get(9).is_some_and(|inode| inodes.contains(*inode))
                    && let Some(port) = fields
                        .get(1)
                        .and_then(|s| s.split(':').nth(1))
                        .and_then(|s| u16::from_str_radix(s, 16).ok())
                    && port != 0
                {
                    ports.insert(port);
                }
            }
        }
    }
    if proc::matches(record) {
        ports.into_iter().collect()
    } else {
        vec![]
    }
}
