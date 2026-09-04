//! Windows-specific implementation details
#![expect(
    unsafe_code,
    reason = "Win32 FFI: job objects, process and thread handles. Every site \
              below carries a SAFETY comment. This is the only module in the \
              crate that needs it, which is why the suppression is scoped here \
              rather than relaxed workspace-wide."
)]

use tokio::process::{Child, Command};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
            Threading::{
                CREATE_SUSPENDED, OpenProcess, OpenThread, PROCESS_ALL_ACCESS, ResumeThread,
                THREAD_SUSPEND_RESUME,
            },
        },
    },
    core::Error,
};

use std::path::{Path, PathBuf};

/// Extensions tried when the configured command carries none of its own, used
/// only when the environment does not say otherwise. Mirrors the `PATHEXT`
/// default that Windows itself ships with, minus the script hosts: a stdio
/// server is an executable or a launcher shim, not a `.vbs`.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

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
    ///
    /// Fails with [`std::io::ErrorKind::NotFound`] when the command resolves to
    /// nothing, which is what lets a caller tell a mistyped or unbuilt server
    /// from one that started and then died.
    pub(super) fn new(command: &str, args: &Vec<&str>) -> std::io::Result<(Job, Child)> {
        let program = resolve_command(command).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "not found on PATH (the working directory is not searched)",
            )
        })?;

        let (job_handle, child) = create_job_object_with_kill_on_close(&program, args)?;
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
        unsafe {
            _ = CloseHandle(self.0);
        }
    }
}

/// Resolves `command` to a concrete executable the way `cmd.exe` would.
///
/// `std` deliberately does not do this: `CreateProcessW` only ever appends
/// `.exe`, so a bare `npx` -- which ships as the `npx.cmd` shim, and is how
/// most MCP servers are launched -- resolves to nothing. Routing everything
/// through `cmd /c` was the older way around that, at the cost of spawning
/// `cmd.exe` itself: the spawn then always succeeded and a missing server
/// surfaced as a child exit long after the handshake had returned.
///
/// Resolving here instead keeps the shim working and makes the failure
/// reportable. It also hands `Command` a path ending in `.cmd` or `.bat` when
/// that is what the command is, which routes it through `std`'s own batch-file
/// handling and its argument escaping, rather than concatenating caller
/// arguments into a `cmd /c` line this module would have to escape itself.
///
/// `PATH` is searched, with each `PATHEXT` extension appended when the command
/// carries no extension of its own. The working directory is deliberately not
/// searched, unlike `cmd.exe`: resolving a server out of the current directory
/// is a plant vector, and a caller who means a local binary can pass a path.
fn resolve_command(command: &str) -> Option<PathBuf> {
    let extensions = pathext();
    let candidate = Path::new(command);

    // Anything carrying a separator is a path the caller chose, not a name to
    // look up.
    if candidate.components().count() > 1 {
        return first_existing(candidate, &extensions);
    }

    std::env::split_paths(&std::env::var_os("PATH")?)
        .find_map(|dir| first_existing(&dir.join(command), &extensions))
}

/// The extensions to try, from the environment when it offers any.
fn pathext() -> Vec<String> {
    let configured = std::env::var("PATHEXT")
        .ok()
        .filter(|value| !value.trim().is_empty());

    configured
        .as_deref()
        .unwrap_or(DEFAULT_PATHEXT)
        .split(';')
        .map(str::trim)
        .filter(|extension| !extension.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The first of `candidate` and its `PATHEXT` spellings that names a file.
fn first_existing(candidate: &Path, extensions: &[String]) -> Option<PathBuf> {
    // An extension the caller wrote is honored as-is first; `cmd.exe` still
    // goes on to try the `PATHEXT` spellings after it, so a `server.v2` next
    // to a `server.v2.exe` resolves the same way here as it would there.
    if candidate.extension().is_some() && candidate.is_file() {
        return Some(candidate.to_path_buf());
    }

    extensions.iter().find_map(|extension| {
        let mut spelling = candidate.as_os_str().to_owned();
        spelling.push(extension);

        let spelling = PathBuf::from(spelling);
        spelling.is_file().then_some(spelling)
    })
}

/// Creates a process in the Job Object with the `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` policy.
/// All processes within will be terminated when the job is dropped.
#[inline]
fn create_job_object_with_kill_on_close(
    program: &Path,
    args: &Vec<&str>,
) -> std::io::Result<(HANDLE, Child)> {
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
        let child = Command::new(program)
            .creation_flags(CREATE_SUSPENDED.0)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;

        // Find and resume the process main thread
        let pid = child
            .id()
            .ok_or_else(|| std::io::Error::other("the child exited before it could be resumed"))?;
        let tid =
            get_main_thread_id(pid).ok_or_else(|| std::io::Error::other("Thread not found"))?;

        let thread_handle = OpenThread(THREAD_SUSPEND_RESUME, false, tid)?;
        let process_handle = OpenProcess(PROCESS_ALL_ACCESS, false, pid)?;

        AssignProcessToJobObject(job, process_handle)?;

        if ResumeThread(thread_handle) == u32::MAX {
            return Err(Error::from_thread().into());
        }

        CloseHandle(thread_handle)?;
        CloseHandle(process_handle)?;

        match result {
            Ok(_) => Ok((job, child)),
            Err(_) => Err(Error::from_thread().into()),
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
    unsafe {
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
}

#[cfg(test)]
mod tests {
    use crate::transport::stdio::windows::{
        Job, create_job_object_with_kill_on_close, get_main_thread_id, resolve_command,
    };
    use std::path::Path;
    use std::time::Duration;
    use tokio::process::Command;
    use windows::Win32::System::Threading::CREATE_SUSPENDED;

    #[tokio::test]
    async fn it_tests_job_object_kills_children() -> Result<(), Box<dyn std::error::Error>> {
        let (_job, mut child) = create_job_object_with_kill_on_close(
            Path::new("cmd.exe"),
            &vec!["/c", "ping", "127.0.0.1", "-n", "5", "-w", "1000"],
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

    /// `cmd.exe` lives in `System32`, which is on `PATH` on any Windows that
    /// can run this, and is named without an extension here on purpose: the
    /// point is that `PATHEXT` is what finds it, which is exactly what `std`
    /// does not do.
    #[test]
    fn it_resolves_a_bare_name_through_pathext() {
        let resolved = resolve_command("cmd").expect("`cmd` must resolve on any Windows");

        assert_eq!(
            resolved.extension().map(|e| e.to_ascii_lowercase()),
            Some("exe".into()),
            "resolution must land on a real executable, got {}",
            resolved.display()
        );
        assert!(resolved.is_file(), "{} is not a file", resolved.display());
    }

    /// The same name spelled with its extension resolves to the same file.
    ///
    /// Compared case-insensitively on purpose: `PATHEXT` is spelled in upper
    /// case, and the extension is appended exactly as it is written there, so
    /// a bare name resolves to `cmd.EXE` while a spelled one keeps the
    /// caller's `cmd.exe`. Windows paths are case-insensitive, so that is the
    /// same file -- and `std`'s own batch-file detection matches `.bat` and
    /// `.cmd` in either case, which is what keeps an `npx.CMD` spelling
    /// working.
    #[test]
    fn it_honors_an_extension_the_caller_wrote() {
        let bare = resolve_command("cmd").expect("`cmd` must resolve");
        let spelled = resolve_command("cmd.exe").expect("`cmd.exe` must resolve");

        assert!(
            bare.as_os_str().eq_ignore_ascii_case(spelled.as_os_str()),
            "{} and {} must name the same file",
            bare.display(),
            spelled.display()
        );
    }

    /// The case #128 is about: nothing to resolve, reported as such rather than
    /// spawning `cmd.exe` and letting the child die out of band.
    #[test]
    fn it_resolves_nothing_for_a_command_that_does_not_exist() {
        assert!(resolve_command("neva-nonexistent-command-for-issue-128").is_none());
    }

    /// A path is taken as written rather than looked up, and a path to nothing
    /// stays nothing.
    #[test]
    fn it_does_not_search_path_for_a_command_carrying_a_separator() {
        assert!(resolve_command(r".\neva-nonexistent-command-for-issue-128").is_none());
    }

    /// End to end through `Job::new`: the error a caller can match on, rather
    /// than a successfully spawned `cmd.exe` wrapping a command that is not
    /// there.
    #[test]
    fn job_new_reports_a_command_that_cannot_be_resolved() {
        // `Job` is not `Debug`, so this cannot go through `expect_err`.
        let Err(err) = Job::new("neva-nonexistent-command-for-issue-128", &vec![]) else {
            panic!("a command that resolves to nothing must not spawn");
        };

        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
