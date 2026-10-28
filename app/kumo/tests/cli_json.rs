//! Golden tests for the control CLI's machine API (`--json`): reply shapes,
//! error codes, and exit codes. Agents and scripts parse this output, so a
//! change here is a contract change (see `docs/content/docs/cli.mdx`).
//!
//! Each golden file stores the *shape* of a reply: keys and value types, not
//! values (ids, pids and paths vary per run). Run with `KUMO_BLESS=1` to
//! rewrite the goldens after an intentional, policy-compliant change.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Daemon {
    child: Child,
    root: PathBuf,
}

impl Daemon {
    /// A daemon on a scratch runtime dir whose workspace is a fresh git repo
    /// (so the worktree commands have something to report).
    fn start() -> Self {
        let root = std::env::temp_dir().join(format!("kumo-cli-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&root);
        let workspace = root.join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(root.join("config"), "shell = /bin/sh\nupdate-check = false\nnew-cwd = current\n").unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["-c", "user.name=kumo", "-c", "user.email=kumo@test", "commit", "-q", "--allow-empty", "-m", "init"]] {
            let ok = Command::new("git").args(args).current_dir(&workspace).status().unwrap().success();
            assert!(ok, "git {args:?} failed");
        }
        let child = Self::env(&mut Command::new(env!("CARGO_BIN_EXE_kumo")), &root)
            .arg("daemon")
            .arg(&workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let daemon = Self { child, root };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !daemon.root.join("kumo/kumo.sock").exists() {
            assert!(Instant::now() < deadline, "daemon did not become ready");
            std::thread::sleep(Duration::from_millis(20));
        }
        daemon
    }

    fn env<'a>(cmd: &'a mut Command, root: &Path) -> &'a mut Command {
        cmd.env("KUMO_CONFIG_DIR", root)
            .env("XDG_RUNTIME_DIR", root)
            .env("XDG_STATE_HOME", root)
            .env("KUMO_NO_UPDATE", "1")
    }

    /// Run `kumo <args> --json`: the exit code and the parsed stdout.
    fn kumo(&self, args: &[&str]) -> (i32, Value) {
        let output = Self::env(&mut Command::new(env!("CARGO_BIN_EXE_kumo")), &self.root)
            .args(args)
            .arg("--json")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let json = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("`kumo {}` printed invalid JSON ({e}): {stdout}", args.join(" ")));
        (output.status.code().unwrap_or(-1), json)
    }

    /// Run a command that must succeed and return its JSON reply.
    fn ok(&self, args: &[&str]) -> Value {
        let (code, json) = self.kumo(args);
        assert_eq!(code, 0, "`kumo {}` failed: {json}", args.join(" "));
        json
    }

    /// Run a command that must fail with `code` and exit status 1.
    fn fails_with(&self, args: &[&str], code: &str) {
        let (status, json) = self.kumo(args);
        assert_eq!(status, 1, "`kumo {}` must exit 1: {json}", args.join(" "));
        assert_eq!(json["ok"], Value::Bool(false), "{json}");
        assert_eq!(json["error"]["code"], Value::String(code.into()), "`kumo {}`: {json}", args.join(" "));
        assert!(json["error"]["message"].as_str().is_some_and(|m| !m.is_empty()), "{json}");
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Reduce a reply to its shape: object keys are kept, scalars become their
/// type name, and an array becomes the shape of its first element.
fn shape(value: &Value) -> Value {
    match value {
        Value::Null => Value::Null,
        Value::Bool(_) => Value::String("<bool>".into()),
        Value::Number(_) => Value::String("<number>".into()),
        Value::String(_) => Value::String("<string>".into()),
        Value::Array(items) => Value::Array(items.first().map(shape).into_iter().collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), shape(v))).collect()),
    }
}

fn assert_golden(name: &str, reply: &Value) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/cli").join(format!("{name}.json"));
    let actual = shape(reply);
    if std::env::var_os("KUMO_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
        return;
    }
    let golden: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("missing golden {} ({e}); run with KUMO_BLESS=1 to create it", path.display())
    }))
    .unwrap();
    assert_eq!(
        actual,
        golden,
        "`{name}` JSON shape changed — a CLI contract change; update {} with KUMO_BLESS=1 only if the compatibility policy allows it",
        path.display()
    );
}

fn first_pane(daemon: &Daemon) -> u64 {
    daemon.ok(&["pane", "list"])["tabs"][0]["panes"][0]["id"].as_u64().unwrap()
}

#[test]
fn json_reply_shapes_are_stable() {
    let daemon = Daemon::start();
    let pane = first_pane(&daemon).to_string();

    assert_golden("session_list", &daemon.ok(&["session", "list"]));
    assert_golden("pane_list", &daemon.ok(&["pane", "list"]));
    assert_golden("tab_list", &daemon.ok(&["tab", "list"]));
    assert_golden("reply", &daemon.ok(&["pane", "split"]));
    assert_golden("agent_explain", &daemon.ok(&["agent", "explain", "-p", &pane]));
    assert_golden("worktree_list", &daemon.ok(&["worktree", "list"]));
    assert_golden("worktree_current", &daemon.ok(&["worktree", "current"]));

    daemon.ok(&["pane", "send-keys", "-p", &pane, "echo", "Space", "KUMO_GOLDEN", "Enter"]);
    assert_golden("pane_wait_output", &daemon.ok(&["pane", "wait-output", &pane, "KUMO_GOLDEN", "--timeout", "10s"]));
    assert_golden("agent_read", &daemon.ok(&["agent", "read", &pane]));

    daemon.ok(&["agent", "spawn", "cat"]);
    let agents = daemon.ok(&["agent", "status"]);
    assert!(!agents["agents"].as_array().unwrap().is_empty(), "the spawned agent is listed: {agents}");
    assert_golden("agent_status", &agents);

    assert_golden("kill", &daemon.ok(&["kill"]));
}

#[test]
fn errors_have_stable_codes_and_exit_status() {
    let daemon = Daemon::start();
    let pane = first_pane(&daemon).to_string();

    daemon.fails_with(&["session", "kill", "no-such-session"], "session_not_found");
    daemon.fails_with(&["pane", "list", "-s", "no-such-session"], "session_not_found");
    daemon.fails_with(&["pane", "focus", "-p", "999999"], "pane_not_found");
    daemon.fails_with(&["agent", "explain", "-p", "999999"], "pane_not_found");
    daemon.fails_with(&["agent", "wait", "999999", "--until", "idle", "--timeout", "1s"], "pane_not_found");
    daemon.fails_with(&["agent", "rename", &pane, "worker"], "not_an_agent");
    daemon.fails_with(&["agent", "start", "--kind", "not-a-loaded-agent", "--pane", &pane], "invalid_argument");
    daemon.fails_with(&["pane", "wait-output", &pane, "NEVER_PRINTED", "--timeout", "1s"], "timeout");

    let (status, json) = daemon.kumo(&["pane", "split", "-s", "no-such-session"]);
    assert_eq!((status, &json["error"]["code"]), (1, &Value::String("session_not_found".into())), "{json}");
    assert_golden("error", &json);

    daemon.ok(&["kill"]);
}
