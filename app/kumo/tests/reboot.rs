//! Reboot recovery through the real binary: a daemon stopped by the OS
//! (SIGTERM at shutdown/logout) or killed outright (crash, power loss after
//! an autosave) comes back with its sessions on the next cold start, while
//! `kumo kill` ends them for good.
#![cfg(unix)]

use std::cell::RefCell;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A scratch kumo home plus every daemon started in it, all stopped on drop.
struct Root(PathBuf, RefCell<Vec<Child>>);

impl Root {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("kumo-reboot-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("ws")).unwrap();
        std::fs::write(root.join("config"), "shell = /bin/sh\nupdate-check = false\nnew-cwd = current\n").unwrap();
        Self(root, RefCell::new(Vec::new()))
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kumo"));
        cmd.env("KUMO_CONFIG_DIR", &self.0)
            .env("XDG_RUNTIME_DIR", &self.0)
            .env("XDG_STATE_HOME", &self.0)
            .env("KUMO_NO_UPDATE", "1")
            .stdin(Stdio::null());
        cmd
    }

    fn state_file(&self) -> PathBuf {
        self.0.join("kumo/state.json")
    }

    /// Start a daemon the way the `kumo` client does on a cold start; returns
    /// its pid. A killed daemon leaves its socket file behind, so readiness
    /// is a successful connect, not the file existing.
    fn start_daemon(&self, workspace: Option<&Path>) -> u32 {
        let mut cmd = self.command();
        cmd.arg("daemon");
        if let Some(dir) = workspace {
            cmd.arg(dir);
        }
        let child = cmd.stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let pid = child.id();
        self.1.borrow_mut().push(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        while UnixStream::connect(self.0.join("kumo/kumo.sock")).is_err() {
            assert!(Instant::now() < deadline, "daemon did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        pid
    }

    /// Send `sig` to the daemon `pid` and wait until it has exited.
    fn signal(&self, pid: u32, sig: &str) {
        let status = Command::new("kill").arg(format!("-{sig}")).arg(pid.to_string()).status().unwrap();
        assert!(status.success());
        let mut children = self.1.borrow_mut();
        let child = children.iter_mut().find(|c| c.id() == pid).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "daemon ignored SIG{sig}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn ok(&self, args: &[&str]) -> Value {
        let output = self.command().args(args).arg("--json").output().unwrap();
        let json: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
        assert!(output.status.success(), "`kumo {}` failed: {json}", args.join(" "));
        json
    }

    fn pane_ids(&self) -> Vec<u64> {
        let list = self.ok(&["pane", "list"]);
        list["tabs"][0]["panes"].as_array().unwrap().iter().map(|p| p["id"].as_u64().unwrap()).collect()
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = self.command().arg("kill").output();
        for child in self.1.get_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Build a split with a marker on screen, as a user would before rebooting.
fn build_layout(root: &Root) -> u64 {
    let pane = root.pane_ids()[0];
    root.ok(&["pane", "split"]);
    root.ok(&["pane", "send-keys", "-p", &pane.to_string(), "echo", "Space", "BEFORE_$((40+2))", "Enter"]);
    root.ok(&["pane", "wait-output", &pane.to_string(), "BEFORE_42", "--timeout", "10s"]);
    pane
}

fn assert_restored(root: &Root) {
    let panes = root.pane_ids();
    assert_eq!(panes.len(), 2, "the split comes back");
    let screens: String = panes
        .iter()
        .map(|id| root.ok(&["agent", "read", &id.to_string()])["text"].as_str().unwrap().to_string())
        .collect();
    // The old output stays above the marker; the new shell starts below it.
    let output = screens.find("\nBEFORE_42").unwrap_or_else(|| panic!("the saved screen comes back:\n{screens}"));
    let marker = screens.find("restored by kumo").unwrap_or_else(|| panic!("{screens}"));
    assert!(output < marker, "the marker must not overwrite the saved output:\n{screens}");
}

#[test]
fn sigterm_saves_and_cold_start_restores() {
    let root = Root::new();
    let daemon = root.start_daemon(Some(&root.0.join("ws")));
    build_layout(&root);
    root.signal(daemon, "TERM");
    assert!(root.state_file().exists(), "shutdown must leave the sessions saved");

    // After the reboot the client starts the daemon without a workspace.
    root.start_daemon(None);
    assert_restored(&root);

    // `kumo kill` ends the sessions: nothing is restored next time.
    root.ok(&["kill"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while root.state_file().exists() {
        assert!(Instant::now() < deadline, "`kumo kill` must clear the saved sessions");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn autosave_survives_a_crash() {
    let root = Root::new();
    let daemon = root.start_daemon(Some(&root.0.join("ws")));
    build_layout(&root);
    // The layout change is saved after its debounce, without any shutdown.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let saved = std::fs::read_to_string(root.state_file()).unwrap_or_default();
        if saved.matches("\"Pane\"").count() == 2 {
            break;
        }
        assert!(Instant::now() < deadline, "the split was never autosaved");
        std::thread::sleep(Duration::from_millis(100));
    }
    root.signal(daemon, "KILL");

    root.start_daemon(None);
    let panes = root.pane_ids();
    assert_eq!(panes.len(), 2, "the autosaved split comes back after a crash");
}
