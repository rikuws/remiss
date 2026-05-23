use std::{
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

pub(crate) fn configure_child_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        // SAFETY: `pre_exec` runs after fork and before exec. The closure only
        // calls the async-signal-safe libc `setpgid` and constructs an OS error
        // from errno when that call fails.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
    }

    #[cfg(not(unix))]
    {
        let _ = command;
    }
}

pub(crate) fn terminate_child_process_group_with_id(
    child: &mut Child,
    process_group_id: Option<u32>,
    grace: Duration,
) {
    #[cfg(unix)]
    {
        signal_process_group(child.id(), process_group_id, libc::SIGTERM);
        if wait_until_child_exits(child, grace) {
            return;
        }
        signal_process_group(child.id(), process_group_id, libc::SIGKILL);
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(not(unix))]
    {
        let _ = (process_group_id, grace);
        let _ = child.kill();
        let _ = child.wait();
    }
}

pub(crate) fn terminate_process_group_by_id(
    process_id: u32,
    process_group_id: Option<u32>,
    grace: Duration,
) {
    #[cfg(unix)]
    {
        signal_process_group(process_id, process_group_id, libc::SIGTERM);
        thread::sleep(grace);
        signal_process_group(process_id, process_group_id, libc::SIGKILL);
    }

    #[cfg(not(unix))]
    {
        let _ = (process_id, process_group_id, grace);
    }
}

pub(crate) fn lookup_child_process_group_id(process_id: u32) -> Option<u32> {
    #[cfg(unix)]
    {
        let process_group_id = unsafe { libc::getpgid(process_id as libc::pid_t) };
        (process_group_id > 0).then_some(process_group_id as u32)
    }

    #[cfg(not(unix))]
    {
        let _ = process_id;
        None
    }
}

fn wait_until_child_exits(child: &mut Child, timeout: Duration) -> bool {
    let started_at = Instant::now();
    loop {
        if child.try_wait().ok().flatten().is_some() {
            return true;
        }
        if started_at.elapsed() >= timeout {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn signal_process_group(process_id: u32, process_group_id: Option<u32>, signal: libc::c_int) {
    let process_id = process_id as libc::pid_t;
    let process_group_id = process_group_id.unwrap_or(process_id as u32) as libc::pid_t;
    if process_group_id <= 0 {
        return;
    }

    // SAFETY: launched commands are requested to run in their own process group.
    // If the recorded group matches this app's group, fall back to the child pid
    // so cleanup cannot signal unrelated app siblings.
    unsafe {
        if libc::getpgrp() == process_group_id {
            let _ = libc::kill(process_id, signal);
        } else {
            let _ = libc::kill(-process_group_id, signal);
        }
    }
}
