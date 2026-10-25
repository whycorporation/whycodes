//! Windows Job Object around a spawned shell, so a kill reaches the processes
//! it started too. Unix gets the same from process groups ([`crate::kill_pid_group`]).

use std::ffi::c_void;
use std::os::windows::io::RawHandle;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
    fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
    fn TerminateJobObject(job: *mut c_void, exit_code: u32) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

/// Owns a Job Object holding one process and the children it starts after
/// [`ProcessTree::adopt`]. Dropping it closes the handle without killing
/// anything (no `KILL_ON_JOB_CLOSE`): like a Unix process group, a process
/// left running on purpose outlives a normal exit.
#[derive(Debug)]
pub struct ProcessTree {
    job: *mut c_void,
}

// SAFETY: a job handle may be used and closed from any thread.
unsafe impl Send for ProcessTree {}

impl ProcessTree {
    /// Put `process` in a new job. Children it already started stay outside;
    /// call this right after spawning. `None` when the job cannot be set up.
    pub fn adopt(process: RawHandle) -> Option<Self> {
        // SAFETY: null attributes and name create an unnamed, default job.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return None;
        }
        let tree = Self { job };
        // SAFETY: both handles are live; `process` is owned by the caller's child.
        if unsafe { AssignProcessToJobObject(job, process.cast()) } == 0 {
            tracing::debug!("process tree: AssignProcessToJobObject failed");
            return None;
        }
        Some(tree)
    }

    /// Terminate every process in the job.
    pub fn kill(&self) {
        // SAFETY: `self.job` is a live job handle until drop.
        if unsafe { TerminateJobObject(self.job, 1) } == 0 {
            tracing::debug!("process tree: TerminateJobObject failed");
        }
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        // SAFETY: closed exactly once, here.
        unsafe { CloseHandle(self.job) };
    }
}

#[cfg(test)]
mod tests {
    use std::os::windows::io::AsRawHandle;
    use std::time::{Duration, Instant};

    use super::ProcessTree;

    #[test]
    fn kill_terminates_the_adopted_process() {
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn ping");
        let tree = ProcessTree::adopt(child.as_raw_handle()).expect("adopt");
        let t0 = Instant::now();
        tree.kill();
        let status = child.wait().expect("wait");
        assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
        assert_eq!(status.code(), Some(1));
    }

    #[test]
    fn drop_leaves_the_process_running() {
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn ping");
        drop(ProcessTree::adopt(child.as_raw_handle()).expect("adopt"));
        std::thread::sleep(Duration::from_millis(200));
        assert!(child.try_wait().expect("try_wait").is_none());
        child.kill().expect("kill");
        child.wait().expect("reap");
    }
}
