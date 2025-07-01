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

const CMD: &str = "cmd";

/// Job Object wrapper for automatic handle closing
pub(super) struct Job(HANDLE);

// SAFETY:
// It is safe to implement `Send` for `Job` because:
// - `HANDLE` is just a raw pointer-like type (`isize`) and can be safely transferred between threads.
// - The Windows Job Object API is thread-safe: the handle can be used (e.g., assigned to processes or closed)
//   from any thread without violating memory safety or causing data races.
// - `Job` does not provide interior mutability or expose any mutable aliasing of its internals.
// - We do not implement `Sync`, so shared concurrent access is disallowed, aligning with typical handle semantics.
unsafe impl Send for Job {}

impl Job {
    /// Creates and returns a new child process ['Child'] and ['Job'] - job object wrapper
    pub(super) fn new(command: &str, args: &Vec<&str>) -> Result<(Job, Child)> {
        let (command, args) = if !command.contains(CMD) {
            let mut win_args = vec!["/c", command];
            win_args.extend_from_slice(args);
            (CMD, win_args)
        } else {
            (command, args.clone())
        }; 
        
        let (job_handle, child) = create_job_object_with_kill_on_close(command, args)?;
        let job = Self(job_handle);
        Ok((job, child))
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY:
        // This is safe because:
        // - `self.0` is a valid handle to a Job Object created by `CreateJobObjectW`.
        // - The handle is owned by this `Job` wrapper, and not aliased elsewhere.
        // - This is the only place where the handle is closed (via `Drop`), ensuring it is closed exactly once.
        // - `CloseHandle` is safe to call on a valid handle, and we ignore the result to prevent panicking during drop.
        unsafe { _ = CloseHandle(self.0); }
    }
}

/// Creates a process in the Job Object with the `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` policy.
/// All processes within will be terminated when the job is dropped.
#[inline]
fn create_job_object_with_kill_on_close(command: &str, args: Vec<&str>) -> Result<(HANDLE, Child)> {
    // SAFETY:
    // This block performs a sequence of Windows API calls that require unsafe operations.
    //
    // - `CreateJobObjectW`: Returns a valid job handle on success, which is managed and eventually closed by the caller.
    // - `SetInformationJobObject`: Safe to call with a properly initialized `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`
    //   struct. The pointer cast is safe because `info` is stack-allocated and lives long enough for the call.
    // - `Command::spawn` with `CREATE_SUSPENDED` is safe; the child is immediately suspended.
    // - `OpenThread` and `OpenProcess` are given thread/process IDs returned from `child.id()` and `get_main_thread_id`.
    //   We assume these functions return valid IDs for the current child process.
    // - `AssignProcessToJobObject`: The job and process handles are valid and open at this point.
    // - `ResumeThread`: Called only after the thread handle is successfully opened.
    // - `CloseHandle`: Closes valid handles after they are no longer needed.
    //
    // Invariant: The caller must ensure that `job` is eventually closed (e.g., with `CloseHandle` or wrapped in a RAII type),
    // and the returned `Child` is managed (e.g., `wait` or `kill`) to avoid leaking resources.
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
    // SAFETY:
    // This function is marked `unsafe` because it performs raw Windows API calls and dereferences pointers internally.
    //
    // - `CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)` returns a snapshot of all threads in the system.
    //   The returned handle is valid if `ok()?` succeeds.
    //
    // - `THREADENTRY32` is a POD struct and is safely initialized with a known size and default zeroed fields.
    //   `dwSize` is set to the expected size as required by the API.
    //
    // - `Thread32First` and `Thread32Next` fill in `thread_entry` with thread information. These calls are safe
    //   as long as `thread_entry` is properly initialized and its lifetime outlives the calls, which it does here.
    //
    // - The function returns the first thread found in the snapshot belonging to the given `process_id`,
    //   which is typically the main thread but is not guaranteed by Windows. This heuristic is commonly used
    //   and works in most real-world scenarios.
    //
    // - The snapshot handle is closed automatically by `CloseHandle` via the RAII wrapper in `Ok(Handle)`.
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