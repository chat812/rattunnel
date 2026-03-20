/// Find the inode of a LISTEN socket on the given port by reading /proc/net/tcp[6].
fn listening_inode(port: u16) -> Option<u64> {
    let port_hex = format!("{:04X}", port);

    for path in &["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in content.lines().skip(1) {
            let fields: Vec<&str> = line.split_ascii_whitespace().collect();
            if fields.len() <= 9 {
                continue;
            }
            // state 0A = LISTEN
            if fields[3] != "0A" {
                continue;
            }
            // local_address field: XXXXXXXX:PPPP
            if fields[1]
                .split(':')
                .nth(1)
                .map_or(false, |p| p.eq_ignore_ascii_case(&port_hex))
            {
                if let Ok(inode) = fields[9].parse::<u64>() {
                    return Some(inode);
                }
            }
        }
    }
    None
}

/// Walk /proc/<pid>/fd/* to find which PID owns the socket inode.
fn pid_for_inode(inode: u64) -> Option<u32> {
    let target = format!("socket:[{}]", inode);

    let Ok(proc_dir) = std::fs::read_dir("/proc") else {
        return None;
    };

    for entry in proc_dir.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(format!("/proc/{}/fd", pid)) else {
            continue;
        };
        for fd in fds.flatten() {
            if let Ok(link) = std::fs::read_link(fd.path()) {
                if link.to_string_lossy() == target {
                    return Some(pid);
                }
            }
        }
    }
    None
}

/// Read /proc/<pid>/comm for the process name.
fn process_name(pid: u32) -> String {
    std::fs::read_to_string(format!("/proc/{}/comm", pid))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// If anything is still listening on `port`, find it and kill it.
/// Returns true if a process was killed.
pub fn kill_listener(port: u16) -> bool {
    let Some(inode) = listening_inode(port) else {
        return false; // port already free
    };
    let Some(pid) = pid_for_inode(inode) else {
        log::warn!("port {}: socket inode {} found but no owning PID", port, inode);
        return false;
    };

    let name = process_name(pid);
    log::info!(
        "port {}: still listening after removal — killing PID {} ({})",
        port, pid, name
    );

    unsafe {
        // SIGTERM first
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }

    // Give it 500ms to exit cleanly
    std::thread::sleep(std::time::Duration::from_millis(500));

    if listening_inode(port).is_some() {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
        log::warn!("port {}: PID {} didn't die on SIGTERM, sent SIGKILL", port, pid);
    }

    true
}
