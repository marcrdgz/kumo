#![allow(dead_code)]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// The PTY master handle. Freshly spawned panes own a `portable-pty` master;
/// panes resumed across a daemon restart (`kumo update`) adopt an inherited raw
/// descriptor that survived the exec.
pub enum PtyMaster {
    Spawned(Box<dyn MasterPty + Send>),
    /// A PTY master descriptor inherited from the previous daemon process.
    /// This process owns the fd; it is closed on drop. Only readable/writable
    /// via dup'd descriptors (`Pty::reader`/writer), and resized with
    /// `TIOCSWINSZ` directly.
    #[cfg(unix)]
    Inherited { fd: i32 },
}

#[cfg(unix)]
impl Drop for PtyMaster {
    fn drop(&mut self) {
        if let Self::Inherited { fd } = *self {
            unsafe {
                libc::close(fd);
            }
        }
    }
}

#[derive(Debug)]
struct DummyChild;
impl portable_pty::ChildKiller for DummyChild {
    fn kill(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(DummyChild)
    }
}
impl Child for DummyChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        Ok(Some(portable_pty::ExitStatus::with_exit_code(0)))
    }
    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        Ok(portable_pty::ExitStatus::with_exit_code(0))
    }
    fn process_id(&self) -> Option<u32> {
        None
    }
    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        None
    }
}

/// The child process running in the PTY. Spawned panes own a reapable `Child`;
/// resumed panes can only signal the (now-reparented) process by pid.
pub enum PtyChild {
    Spawned(Box<dyn Child + Send + Sync>),
    /// Child of a *previous* daemon process: after exec it belongs to init, so
    /// we can signal it by pid but never wait/reap it.
    #[cfg(unix)]
    Pid { pid: Option<i32> },
}

/// A live PTY: master handle for IO + the child process.
pub struct Pty {
    pub id: u64,
    pub master: PtyMaster,
    pub writer: Box<dyn Write + Send>,
    pub child: PtyChild,
    pub cols: u16,
    pub rows: u16,
    pub shell: String,
}

/// Program to execute inside the PTY. When `program` is set it takes
/// precedence over `shell` (used for AI CLI panes). If neither is set,
/// the shell is used.
#[derive(Default)]
pub struct PtySpec {
    pub shell: String,
    pub program: Option<(String, Vec<String>)>,
    pub cwd: Option<PathBuf>,
    pub cols: u16,
    pub rows: u16,
}

impl Pty {
    pub fn next_pane_id() -> u64 {
        next_id()
    }

    /// Spawn a new PTY running `shell` (or `program` when set).
    pub fn spawn(spec: &PtySpec) -> Result<Self> {
        let pty_system = native_pty_system();
        let size = PtySize {
            rows: spec.rows,
            cols: spec.cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system.openpty(size)?;
        let mut cmd = match &spec.program {
            Some((program, args)) => {
                let mut c = CommandBuilder::new(program);
                for a in args {
                    c.arg(a);
                }
                c
            }
            None => CommandBuilder::new(&spec.shell),
        };
        let cwd = spec.cwd.clone().unwrap_or_else(|| {
            std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
        });
        cmd.cwd(cwd);
        // Present the pane as a plain xterm instead of inheriting the host
        // TERM (e.g. `xterm-ghostty`): host terminfo entries advertise mouse
        // capabilities (XM/kmous) that make vim/less/opencode enable mouse
        // reporting, which would steal mouse events from kumo's text
        // selection.
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let writer = pair.master.take_writer()?;

        Ok(Self {
            id: next_id(),
            master: PtyMaster::Spawned(pair.master),
            writer,
            child: PtyChild::Spawned(child),
            cols: spec.cols,
            rows: spec.rows,
            shell: spec.program.as_ref().map(|p| p.0.clone()).unwrap_or_else(|| spec.shell.clone()),
        })
    }

    /// Adopt a PTY master descriptor inherited across a daemon restart. The
    /// child process keeps running inside the (already-open) PTY; the new
    /// daemon only re-establishes the read/write handles and the size.
    #[cfg(unix)]
    pub fn resume(
        id: u64,
        fd: i32,
        child_pid: Option<i32>,
        cols: u16,
        rows: u16,
        shell: String,
    ) -> Result<Self> {
        use std::os::unix::io::FromRawFd;
        // The original fd stays owned by `PtyMaster::Inherited` (kept open for
        // resize); writing goes through a dup'd descriptor so no two Rust
        // values ever own the same fd. The read side is dup'd by `Pty::reader`.
        let write_fd = unsafe { libc::dup(fd) };
        if write_fd < 0 {
            bail!("failed to dup inherited PTY master: {}", std::io::Error::last_os_error());
        }
        let writer: Box<dyn Write + Send> =
            Box::new(unsafe { std::fs::File::from_raw_fd(write_fd) });

        Ok(Self {
            id,
            master: PtyMaster::Inherited { fd },
            writer,
            child: PtyChild::Pid { pid: child_pid },
            cols,
            rows,
            shell,
        })
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        if cols == 0 || rows == 0 {
            return Ok(());
        }
        self.cols = cols;
        self.rows = rows;
        match &self.master {
            PtyMaster::Spawned(m) => m.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })?,
            #[cfg(unix)]
            PtyMaster::Inherited { fd } => resize_fd(*fd, cols, rows)?,
        }
        Ok(())
    }

    /// A fresh readable handle on the PTY master (for the read loop). Safe to
    /// call once per pane.
    pub fn reader(&self) -> Result<Box<dyn Read + Send>> {
        match &self.master {
            PtyMaster::Spawned(m) => m.try_clone_reader(),
            #[cfg(unix)]
            PtyMaster::Inherited { fd } => {
                use std::os::unix::io::FromRawFd;
                let n = unsafe { libc::dup(*fd) };
                if n < 0 {
                    bail!("failed to dup inherited PTY master: {}", std::io::Error::last_os_error());
                }
                Ok(Box::new(unsafe { std::fs::File::from_raw_fd(n) }))
            }
        }
    }

    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data)?;
        Ok(())
    }

    /// Read loop. Runs on its own thread, invoking `on_data` for each chunk.
    pub fn read_loop<F>(reader: Box<dyn Read + Send>, on_data: F)
    where
        F: Fn(Vec<u8>) + Send + 'static,
    {
        let _ = std::thread::Builder::new()
            .name("kumo-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                let mut reader = reader;
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => on_data(buf[..n].to_vec()),
                    }
                }
            });
    }

    /// The child's process id (to signal it), if known.
    pub fn process_id(&self) -> Option<u32> {
        match &self.child {
            PtyChild::Spawned(c) => c.process_id(),
            #[cfg(unix)]
            PtyChild::Pid { pid } => pid.map(|p| p as u32),
        }
    }

    /// Non-blocking exit probe. Resumed children are reparented to init, which
    /// reaps them, so liveness is checked with `kill(pid, 0)` instead.
    pub fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        match &mut self.child {
            PtyChild::Spawned(c) => c.try_wait(),
            #[cfg(unix)]
            PtyChild::Pid { pid } => match pid {
                Some(pid) => {
                    let rc = unsafe { libc::kill(*pid, 0) };
                    if rc == 0 {
                        Ok(None)
                    } else {
                        let err = std::io::Error::last_os_error();
                        if err.raw_os_error() == Some(libc::ESRCH) {
                            Ok(Some(portable_pty::ExitStatus::with_exit_code(0)))
                        } else {
                            // EPERM or other: process exists but we lack permission, treat as alive.
                            Ok(None)
                        }
                    }
                }
                None => Ok(Some(portable_pty::ExitStatus::with_exit_code(0))),
            },
        }
    }

    /// Kill the child process. Spawned children are reaped too; resumed
    /// children were reparented to init by the previous daemon's exec, so they
    /// are only signalled. Never blocks the daemon loop: the actual `wait` and
    /// `SIGKILL` fallback run in a detached thread.
    pub fn kill(&mut self) {
        // Take ownership of the child so we can move it into a waiter thread.
        #[cfg(unix)]
        let child = std::mem::replace(&mut self.child, PtyChild::Pid { pid: None });
        #[cfg(not(unix))]
        let child = std::mem::replace(
            &mut self.child,
            PtyChild::Spawned(Box::new(DummyChild)),
        );
        match child {
            PtyChild::Spawned(mut c) => {
                let _ = c.kill();
                let _ = std::thread::Builder::new()
                    .name("kumo-pty-killer".into())
                    .spawn(move || {
                        use std::time::{Duration, Instant};
                        let deadline = Instant::now() + Duration::from_millis(500);
                        loop {
                            match c.try_wait() {
                                Ok(Some(_)) => break,
                                Ok(None) if Instant::now() < deadline => {
                                    std::thread::sleep(Duration::from_millis(10));
                                }
                                Ok(None) => {
                                    #[cfg(unix)]
                                    if let Some(pid) = c.process_id() {
                                        unsafe {
                                            libc::kill(pid as i32, libc::SIGKILL);
                                        }
                                    }
                                    let _ = c.wait();
                                    break;
                                }
                                Err(_) => {
                                    let _ = c.wait();
                                    break;
                                }
                            }
                        }
                    });
            }
            #[cfg(unix)]
            PtyChild::Pid { pid: Some(pid) } => {
                unsafe {
                    // Signal the process group and the pid itself.
                    libc::kill(-pid, libc::SIGTERM);
                    libc::kill(pid, libc::SIGTERM);
                }
                let _ = std::thread::Builder::new()
                    .name("kumo-pty-killer".into())
                    .spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(200));
                        unsafe {
                            libc::kill(-pid, libc::SIGKILL);
                            libc::kill(pid, libc::SIGKILL);
                        }
                    });
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    /// The raw PTY master descriptor, when known (used to clear `FD_CLOEXEC`
    /// before the daemon execs a new binary for `kumo update`).
    #[cfg(unix)]
    pub fn raw_fd(&self) -> Option<i32> {
        match &self.master {
            PtyMaster::Spawned(m) => m.as_raw_fd(),
            PtyMaster::Inherited { fd } => Some(*fd),
        }
    }
}

/// Resize an inherited PTY master directly (`TIOCSWINSZ`), mirroring what
/// `portable-pty` does internally for spawned masters.
#[cfg(unix)]
fn resize_fd(fd: i32, cols: u16, rows: u16) -> Result<()> {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
    if rc != 0 {
        bail!("TIOCSWINSZ failed: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(unix, test))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A resumed `Pty` adopts a live PTY master across a "restart" (simulated
    /// here by forgetting the original handle so its fd is never closed): the
    /// shell keeps running and IO keeps working through the adopted fd.
    #[test]
    fn resumed_pty_keeps_live_shell_io() {
        let spec = PtySpec {
            shell: "/bin/sh".into(),
            program: None,
            cwd: Some(std::env::temp_dir()),
            cols: 40,
            rows: 10,
        };
        let pty = Pty::spawn(&spec).unwrap();
        let fd = pty.raw_fd().expect("spawned pty must expose its master fd");
        let child_pid = pty.process_id().expect("spawned shell has a pid");
        // Simulate exec-inheritance: the original handle is forgotten, so its
        // fd stays open for `Pty::resume` to adopt (exactly what the daemon
        // restart relies on after clearing FD_CLOEXEC).
        std::mem::forget(pty);

        let mut resumed =
            Pty::resume(99, fd, Some(child_pid as i32), 40, 10, "/bin/sh".into()).unwrap();
        assert_eq!(resumed.raw_fd(), Some(fd));
        assert_eq!(resumed.process_id(), Some(child_pid));

        // The shell is still alive inside the PTY: a command echoes back.
        resumed.write(b"echo KUMO_RESUME_OK\r\n").unwrap();
        let mut reader = resumed.reader().unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline
            && !got.windows(b"KUMO_RESUME_OK".len()).any(|w| w == b"KUMO_RESUME_OK")
        {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        assert!(
            got.windows(b"KUMO_RESUME_OK".len()).any(|w| w == b"KUMO_RESUME_OK"),
            "resumed PTY did not pass IO through: {:?}",
            String::from_utf8_lossy(&got)
        );

        resumed.kill();
    }
}
