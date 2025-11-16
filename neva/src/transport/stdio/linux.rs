//! Linux-specific implementation details

use tokio::process::{Child, Command};
use nix::{
    sys::signal::{killpg, Signal},
    unistd::Pid,
};

/// Process group wrapper for automatic handle closing
pub(super) struct Job(i32);

impl Job {
    /// Creates and returns a new child process ['Child'] and ['Job'] - process group wrapper
    pub(super) fn new(command: &str, args: &Vec<&str>) -> std::io::Result<(Job, Child)> {
        let (job_handle, child) = create_process_group(command, args)?;
        let job = Self(job_handle);
        Ok((job, child))
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        let _ = killpg(Pid::from_raw(self.0), Signal::SIGTERM);
    }
}

/// Creates a process in a new group with automatic termination
#[inline]
pub(super) fn create_process_group(command: &str, args: &Vec<&str>) -> std::io::Result<(i32, Child)> {
    let child = Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .process_group(0)
        .spawn()?;

    let group_pid = child.id().expect("Failed to get process id");
    
    Ok((group_pid as i32, child))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::process::Command;

    #[tokio::test]
    async fn it_tests_process_group_kill() {
        let (job, _) = create_process_group(
            "sh",
            &vec!["-c", "sleep 300 & sleep 300"]
        ).unwrap();
        
        let job = Job(job);

        tokio::time::sleep(Duration::from_millis(100)).await;
        
        drop(job);

        let output = Command::new("pgrep")
            .arg("-f")
            .arg("sleep 300")
            .output()
            .await
            .unwrap();

        assert!(output.stdout.is_empty(), "Processes still running");
    }
}