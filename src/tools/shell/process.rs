use std::collections::HashMap;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use tokio::process::Command;

pub(super) fn shell_command(
    command: &str,
    cwd: &Path,
    env: Option<&HashMap<String, String>>,
) -> Command {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(command);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    };
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    #[cfg(unix)]
    cmd.process_group(0);

    if let Some(env_vars) = env {
        for (k, v) in env_vars {
            cmd.env(k, v);
        }
    }

    cmd
}

#[cfg(unix)]
#[must_use]
fn signal_name(sig: i32) -> &'static str {
    match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _ => "UNKNOWN",
    }
}

#[must_use]
pub(super) fn exit_description(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => code.to_string(),
        None => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                match status.signal() {
                    Some(sig) => {
                        let name = signal_name(sig);
                        if name == "UNKNOWN" {
                            format!("signal: {sig}")
                        } else {
                            format!("signal: {sig} ({name})")
                        }
                    }
                    None => "unknown".to_string(),
                }
            }
            #[cfg(not(unix))]
            "signal".to_string()
        }
    }
}

pub(super) fn kill_child(child: &mut tokio::process::Child, child_id: Option<u32>) {
    #[cfg(unix)]
    let _ = child;
    #[cfg(unix)]
    if let Some(id) = child_id {
        // Child processes are started in their own process group; kill the
        // group so shell grandchildren do not survive a timeout.
        // Guard: PID 0 (self) and PID 1 (init) must never be killed.
        if id > 1 {
            unsafe { libc::kill(-(id as i32), libc::SIGKILL) };
        }
    }
    #[cfg(not(unix))]
    child.start_kill().ok();
}
