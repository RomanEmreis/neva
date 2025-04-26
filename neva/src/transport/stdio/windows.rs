//! Windows-specific implementation details

use tokio::process::{Command, Child};
use windows::{
    core::{Result, Error},
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::{
            Threading::{
                OpenThread, OpenProcess, ResumeThread, 
                PROCESS_ALL_ACCESS, THREAD_SUSPEND_RESUME, CREATE_SUSPENDED
            },
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Thread32First, Thread32Next, 
                TH32CS_SNAPTHREAD,
                THREADENTRY32,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
                JobObjectExtendedLimitInformation, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            },
        },
    },
};

/// Job Object wrapper for automatic handle closing
pub(super) struct Job(HANDLE);
unsafe impl Send for Job {}

impl Job {
    /// Creates and returns a new child process ['Child'] and ['Job'] - job object wrapper
    pub(super) fn new(command: &str, args: Vec<&str>) -> Result<(Job, Child)> {
        let command = "cmd.exe";
        let args = {
            let mut win_args = vec!["/c", options.command];
            win_args.extend_from_slice(&options.args);
            win_args
        };
        
        let (job_handle, child) = create_job_object_with_kill_on_close(command, args)?;
        let job = Self(job_handle);
        Ok((job, child))
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe { _ = CloseHandle(self.0); }
    }
}

/// Creates a process in the Job Object with the `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` policy.
/// All processes within will be terminated when the job is dropped.
#[inline]
fn create_job_object_with_kill_on_close(command: &str, args: Vec<&str>) -> Result<(HANDLE, Child)> {
    unsafe {
        let job = CreateJobObjectW(None, None)?;
        // Configure Job Object
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        let result = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );

        // Run a suspended child process
        let child = Command::new(command)
            .creation_flags(CREATE_SUSPENDED.0)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;

        // Find and resume the process main thread
        let pid = child.id().expect("Failed to get process id");
        let tid = get_main_thread_id(pid)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "Thread not found"))?;

        let thread_handle = OpenThread(THREAD_SUSPEND_RESUME, false, tid)?;
        let process_handle = OpenProcess(PROCESS_ALL_ACCESS, false, pid)?;

        AssignProcessToJobObject(job, process_handle)?;

        if ResumeThread(thread_handle) == u32::MAX {
            return Err(Error::from_win32());
        }

        CloseHandle(thread_handle)?;
        CloseHandle(process_handle)?;
        
        match result {
            Ok(_) => Ok((job, child)),
            Err(_) => Err(Error::from_win32()),
        }
    }
}

/// Finds the main thread ID for the specified process.
#[inline]
unsafe fn get_main_thread_id(process_id: u32) -> Option<u32> {
    let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).ok()?;
    let mut thread_entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };

    if Thread32First(snapshot, &mut thread_entry).is_ok() {
        loop {
            if thread_entry.th32OwnerProcessID == process_id {
                return Some(thread_entry.th32ThreadID);
            }
            if Thread32Next(snapshot, &mut thread_entry).is_err() {
                break;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use tokio::process::Command;
    use std::time::Duration;
    use windows::Win32::System::Threading::CREATE_SUSPENDED;
    use crate::transport::stdio::windows::{create_job_object_with_kill_on_close, get_main_thread_id};

    #[tokio::test]
    async fn it_tests_job_object_kills_children() -> Result<(), Box<dyn std::error::Error>> {
        let (_job, mut child) = create_job_object_with_kill_on_close(
            "cmd.exe",
            vec!["/c", "ping", "127.0.0.1", "-n", "5", "-w", "1000"]
        )?;

        tokio::time::sleep(Duration::from_secs(1)).await;

        child.kill().await.unwrap();
        child.wait().await.unwrap();

        let output = Command::new("tasklist")
            .kill_on_drop(true)
            .arg("/FI")
            .arg("IMAGENAME eq ping.exe")
            .output()
            .await
            .unwrap();

        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("ping.exe"),
            "Notepad should be killed"
        );

        Ok(())
    }

    #[tokio::test]
    async fn it_test_get_main_thread_id() {
        let mut child = Command::new("cmd.exe")
            .kill_on_drop(true)
            .arg("/c")
            .arg("pause")
            .creation_flags(CREATE_SUSPENDED.0)
            .spawn()
            .unwrap();

        let tid = unsafe { get_main_thread_id(child.id().unwrap()) }.unwrap();
        assert!(tid > 0, "Valid thread ID");

        child.kill().await.unwrap();
    }
}