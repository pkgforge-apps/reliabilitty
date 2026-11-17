//! Signal handling through a self-pipe.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

static WAKE_FD: AtomicI32 = AtomicI32::new(-1);
static QUIT: AtomicBool = AtomicBool::new(false);
static HANGUP: AtomicBool = AtomicBool::new(false);

pub struct Signals {
    read: OwnedFd,
    write: OwnedFd,
}

impl Signals {
    pub fn install() -> io::Result<Self> {
        let mut fds = [0; 2];
        // SAFETY: fds is a valid two-element array.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: pipe2 returned two fresh descriptors.
        let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        // SAFETY: pipe2 returned two fresh descriptors.
        let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        WAKE_FD.store(write.as_raw_fd(), Ordering::SeqCst);
        for signal in [
            libc::SIGCHLD,
            libc::SIGTERM,
            libc::SIGINT,
            libc::SIGHUP,
            libc::SIGQUIT,
        ] {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = handler as *const () as usize;
            action.sa_flags = libc::SA_RESTART;
            unsafe {
                libc::sigemptyset(&mut action.sa_mask);
                if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
        }
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        }
        Ok(Self { read, write })
    }

    pub fn fd(&self) -> RawFd {
        self.read.as_raw_fd()
    }

    pub fn drain(&mut self) {
        let mut buffer = [0u8; 64];
        loop {
            let result = unsafe {
                libc::read(
                    self.read.as_raw_fd(),
                    buffer.as_mut_ptr() as *mut libc::c_void,
                    buffer.len(),
                )
            };
            if result <= 0 {
                break;
            }
        }
    }

    pub fn take_quit(&self) -> bool {
        QUIT.swap(false, Ordering::SeqCst)
    }

    pub fn take_hangup(&self) -> bool {
        HANGUP.swap(false, Ordering::SeqCst)
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        WAKE_FD.store(-1, Ordering::SeqCst);
        let _ = self.write.as_raw_fd();
    }
}

extern "C" fn handler(signal: libc::c_int) {
    match signal {
        libc::SIGCHLD => {}
        libc::SIGHUP => HANGUP.store(true, Ordering::SeqCst),
        _ => QUIT.store(true, Ordering::SeqCst),
    }
    let fd = WAKE_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        let byte = [1u8];
        unsafe {
            libc::write(fd, byte.as_ptr() as *const libc::c_void, 1);
        }
    }
}
