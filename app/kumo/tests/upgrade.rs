//! Live upgrade from a released kumo: the released daemon builds sessions,
//! `kumo server restart` makes it exec this build (as `kumo update` does after
//! swapping the binary), and this build must adopt the live panes.
//!
//! Needs a released binary, so it is opt-in (run before every release):
//!
//! ```sh
//! KUMO_PREV_BIN="$(command -v kumo)" cargo test -p kumo --test upgrade -- --ignored
//! ```
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

struct Root(PathBuf);

impl Drop for Root {
    fn drop(&mut self) {
        let _ = kumo(&self.0, &self.0.join("bin/kumo-released"), &["kill"]);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn kumo(root: &Path, bin: &Path, args: &[&str]) -> (bool, Value) {
    let output = Command::new(bin)
        .args(args)
        .arg("--json")
        .env("KUMO_CONFIG_DIR", root)
        .env("XDG_RUNTIME_DIR", root)
        .env("XDG_STATE_HOME", root)
        .env("KUMO_NO_UPDATE", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let json = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    (output.status.success(), json)
}

fn ok(root: &Path, bin: &Path, args: &[&str]) -> Value {
    let (success, json) = kumo(root, bin, args);
    assert!(success, "`kumo {}` failed: {json}", args.join(" "));
    json
}

#[test]
#[ignore = "needs a released kumo binary in KUMO_PREV_BIN"]
fn update_from_previous_release_keeps_live_sessions() {
    let released = PathBuf::from(std::env::var("KUMO_PREV_BIN").expect("set KUMO_PREV_BIN to a released kumo"));
    let root = Root(std::env::temp_dir().join(format!("kumo-upgrade-{}", std::process::id())));
    let root_dir = root.0.clone();
    let _ = std::fs::remove_dir_all(&root_dir);
    std::fs::create_dir_all(root_dir.join("bin")).unwrap();
    std::fs::create_dir_all(root_dir.join("ws")).unwrap();
    std::fs::write(root_dir.join("config"), "shell = /bin/sh\nupdate-check = false\nnew-cwd = current\n").unwrap();
    let old = root_dir.join("bin/kumo-released");
    std::fs::copy(&released, &old).unwrap();
    // The daemon re-execs its own path on restart: start it from `bin/kumo`,
    // then point that path at this build, as `kumo update` swaps the binary.
    let exe = root_dir.join("bin/kumo");
    std::fs::copy(&released, &exe).unwrap();
    Command::new(&exe)
        .arg("daemon")
        .arg(root_dir.join("ws"))
        .env("KUMO_CONFIG_DIR", &root_dir)
        .env("XDG_RUNTIME_DIR", &root_dir)
        .env("XDG_STATE_HOME", &root_dir)
        .env("KUMO_NO_UPDATE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root_dir.join("kumo/kumo.sock").exists() {
        assert!(Instant::now() < deadline, "released daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }

    let pane = ok(&root_dir, &old, &["pane", "list"])["tabs"][0]["panes"][0]["id"].as_u64().unwrap().to_string();
    ok(&root_dir, &old, &["pane", "split"]);
    ok(&root_dir, &old, &["agent", "spawn", "cat"]);
    let agent = ok(&root_dir, &old, &["agent", "status"])["agents"][0]["pane_id"].as_u64().unwrap().to_string();
    ok(&root_dir, &old, &["agent", "rename", &agent, "worker"]);
    ok(&root_dir, &old, &["tab", "new", root_dir.join("ws").to_str().unwrap(), "--name", "second"]);
    let before = ok(&root_dir, &old, &["session", "list"]);

    std::fs::remove_file(&exe).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_kumo"), &exe).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    ok(&root_dir, &old, &["server", "restart"]);

    // Talk to the restarted daemon with this build's CLI from here on.
    let new = PathBuf::from(env!("CARGO_BIN_EXE_kumo"));
    let deadline = Instant::now() + Duration::from_secs(10);
    let after = loop {
        if let (true, json) = kumo(&root_dir, &new, &["session", "list"]) {
            break json;
        }
        assert!(Instant::now() < deadline, "restarted daemon never answered");
        std::thread::sleep(Duration::from_millis(50));
    };
    let shape = |list: &Value| -> Vec<(String, usize, u64)> {
        list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| (s["name"].as_str().unwrap().to_string(), s["tab_count"].as_u64().unwrap() as usize, s["pane_count"].as_u64().unwrap()))
            .collect()
    };
    assert_eq!(shape(&after), shape(&before), "sessions changed across the upgrade");
    let alias = ok(&root_dir, &new, &["agent", "status"])["agents"][0]["name"].clone();
    assert_eq!(alias, Value::String("worker".into()), "the agent alias was lost");
    // Releases up to 0.8 named default tabs "1", "2", …; those now read as tab
    // ids, so the upgraded tab derives its name while a user's name is kept.
    let tab_names: Vec<Value> = ok(&root_dir, &new, &["tab", "list"])["tabs"].as_array().unwrap().iter().map(|t| t["name"].clone()).collect();
    assert_ne!(tab_names[0], Value::String("1".into()), "the legacy default tab name survived the upgrade");
    assert_eq!(tab_names[1], Value::String("second".into()), "the custom tab name was lost");

    // The adopted shell is the same live process: it still runs commands.
    ok(&root_dir, &new, &["pane", "send-keys", "-p", &pane, "echo", "Space", "AFTER_UPGRADE", "Enter"]);
    ok(&root_dir, &new, &["pane", "wait-output", &pane, "AFTER_UPGRADE", "--timeout", "10s"]);
}
