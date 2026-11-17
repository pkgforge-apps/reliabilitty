//! PTY creation and child supervision.

use std::collections::BTreeMap;
use std::ffi::{CString, OsString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::config::Shell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitStatus {
    pub code: i32,
    pub signaled: bool,
}

pub struct Pty {
    master: OwnedFd,
    child: libc::pid_t,
    pending: Vec<u8>,
    exit: Option<ExitStatus>,
}

impl Pty {
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        command: &[String],
        shell: Option<&Shell>,
        working_directory: Option<&Path>,
        environment: &BTreeMap<String, String>,
        cols: usize,
        rows: usize,
        width_px: usize,
        height_px: usize,
        window_id: u32,
    ) -> io::Result<Self> {
        let (program, args) = match (command.is_empty(), shell) {
            (false, _) => (command[0].clone(), command[1..].to_vec()),
            (true, Some(shell)) => (shell.program.clone(), shell.args.clone()),
            (true, None) => (default_shell(), Vec::new()),
        };
        let program_c = CString::new(program.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "program contains NUL"))?;
        let mut arg_strings = Vec::with_capacity(args.len() + 1);
        arg_strings.push(program_c.clone());
        for arg in &args {
            arg_strings.push(CString::new(arg.as_bytes()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "argument contains NUL")
            })?);
        }
        let mut argv: Vec<*const libc::c_char> =
            arg_strings.iter().map(|arg| arg.as_ptr()).collect();
        argv.push(std::ptr::null());

        let environment = build_environment(environment, window_id);
        let mut envp: Vec<*const libc::c_char> =
            environment.iter().map(|entry| entry.as_ptr()).collect();
        envp.push(std::ptr::null());

        let directory = working_directory
            .map(|path| {
                CString::new(path.as_os_str().as_bytes()).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidInput, "directory contains NUL")
                })
            })
            .transpose()?;

        let mut master_fd = -1;
        let mut slave_fd = -1;
        let winsize = make_winsize(cols, rows, width_px, height_px);
        // SAFETY: valid descriptor pointers and a null termios pointer.
        let result = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null(),
                &winsize,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openpty returned a fresh descriptor.
        let master = unsafe { OwnedFd::from_raw_fd(master_fd) };
        // SAFETY: openpty returned a fresh descriptor.
        let slave = unsafe { OwnedFd::from_raw_fd(slave_fd) };
        set_cloexec(master.as_raw_fd())?;
        set_cloexec(slave.as_raw_fd())?;

        // SAFETY: the child only calls async-signal-safe functions before exec.
        let child = unsafe { libc::fork() };
        if child < 0 {
            return Err(io::Error::last_os_error());
        }
        if child == 0 {
            child_exec(
                slave.as_raw_fd(),
                master.as_raw_fd(),
                &program_c,
                &argv,
                &envp,
                directory.as_ref(),
            );
        }

        drop(slave);
        set_nonblocking(master.as_raw_fd())?;
        Ok(Self {
            master,
            child,
            pending: Vec::new(),
            exit: None,
        })
    }

    pub fn fd(&self) -> std::os::fd::RawFd {
        self.master.as_raw_fd()
    }

    /// Reads available output. `WouldBlock` means nothing is ready; zero means
    /// the child closed the terminal.
    pub fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            // SAFETY: buffer length is valid.
            let result = unsafe {
                libc::read(
                    self.master.as_raw_fd(),
                    buffer.as_mut_ptr() as *mut libc::c_void,
                    buffer.len(),
                )
            };
            if result >= 0 {
                return Ok(result as usize);
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => return Err(error),
                _ if error.raw_os_error() == Some(libc::EIO) => return Ok(0),
                _ => return Err(error),
            }
        }
    }

    pub fn queue(&mut self, bytes: &[u8]) {
        const LIMIT: usize = 8 * 1024 * 1024;
        if self.pending.len() + bytes.len() > LIMIT {
            let excess = self.pending.len() + bytes.len() - LIMIT;
            self.pending.drain(..excess.min(self.pending.len()));
        }
        self.pending.extend_from_slice(bytes);
    }

    pub fn flush(&mut self) -> io::Result<()> {
        while !self.pending.is_empty() {
            // SAFETY: pending slice is valid.
            let written = unsafe {
                libc::write(
                    self.master.as_raw_fd(),
                    self.pending.as_ptr() as *const libc::c_void,
                    self.pending.len(),
                )
            };
            if written > 0 {
                self.pending.drain(..written as usize);
                continue;
            }
            if written == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => return Ok(()),
                _ if error.raw_os_error() == Some(libc::EIO) => return Ok(()),
                _ => return Err(error),
            }
        }
        Ok(())
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn resize(&mut self, cols: usize, rows: usize, width_px: usize, height_px: usize) {
        let winsize = make_winsize(cols, rows, width_px, height_px);
        // SAFETY: TIOCSWINSZ reads one winsize.
        unsafe {
            libc::ioctl(
                self.master.as_raw_fd(),
                libc::TIOCSWINSZ,
                &winsize as *const libc::winsize,
            );
        }
    }

    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        if let Some(exit) = self.exit {
            return Some(exit);
        }
        let mut status = 0;
        loop {
            // SAFETY: status is writable and pid is our child.
            let result = unsafe { libc::waitpid(self.child, &mut status, libc::WNOHANG) };
            if result == self.child {
                let status = decode_status(status);
                self.exit = Some(status);
                return Some(status);
            }
            if result == 0 {
                return None;
            }
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if error.raw_os_error() == Some(libc::ECHILD) {
                return None;
            }
            return None;
        }
    }

    /// Sends SIGHUP and SIGTERM to the child's process group.
    pub fn terminate(&mut self) {
        if self.exit.is_some() {
            return;
        }
        // The child is a session leader; negative pid means its process group.
        unsafe {
            libc::kill(-self.child, libc::SIGHUP);
            libc::kill(-self.child, libc::SIGTERM);
        }
        for _ in 0..20 {
            if self.try_wait().is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if self.exit.is_none() {
            unsafe {
                libc::kill(-self.child, libc::SIGHUP);
            }
        }
    }
}

fn child_exec(
    slave: i32,
    master: i32,
    program: &CString,
    argv: &[*const libc::c_char],
    envp: &[*const libc::c_char],
    directory: Option<&CString>,
) -> ! {
    unsafe {
        if libc::setsid() < 0 {
            child_fail("setsid");
        }
        if libc::ioctl(slave, libc::TIOCSCTTY, 0) < 0 {
            child_fail("TIOCSCTTY");
        }
        if libc::dup2(slave, 0) < 0 || libc::dup2(slave, 1) < 0 || libc::dup2(slave, 2) < 0 {
            child_fail("dup2");
        }
        if slave > 2 {
            libc::close(slave);
        }
        libc::close(master);
        if let Some(directory) = directory
            && libc::chdir(directory.as_ptr()) < 0
        {
            child_fail("chdir");
        }
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        libc::execvpe(program.as_ptr(), argv.as_ptr(), envp.as_ptr());
    }
    child_fail("exec");
}

fn child_fail(operation: &str) -> ! {
    let error = io::Error::last_os_error();
    let message = format!("rtty: {operation}: {error}\r\n");
    unsafe {
        libc::write(2, message.as_ptr() as *const libc::c_void, message.len());
        libc::_exit(127);
    }
}

fn build_environment(extra: &BTreeMap<String, String>, window_id: u32) -> Vec<CString> {
    let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    environment.insert("TERM".into(), "xterm-256color".into());
    environment
        .entry("COLORTERM".into())
        .or_insert_with(|| "truecolor".into());
    environment.insert("TERM_PROGRAM".into(), "Reliabilitty".into());
    environment.insert("TERM_PROGRAM_VERSION".into(), crate::VERSION.into());
    environment.insert("WINDOWID".into(), window_id.to_string().into());
    for (key, value) in extra {
        environment.insert(key.into(), value.into());
    }
    environment
        .into_iter()
        .filter_map(|(key, value)| {
            let mut entry = Vec::with_capacity(key.len() + value.len() + 1);
            entry.extend_from_slice(key.as_bytes());
            if entry.contains(&0) {
                return None;
            }
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            if entry.contains(&0) {
                return None;
            }
            CString::new(entry).ok()
        })
        .collect()
}

fn make_winsize(cols: usize, rows: usize, width_px: usize, height_px: usize) -> libc::winsize {
    libc::winsize {
        ws_row: rows.clamp(1, u16::MAX as usize) as u16,
        ws_col: cols.clamp(1, u16::MAX as usize) as u16,
        ws_xpixel: width_px.min(u16::MAX as usize) as u16,
        ws_ypixel: height_px.min(u16::MAX as usize) as u16,
    }
}

fn default_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string())
}

fn set_cloexec(fd: i32) -> io::Result<()> {
    // SAFETY: fcntl reads descriptor flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: same descriptor.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_nonblocking(fd: i32) -> io::Result<()> {
    // SAFETY: fcntl reads status flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: same descriptor.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn decode_status(status: i32) -> ExitStatus {
    if libc::WIFEXITED(status) {
        ExitStatus {
            code: libc::WEXITSTATUS(status),
            signaled: false,
        }
    } else if libc::WIFSIGNALED(status) {
        ExitStatus {
            code: 128 + libc::WTERMSIG(status),
            signaled: true,
        }
    } else {
        ExitStatus {
            code: status,
            signaled: false,
        }
    }
}
