//! Keep the fork-to-exec descriptor inheritance window open deterministically.

use std::io::Read;
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

extern "C" {
    fn fork() -> i32;
    fn close(fd: i32) -> i32;
    fn read(fd: i32, buffer: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buffer: *const u8, count: usize) -> isize;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn _exit(status: i32) -> !;
}

pub struct ForkHolder {
    pid: i32,
    release: UnixStream,
}

impl ForkHolder {
    pub fn new() -> Self {
        let (mut parent, child) = UnixStream::pair().unwrap();
        parent
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let parent_fd = parent.as_raw_fd();
        let child_fd = child.as_raw_fd();
        // SAFETY: after fork the child calls only async-signal-safe syscalls,
        // using live socket descriptors and stack bytes, then exits without
        // running Rust destructors or touching inherited allocator locks.
        let pid = unsafe {
            let pid = fork();
            if pid == 0 {
                close(parent_fd);
                let mut byte = 1u8;
                if write(child_fd, &byte, 1) != 1 {
                    _exit(1);
                }
                read(child_fd, &mut byte, 1);
                _exit(0);
            }
            pid
        };
        assert!(pid > 0, "fork failed: {}", std::io::Error::last_os_error());
        drop(child);
        let holder = Self {
            pid,
            release: parent.try_clone().unwrap(),
        };
        parent.read_exact(&mut [0]).unwrap();
        holder
    }
}

impl Drop for ForkHolder {
    fn drop(&mut self) {
        self.release.shutdown(Shutdown::Write).unwrap();
        let mut status = 0;
        // SAFETY: pid names our child and status is valid writable storage.
        while unsafe { waitpid(self.pid, &mut status, 0) } == -1 {
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::Interrupted
            );
        }
        assert_eq!(status, 0, "fork holder failed");
    }
}
