//! Data-driven agent-detection rules (`agent-detection/<agent>.toml`).
//!
//! The bundled manifests (in `rules/`) express the built-in classifiers —
//! claude, opencode — as TOML so third-party agents (codex, gemini, ...) get
//! accurate state **without a kumo release**: drop a `<id>.toml` into the user
//! config dir (`config_dir()/agent-detection`) and it loads on daemon start
//! and on `kumo reload`. The engine is deliberately local-only in 0.7.0.
//!
//! # Schema
//!
//! ```toml
//! [agent]
//! id = "example"            # [a-z0-9-]+, must match the filename stem
//! name = "Example Agent"    # optional display label
//!
//! # Each signal is an OR of `[[signal]]` groups; a group is an AND of its
//! # `tests`. `blocked > working > idle` precedence is applied by the
//! # dispatcher, never inside the manifest.
//! [[blocked]]
//! tests = [
//!   { region = "form", contains = "esc to cancel" },
//!   { region = "form", yes-no-line = true },
//! ]
//! # any-of: first matching branch counts (branches may be nested groups).
//! # not: succeeds when the AND of the listed entries does NOT match.
//! ```
//!
//! Leaf matchers (exactly one per test):
//! - `contains` — ASCII case-insensitive substring in the region
//! - `contains-any` — first matching pattern of the list
//! - `prefix` — region text trimmed-start starts with the string (title idle)
//! - `spinner` — `"braille"` | `"half-circle"` first char of the OSC title
//! - `spinner-line` — a trimmed line ≤16 bytes holding a dingbat glyph
//! - `btw-overlay` — the `/btw` overlay (header + `esc to close` in last lines)
//! - `yes-no-line` — a Claude yes/no option line (`1. yes`, `2. no`, `❯ yes`)
//! - `braille-in` — any braille-glyph character in the region text

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use serde::Deserialize;

use super::{contains_ci, ends_with_ci, AgentEvidence, MarkerMatch, Region, Snapshot};

// ---------------------------------------------------------------------------
// Manifests
// ---------------------------------------------------------------------------

/// Top-level shape of an `agent-detection/<agent>.toml` manifest.
#[derive(Deserialize)]
struct Manifest {
    agent: Header,
    #[serde(default)]
    blocked: Vec<GroupDef>,
    #[serde(default)]
    working: Vec<GroupDef>,
    #[serde(default)]
    idle: Vec<GroupDef>,
}

#[derive(Deserialize)]
struct Header {
    id: String,
}

/// A signal group: an AND over `tests`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupDef {
    #[serde(default)]
    tests: Vec<Entry>,
}

/// A boolean entry used inside a group: a leaf test, an `any-of` branch list,
/// a negated AND-group (`not`), or a nested AND group (`tests`).
#[derive(Deserialize)]
#[serde(untagged)]
enum Entry {
    Test(Test),
    AnyOf {
        #[serde(rename = "any-of")]
        any_of: Vec<Entry>,
    },
    Not {
        #[serde(rename = "not")]
        not: Vec<Entry>,
    },
    Group {
        tests: Vec<Entry>,
    },
}

/// A leaf matcher, bound to one evidence region.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Test {
    region: String,
    #[serde(default)]
    contains: Option<String>,
    #[serde(default, rename = "contains-any")]
    contains_any: Option<Vec<String>>,
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    spinner: Option<String>,
    #[serde(default, rename = "spinner-line")]
    spinner_line: Option<bool>,
    #[serde(default, rename = "btw-overlay")]
    btw_overlay: Option<bool>,
    #[serde(default, rename = "yes-no-line")]
    yes_no_line: Option<bool>,
    #[serde(default, rename = "braille-in")]
    braille_in: Option<bool>,
}

// ---------------------------------------------------------------------------
// Runtime rules
// ---------------------------------------------------------------------------

/// The active rule set: one `AgentRules` per detected agent.
#[derive(Debug, Clone, Default)]
pub(crate) struct Rules {
    pub(crate) agents: Vec<AgentRules>,
}

/// Evaluation group for one signal; a copy of `GroupDef` with owned strings.
#[derive(Debug, Clone)]
struct Group(Vec<EntryOwned>);

#[derive(Debug, Clone)]
enum EntryOwned {
    Test(TestOwned),
    AnyOf(Vec<EntryOwned>),
    Not(Vec<EntryOwned>),
    Group(Vec<EntryOwned>),
}

#[derive(Debug, Clone)]
struct TestOwned {
    region: Region,
    matcher: Matcher,
}

#[derive(Debug, Clone)]
enum Matcher {
    Contains(String),
    ContainsAny(Vec<String>),
    Prefix(String),
    Spinner(SpinnerKind),
    SpinnerLine,
    BtwOverlay,
    YesNoLine,
    BrailleIn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpinnerKind {
    Braille,
    HalfCircle,
}

/// Classification rules of one agent (`<id>.toml`).
#[derive(Debug, Clone)]
pub(crate) struct AgentRules {
    pub(crate) id: String,
    blocked: Vec<Group>,
    working: Vec<Group>,
    idle: Vec<Group>,
}

impl AgentRules {
    pub(crate) fn blocked(&self, snap: &Snapshot) -> bool {
        signal_matches(&self.blocked, snap)
    }

    pub(crate) fn working(&self, snap: &Snapshot) -> bool {
        signal_matches(&self.working, snap)
    }

    pub(crate) fn idle(&self, snap: &Snapshot) -> bool {
        signal_matches(&self.idle, snap)
    }

    pub(crate) fn evidence(&self, snap: &Snapshot) -> AgentEvidence {
        AgentEvidence {
            agent: self.id.clone(),
            blocked: signal_evidence(&self.blocked, snap),
            working: signal_evidence(&self.working, snap),
            idle: signal_evidence(&self.idle, snap),
        }
    }
}

fn signal_matches(groups: &[Group], snap: &Snapshot) -> bool {
    groups.iter().any(|g| group_match(g, snap).is_some())
}

fn signal_evidence(groups: &[Group], snap: &Snapshot) -> Vec<MarkerMatch> {
    let mut out = Vec::new();
    for g in groups {
        if let Some(mut markers) = group_match(g, snap) {
            out.append(&mut markers);
        }
    }
    out
}

/// A group matches when every entry matches; returns the joined evidence of
/// every entry, in declaration order.
fn group_match(group: &Group, snap: &Snapshot) -> Option<Vec<MarkerMatch>> {
    let mut out = Vec::new();
    for entry in &group.0 {
        match entry_match(entry, snap) {
            Ok(mut markers) => out.append(&mut markers),
            Err(()) => return None,
        }
    }
    Some(out)
}

/// `Ok(evidence)` when the entry matches; `Err(())` when it does not.
fn entry_match(entry: &EntryOwned, snap: &Snapshot) -> Result<Vec<MarkerMatch>, ()> {
    match entry {
        EntryOwned::Test(t) => test_match(t, snap).map(|m| vec![m]).ok_or(()),
        EntryOwned::AnyOf(branches) => {
            for b in branches {
                if let Ok(markers) = entry_match(b, snap) {
                    return Ok(markers);
                }
            }
            Err(())
        }
        EntryOwned::Group(tests) => {
            let group = Group(tests.clone());
            group_match(&group, snap).ok_or(())
        }
        EntryOwned::Not(tests) => {
            // `not` holds the AND of its entries: it succeeds when that AND
            // does NOT match.
            let group = Group(tests.clone());
            if group_match(&group, snap).is_some() {
                Err(())
            } else {
                Ok(Vec::new())
            }
        }
    }
}

impl TestOwned {
    fn match_marker(&self, snap: &Snapshot) -> Option<MarkerMatch> {
        let text = match self.region {
            Region::Screen => &snap.screen,
            Region::Form => &snap.form,
            Region::Footer => &snap.footer,
            Region::Title => &snap.title,
        };
        let marker: Option<String> = match &self.matcher {
            Matcher::Contains(p) => contains_ci(text, p).then(|| p.clone()),
            Matcher::ContainsAny(ps) => ps.iter().find(|p| contains_ci(text, p)).map(|p| (*p).clone()),
            Matcher::Prefix(p) => text.trim_start().starts_with(p).then(|| format!("{p} idle title")),
            Matcher::Spinner(k) => title_first_spinner(text, *k).map(|n| n.to_string()),
            Matcher::SpinnerLine => spinner_line_in(text).then(|| "dingbat spinner".to_string()),
            Matcher::BtwOverlay => btw_overlay(text).then(|| "/btw overlay".to_string()),
            Matcher::YesNoLine => text.lines().any(yes_no_line).then(|| "yes/no options".to_string()),
            Matcher::BrailleIn => text.chars().any(|c| ('\u{2800}'..='\u{28ff}').contains(&c))
                .then(|| "braille spinner".to_string()),
        };
        marker.map(|marker| MarkerMatch { marker, region: self.region })
    }
}

fn test_match(t: &TestOwned, snap: &Snapshot) -> Option<MarkerMatch> {
    t.match_marker(snap)
}

/// Whether the region text starts with a spinner glyph of the given kind
/// (applies to the OSC title region).
fn title_first_spinner(text: &str, kind: SpinnerKind) -> Option<&'static str> {
    let c = text.chars().next()?;
    let ok = match kind {
        SpinnerKind::Braille => ('\u{2800}'..='\u{28ff}').contains(&c),
        SpinnerKind::HalfCircle => ('\u{25d0}'..='\u{25d3}').contains(&c),
    };
    ok.then_some(match kind {
        SpinnerKind::Braille => "title spinner (braille)",
        SpinnerKind::HalfCircle => "title spinner (half-circle)",
    })
}

/// Dingbat spinner glyphs Claude paints in its prompt box while working.
const DINGBAT_SPINNER: &[char] = &[
    '\u{2722}', // ✢
    '\u{2736}', // ✶
    '\u{273b}', // ✻
    '\u{273d}', // ✽
];

/// Max trimmed line length for a dingbat to count as an active spinner.
const DINGBAT_LINE_MAX_LEN: usize = 16;

/// True when `text` contains a dingbat on a short trimmed line (≤16 bytes).
/// Longer lines (`✻ Sautéed for 43s`) are completion summaries, not spinners.
fn spinner_line_in(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.len() <= DINGBAT_LINE_MAX_LEN
            && DINGBAT_SPINNER.iter().any(|c| trimmed.contains(*c))
    })
}

/// True when Claude's `/btw` reasoning overlay is on screen: within the last
/// five non-empty lines a header starts with `/btw` and another ends with
/// `esc to close`.
fn btw_overlay(screen: &str) -> bool {
    let tail: Vec<&str> = screen
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .rev()
        .take(5)
        .collect();
    let has_btw = tail.iter().any(|l| {
        l.trim_start().starts_with("/btw")
            && l.trim_start()[4..].chars().next().is_none_or(char::is_whitespace)
    });
    let has_close = tail.iter().any(|l| ends_with_ci(l, "esc to close"));
    has_btw && has_close
}

/// Whether `line` is a yes/no option like `1. yes` / `2. no` (Claude forms).
pub(crate) fn yes_no_line(line: &str) -> bool {
    let mut t = line.trim_start();
    if let Some(rest) = t.strip_prefix('\u{276f}') {
        t = rest.trim_start();
    }
    let (num, rest) = match t.split_once('.') {
        Some((n, r)) => (n.trim(), r),
        None => ("", t),
    };
    let word = rest
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase();
    let yes = word == "yes";
    let no = word == "no";
    (num.is_empty() && yes)
        || (num == "1" && yes)
        || (num == "2" && (yes || no))
        || (num == "3" && no)
}

// ---------------------------------------------------------------------------
// Compile: manifest -> AgentRules (with validation)
// ---------------------------------------------------------------------------

struct ManifestError(pub(crate) String);

impl std::fmt::Debug for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ManifestError").field(&self.0).finish()
    }
}

fn compile(man: Manifest, stem: Option<&str>) -> Result<AgentRules, ManifestError> {
    let id = man.agent.id.clone();
    if id.is_empty() {
        return Err(ManifestError("no [agent] id".into()));
    }
    let chips: Vec<&str> = id.split('-').collect();
    if id.chars().any(|p| !(p.is_ascii_alphanumeric() || p == '-'))
        || chips.iter().all(|p| p.is_empty())
    {
        return Err(ManifestError(format!("invalid agent id {id:?} (allow [a-z0-9-]+)")));
    }
    if let Some(stem) = stem {
        if stem != id {
            return Err(ManifestError(format!(
                "agent id {id:?} does not match the filename stem {stem:?}"
            )));
        }
    }
    let blocked = compile_groups(man.blocked)?;
    let working = compile_groups(man.working)?;
    let idle = compile_groups(man.idle)?;
    Ok(AgentRules { id, blocked, working, idle })
}

fn compile_groups(groups: Vec<GroupDef>) -> Result<Vec<Group>, ManifestError> {
    groups.into_iter().map(compile_group).collect()
}

fn compile_group(g: GroupDef) -> Result<Group, ManifestError> {
    Ok(Group(compile_entries(g.tests)?))
}

fn compile_entries(entries: Vec<Entry>) -> Result<Vec<EntryOwned>, ManifestError> {
    entries.into_iter().map(compile_entry).collect()
}

fn compile_entry(entry: Entry) -> Result<EntryOwned, ManifestError> {
    match entry {
        Entry::Test(t) => {
            let region = match t.region.as_str() {
                "screen" => Region::Screen,
                "form" => Region::Form,
                "footer" => Region::Footer,
                "title" => Region::Title,
                other => return Err(ManifestError(format!("unknown region {other:?}"))),
            };
            let mut matchers = Vec::new();
            if let Some(p) = t.contains {
                matchers.push(Matcher::Contains(p));
            }
            if let Some(ps) = t.contains_any {
                matchers.push(Matcher::ContainsAny(ps));
            }
            if let Some(p) = t.prefix {
                matchers.push(Matcher::Prefix(p));
            }
            if let Some(s) = t.spinner {
                let kind = match s.as_str() {
                    "braille" => SpinnerKind::Braille,
                    "half-circle" => SpinnerKind::HalfCircle,
                    other => return Err(ManifestError(format!("unknown spinner kind {other:?}"))),
                };
                matchers.push(Matcher::Spinner(kind));
            }
            if t.spinner_line.unwrap_or(false) {
                matchers.push(Matcher::SpinnerLine);
            }
            if t.btw_overlay.unwrap_or(false) {
                matchers.push(Matcher::BtwOverlay);
            }
            if t.yes_no_line.unwrap_or(false) {
                matchers.push(Matcher::YesNoLine);
            }
            if t.braille_in.unwrap_or(false) {
                matchers.push(Matcher::BrailleIn);
            }
            if matchers.len() != 1 {
                return Err(ManifestError(format!(
                    "test on region {:?} must use exactly one matcher",
                    t.region
                )));
            }
            Ok(EntryOwned::Test(TestOwned {
                region,
                matcher: matchers.pop().expect("exactly one matcher"),
            }))
        }
        Entry::AnyOf { any_of } => Ok(EntryOwned::AnyOf(compile_entries(any_of)?)),
        Entry::Not { not } => Ok(EntryOwned::Not(compile_entries(not)?)),
        Entry::Group { tests } => Ok(EntryOwned::Group(compile_entries(tests)?)),
    }
}

// ---------------------------------------------------------------------------
// Loading: bundled (+ user-dir overrides)
// ---------------------------------------------------------------------------

/// Bundled manifests, compiled into the binary via `include_str!`.
const BUNDLED: &[(&str, &str)] = &[
    (
        "claude",
        include_str!("rules/claude.toml"),
    ),
    (
        "opencode",
        include_str!("rules/opencode.toml"),
    ),
];

/// Parse the bundled manifests. They are compile-time constants, so a parse
/// failure is a programming error caught by tests (and panics on boot).
fn bundled_rules() -> Rules {
    let mut rules = Rules::default();
    for (id, src) in BUNDLED {
        let man = toml::from_str::<Manifest>(src)
            .unwrap_or_else(|e| panic!("bundled rules for {id} are invalid: {e}"));
        match compile(man, Some(id)) {
            Ok(agent) => rules.agents.push(agent),
            Err(e) => panic!("bundled rules for {id} failed validation: {}", e.0),
        }
    }
    rules
}

/// The user-dir override directory: `config_dir()/agent-detection`.
fn user_rules_dir() -> PathBuf {
    kumo_core::config::config_dir().join("agent-detection")
}

/// Load the bundled rules, then overlay every `<id>.toml` in the user dir
/// (a user file REPLACES the bundled rules of the same agent). Invalid files
/// are skipped with a warning — detection never crashes and falls back to the
/// bundled defaults.
pub(crate) fn load_rules() -> Rules {
    load_rules_from(&user_rules_dir())
}

/// `load_rules` against an explicit directory (tests use a temp dir).
pub(crate) fn load_rules_from(dir: &Path) -> Rules {
    let mut rules = bundled_rules();
    let mut entries: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ex| ex == "toml"))
            .map(|e| e.path())
            .collect(),
        Err(_) => return rules,
    };
    entries.sort();
    for path in entries {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let src = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("kumo: agent rules {path:?} unreadable: {e}, skipping");
                continue;
            }
        };
        match toml::from_str::<Manifest>(&src).map_err(|e| e.to_string()).and_then(
            |man| compile(man, Some(&stem)).map_err(|e| e.0),
        ) {
            Ok(agent) => {
                if let Some(existing) = rules.agents.iter_mut().find(|a| a.id == agent.id) {
                    *existing = agent;
                } else {
                    rules.agents.push(agent);
                }
            }
            Err(e) => log::warn!("kumo: agent rules {path:?} rejected: {e}, skipping"),
        }
    }
    rules
}

/// Active rule set. Bundled rules are the lazy default; `reload_rules` swaps
/// in the user-dir overlay on daemon start and on `kumo reload`.
static RULES: LazyLock<RwLock<Rules>> = LazyLock::new(|| RwLock::new(bundled_rules()));

/// Re-read the user-dir overrides. Called once at daemon startup and from
/// `reload_config`; bundled defaults are retained when a user file is invalid.
pub(crate) fn reload_rules() {
    *RULES.write().expect("rules lock poisoned") = load_rules();
}

/// Read-only view of the active rules.
pub(crate) fn with_rules<T>(f: impl FnOnce(&Rules) -> T) -> T {
    let rules = RULES.read().expect("rules lock poisoned");
    f(&rules)
}

/// The rules of one agent (bundled, overridable) — helper for the per-agent
/// wrappers (claude.rs/opencode.rs), which are exercise/test surfaces.
#[cfg(test)]
pub(crate) fn agent(id: &str) -> AgentRules {
    with_rules(|r| {
        r.agents
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .unwrap_or_else(|| panic!("agent rules for {id:?} not loaded"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::agents::after_last_rule;

    fn snap(screen: &str, footer: &str, title: &str) -> Snapshot {
        let form = after_last_rule(screen);
        Snapshot {
            screen: screen.to_string(),
            form,
            footer: footer.to_string(),
            title: title.to_string(),
        }
    }

    fn agent_from(src: &str, stem: &str) -> Result<AgentRules, ManifestError> {
        let man = toml::from_str::<Manifest>(src).map_err(|e| ManifestError(e.to_string()))?;
        compile(man, Some(stem))
    }

    #[test]
    fn bundled_manifests_parse_and_validate() {
        for (id, src) in BUNDLED {
            let man = toml::from_str::<Manifest>(src).expect("bundled toml parses");
            compile(man, Some(id)).expect("bundled manifest validates");
        }
    }

    #[test]
    fn claude_question_dialog_blocks_and_evidences() {
        let rules = agent_from(
            r#"
            [agent]
            id = "claude"
            [[blocked]]
            tests = [
              { region = "form", contains = "esc to cancel" },
              { any-of = [
                  { region = "form", contains = "enter to confirm" },
                  { tests = [
                      { region = "form", contains = "enter to select" },
                      { any-of = [
                          { region = "form", contains = "tab/arrow keys to navigate" },
                          { region = "form", contains = "arrow keys to navigate" },
                          { region = "form", contains = "arrows to navigate" },
                          { region = "form", contains = "↑/↓ to navigate" },
                          { region = "form", contains = "↑↓ to navigate" },
                      ]},
                  ]},
              ]},
            ]
            "#,
            "claude",
        )
        .unwrap();
        let s = snap(
            "─────\nRun a dynamic workflow?\n  enter to confirm\n  esc to cancel\n",
            "",
            "",
        );
        assert!(rules.blocked(&s), "live form must block");
        let ev = rules.evidence(&s);
        assert!(ev.blocked.len() >= 2);
        assert!(ev
            .blocked
            .iter()
            .any(|m| m.marker == "enter to confirm" && m.region == Region::Form));
    }

    #[test]
    fn not_negation_matches_open_code_idle_guard() {
        let rules = agent_from(
            r#"
            [agent]
            id = "oc"
            [[idle]]
            tests = [
              { region = "footer", contains = "esc dismiss" },
              { not = [
                  { any-of = [
                      { region = "screen", contains = "enter submit" },
                      { region = "screen", contains = "enter confirm" },
                  ]},
                  { any-of = [
                      { region = "screen", contains = "↑↓ select" },
                      { region = "screen", contains = "⇆ tab" },
                  ]},
              ]},
            ]
            "#,
            "oc",
        )
        .unwrap();
        // Idle prompt: footer has esc dismiss, no dialog chrome → idle.
        let idle_snap = snap("done\n", "esc dismiss", "");
        assert!(rules.idle(&idle_snap));
        // Question dialog on screen (enter + nav) → the NOT fails → not idle.
        let dialog = snap(
            "⇆ tab   ↑↓ select\nenter submit   esc dismiss\n",
            "esc dismiss",
            "",
        );
        assert!(!rules.idle(&dialog), "question dialog must not read idle");
    }

    #[test]
    fn spinner_matchers_cover_braille_and_half_circle() {
        let rules = agent_from(
            r#"
            [agent]
            id = "c"
            [[working]]
            tests = [{ region = "title", spinner = "braille" }]
            [[working]]
            tests = [{ region = "title", spinner = "half-circle" }]
            "#,
            "c",
        )
        .unwrap();
        assert!(rules.working(&snap("", "", "\u{280b} Fixing")));
        assert!(rules.working(&snap("", "", "\u{25d0} Busy")));
        assert!(!rules.working(&snap("", "", "\u{2733} idle")));
    }

    #[test]
    fn validation_rejects_bad_ids_and_ambiguous_tests() {
        let bad_id = agent_from("[agent]\nid = \"bad.id!\"\n", "bad.id!");
        assert!(bad_id.is_err(), "id charset must validate");

        let stem_mismatch = agent_from("[agent]\nid = \"one\"\n", "twokum");
        assert!(stem_mismatch.is_err(), "id must match the filename stem");

        let two_matchers = agent_from(
            "[agent]\nid = \"x\"\n[[working]]\ntests = [{ region = \"title\", contains = \"a\", prefix = \"b\" }]\n",
            "x",
        );
        assert!(two_matchers.is_err(), "exactly one matcher per test");

        let bad_region = agent_from(
            "[agent]\nid = \"x\"\n[[working]]\ntests = [{ region = \"scrollback\", contains = \"a\" }]\n",
            "x",
        );
        assert!(bad_region.is_err(), "unknown region rejected");
    }

    #[test]
    fn load_rules_overlays_user_dir() {
        let dir = std::env::temp_dir().join(format!("kumo_rules_test_{}", std::process::id()));
        let rules_dir = dir.join("agent-detection");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(
            rules_dir.join("personal.toml"),
            "[agent]\nid = \"personal\"\n[[idle]]\ntests = [{ region = \"screen\", contains = \"my-idle-marker\" }]\n",
        )
        .unwrap();
        std::fs::write(
            rules_dir.join("broken.toml"),
            "[agent]\nid = \"broken!!\"\n",
        )
        .unwrap();

        let rules = load_rules_from(&rules_dir);
        // Bundled retained...
        assert!(rules.agents.iter().any(|a| a.id == "claude"));
        // ...user agent appended...
        assert!(rules.agents.iter().any(|a| a.id == "personal"));
        // ...invalid user file skipped without panicking.
        assert!(!rules.agents.iter().any(|a| a.id == "broken!!"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn match_marker_kinds_render_evidence_names() {
        let rules = agent_from(
            r#"
            [agent]
            id = "special"
            [[working]]
            tests = [{ region = "screen", btw-overlay = true }]
            [[idle]]
            tests = [{ region = "title", prefix = "✳" }]
            "#,
            "special",
        )
        .unwrap();
        let s = snap("/btw reasoning\n  esc to close\n", "", "");
        assert!(rules.working(&s));
        assert!(rules
            .evidence(&s)
            .working
            .iter()
            .any(|m| m.marker == "/btw overlay" && m.region == Region::Screen));
        let s = snap("", "", "\u{2733} ~/proj");
        assert!(rules.idle(&s));
        assert!(rules
            .evidence(&s)
            .idle
            .iter()
            .any(|m| m.marker == "✳ idle title" && m.region == Region::Title));
    }
}
