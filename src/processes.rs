use std::io;

pub const RECORD_SIZE: usize = 88;
#[cfg(target_os = "mochios")]
const MAX_RECORDS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    Sleeping,
    Zombie,
    Unknown(u64),
}

impl ProcessState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Sleeping => "Sleeping",
            Self::Zombie => "Zombie",
            Self::Unknown(_) => "Unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u64,
    pub parent_pid: u64,
    pub name: String,
    pub state: ProcessState,
}

pub fn decode_records(bytes: &[u8], count: usize) -> Vec<ProcessInfo> {
    let mut processes = bytes
        .chunks_exact(RECORD_SIZE)
        .take(count)
        .filter_map(|record| {
            let pid = u64::from_ne_bytes(record[0..8].try_into().ok()?);
            if pid == 0 {
                return None;
            }
            let raw_state = u64::from_ne_bytes(record[16..24].try_into().ok()?);
            let parent_pid = u64::from_ne_bytes(record[24..32].try_into().ok()?);
            let name_end = record[32..]
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(56);
            let name = String::from_utf8_lossy(&record[32..32 + name_end]).into_owned();
            Some(ProcessInfo {
                pid,
                parent_pid,
                name: if name.is_empty() {
                    format!("Process {pid}")
                } else {
                    name
                },
                state: match raw_state {
                    1 => ProcessState::Running,
                    3 => ProcessState::Sleeping,
                    4 => ProcessState::Zombie,
                    other => ProcessState::Unknown(other),
                },
            })
        })
        .collect::<Vec<_>>();
    processes.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then(left.pid.cmp(&right.pid))
    });
    processes
}

#[cfg(target_os = "mochios")]
pub fn snapshot() -> io::Result<Vec<ProcessInfo>> {
    use mochi_user_syscall as syscall;

    let mut records = vec![0u8; RECORD_SIZE * MAX_RECORDS];
    let count = syscall::call2(
        syscall::SyscallNumber::ListProcesses,
        records.as_mut_ptr() as u64,
        records.len() as u64,
    )
    .map_err(|error| {
        io::Error::from_raw_os_error(error.errno().unwrap_or(5).min(i32::MAX as u64) as i32)
    })?;
    Ok(decode_records(&records, (count as usize).min(MAX_RECORDS)))
}

#[cfg(not(target_os = "mochios"))]
pub fn snapshot() -> io::Result<Vec<ProcessInfo>> {
    let mut processes = Vec::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry.file_name().to_string_lossy().parse::<u64>().ok() else {
            continue;
        };
        let status = match std::fs::read_to_string(entry.path().join("status")) {
            Ok(status) => status,
            Err(_) => continue,
        };
        let field = |key: &str| {
            status
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .map(str::trim)
        };
        let name = field("Name:").unwrap_or("Process").to_owned();
        let parent_pid = field("PPid:")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let state = match field("State:").and_then(|value| value.chars().next()) {
            Some('R') => ProcessState::Running,
            Some('Z' | 'X') => ProcessState::Zombie,
            Some(_) => ProcessState::Sleeping,
            None => ProcessState::Unknown(0),
        };
        processes.push(ProcessInfo {
            pid,
            parent_pid,
            name,
            state,
        });
    }
    processes.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then(left.pid.cmp(&right.pid))
    });
    Ok(processes)
}

#[cfg(target_os = "mochios")]
pub fn uptime_seconds() -> Option<u64> {
    use mochi_user_syscall as syscall;

    let mut timespec = [0i64; 2];
    syscall::call2(
        syscall::SyscallNumber::ClockGettime,
        1,
        timespec.as_mut_ptr() as u64,
    )
    .ok()?;
    u64::try_from(timespec[0]).ok()
}

#[cfg(not(target_os = "mochios"))]
pub fn uptime_seconds() -> Option<u64> {
    std::fs::read_to_string("/proc/uptime")
        .ok()?
        .split_whitespace()
        .next()?
        .split('.')
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_kernel_process_record() {
        let mut record = [0u8; RECORD_SIZE];
        record[0..8].copy_from_slice(&42u64.to_ne_bytes());
        record[16..24].copy_from_slice(&3u64.to_ne_bytes());
        record[24..32].copy_from_slice(&7u64.to_ne_bytes());
        record[32..38].copy_from_slice(b"editor");

        assert_eq!(
            decode_records(&record, 1),
            vec![ProcessInfo {
                pid: 42,
                parent_pid: 7,
                name: "editor".to_owned(),
                state: ProcessState::Sleeping,
            }]
        );
    }

    #[test]
    fn ignores_zero_pid_and_sorts_by_name() {
        let mut records = [0u8; RECORD_SIZE * 3];
        records[0..8].copy_from_slice(&2u64.to_ne_bytes());
        records[16..24].copy_from_slice(&1u64.to_ne_bytes());
        records[32..36].copy_from_slice(b"Zulu");
        records[RECORD_SIZE..RECORD_SIZE + 8].copy_from_slice(&1u64.to_ne_bytes());
        records[RECORD_SIZE + 16..RECORD_SIZE + 24].copy_from_slice(&1u64.to_ne_bytes());
        records[RECORD_SIZE + 32..RECORD_SIZE + 37].copy_from_slice(b"alpha");

        let decoded = decode_records(&records, 3);
        assert_eq!(
            decoded.iter().map(|item| item.pid).collect::<Vec<_>>(),
            [1, 2]
        );
    }
}
