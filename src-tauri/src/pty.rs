use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// A live PTY: master handle for IO + the child process.
pub struct Pty {
    pub id: u64,
    pub master: Box<dyn MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn Child + Send + Sync>,
    pub cols: u16,
    pub rows: u16,
    pub shell: String,
}

pub struct PtySpec {
    pub shell: String,
    pub cols: u16,
    pub rows: u16,
}

impl Pty {
    pub fn next_pane_id() -> u64 {
        next_id()
    }

    /// Spawn a new PTY running `shell`.
    pub fn spawn(spec: &PtySpec) -> anyhow::Result<Self> {
        let pty_system = native_pty_system();
        let size = PtySize {
            rows: spec.rows,
            cols: spec.cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system.openpty(size)?;
        let mut cmd = CommandBuilder::new(&spec.shell);
        cmd.cwd(std::env::var("HOME").unwrap_or_else(|_| "/".to_string()));

        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let writer = pair.master.take_writer()?;

        Ok(Self {
            id: next_id(),
            master: pair.master,
            writer,
            child,
            cols: spec.cols,
            rows: spec.rows,
            shell: spec.shell.clone(),
        })
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        if cols == 0 || rows == 0 {
            return Ok(());
        }
        self.cols = cols;
        self.rows = rows;
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    pub fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Read loop. Runs on its own thread, invoking `on_data` for each chunk.
    pub fn read_loop<F>(mut reader: Box<dyn Read + Send>, on_data: F)
    where
        F: Fn(Vec<u8>) + Send + 'static,
    {
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => on_data(buf[..n].to_vec()),
                }
            }
        });
    }

    /// Kill the child process and reap it.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
