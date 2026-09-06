//! Linux test observations bound to PID and birth time, never process names.
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Identity {
    pub pid: u32,
    pub birth: u64,
    pub name: String,
}

fn read(pid: u32) -> Option<(Identity, u32, bool)> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => panic!("cannot observe owned process {pid}: {error}"),
    };
    let (head, rest) = stat.rsplit_once(')').expect("kernel stat format");
    let fields: Vec<_> = rest.split_whitespace().collect();
    Some((
        Identity {
            pid,
            birth: fields[19].parse().unwrap(),
            name: head.split_once('(').unwrap().1.to_owned(),
        },
        fields[1].parse().unwrap(),
        fields[0] != "Z",
    ))
}

impl Identity {
    pub fn alive(&self) -> bool {
        read(self.pid).is_some_and(|(current, _, live)| current.birth == self.birth && live)
    }
}

pub fn snapshot_tree(root: u32) -> Vec<Identity> {
    let rows: Vec<_> = std::fs::read_dir(Path::new("/proc"))
        .unwrap()
        .map(Result::unwrap)
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u32>().ok())
        .filter_map(read)
        .collect();
    let mut owned = vec![
        rows.iter()
            .find(|(id, _, live)| id.pid == root && *live)
            .expect("positive control: owned root must be alive")
            .0
            .clone(),
    ];
    loop {
        let mut added = false;
        for (id, parent, live) in &rows {
            if *live
                && !owned.iter().any(|old| old.pid == id.pid)
                && owned.iter().any(|old| old.pid == *parent)
            {
                owned.push(id.clone());
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    owned
}

pub async fn assert_terminated(owned: &[Identity]) {
    let outcome = tokio::time::timeout(Duration::from_secs(5), async {
        while owned.iter().any(Identity::alive) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        outcome.is_ok(),
        "owned processes survived cleanup: {:?}",
        owned.iter().filter(|id| id.alive()).collect::<Vec<_>>()
    );
}
