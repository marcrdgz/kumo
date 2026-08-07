use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EditorContext {
    pub editor: String,
    pub file: Option<String>,
}

/// Locate the editor (vim/nvim) running inside the pane's PTY and extract
/// the file path from its command line. The pane child is the shell, so we
/// walk the process tree looking for a vim/nvim descendant.
pub fn editor_context(child_pid: u32) -> Option<EditorContext> {
    let out = Command::new("ps")
        .args(["-axo", "pid=,ppid=,args="])
        .output()
        .ok()?;
    let table = String::from_utf8_lossy(&out.stdout);
    let procs = parse_ps(&table);
    let (args, pid) = find_editor_process(&procs, child_pid)?;
    let file = extract_file(&args)?;
    let file = Some(resolve_cwd(pid, file));
    Some(EditorContext {
        editor: editor_name(&args).to_string(),
        file,
    })
}

/// `ps -axo pid=,ppid=,args=` -> pid -> (ppid, args). Missing/invalid rows
/// are skipped.
fn parse_ps(output: &str) -> HashMap<u32, (u32, String)> {
    let mut procs = HashMap::new();
    for line in output.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (fields.next(), fields.next()) else {
            continue;
        };
        let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) else {
            continue;
        };
        let args = fields.collect::<Vec<_>>().join(" ");
        procs.insert(pid, (ppid, args));
    }
    procs
}

/// Walk descendants of `root` and return (args, pid) of the first vim/nvim
/// found. Search order is by pid (approx process spawn order).
fn find_editor_process(procs: &HashMap<u32, (u32, String)>, root: u32) -> Option<(String, u32)> {
    let mut stack: Vec<u32> = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some((_, args)) = procs.get(&pid) {
            if is_editor(args) {
                return Some((args.clone(), pid));
            }
        }
        for (child, (parent, _)) in procs {
            if *parent == pid {
                stack.push(*child);
            }
        }
    }
    None
}

fn is_editor(args: &str) -> bool {
    matches!(editor_name(args), "vim" | "vim.basic" | "vim.tiny" | "nvim" | "nvi")
}

fn editor_name(args: &str) -> &str {
    args.split_whitespace()
        .next()
        .and_then(|a| std::path::Path::new(a).file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("")
}

/// First non-option argument = the file being edited. Skips flags and the
/// values consumed by flags that take an argument (`-u NONE`, `--listen sock`).
fn extract_file(args: &str) -> Option<String> {
    let mut skip_value = false;
    for token in args.split_whitespace().skip(1) {
        let is_flag = token.starts_with('-') || token.starts_with('+');
        if is_flag {
            skip_value = FLAGS_WITH_VALUE.contains(&token);
            continue;
        }
        if skip_value {
            skip_value = false;
            continue;
        }
        return Some(token.to_string());
    }
    None
}

const FLAGS_WITH_VALUE: &[&str] = &[
    "-u", "-c", "-S", "-i", "-o", "-O", "-p", "-t", "-q", "-V", "-w", "-W",
    "--cmd", "--listen", "--server", "--servername", "--remote-silent",
    "--remote-expr", "--remote", "--windowid", "--data-dir", "--config",
    "--model",
];

/// Resolve a relative path against the editor process's working directory
/// (best effort via `lsof`; unavailable on Linux without lsof).
fn resolve_cwd(pid: u32, file: String) -> String {
    if std::path::Path::new(&file).is_absolute() {
        return file;
    }
    if let Some(cwd) = process_cwd(pid) {
        return cwd.join(&file).to_string_lossy().to_string();
    }
    file
}

fn process_cwd(pid: u32) -> Option<PathBuf> {
    let out = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if let Some(path) = line.strip_prefix('n') {
            return Some(PathBuf::from(path));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ps_rows() {
        let table = "  1     0 /sbin/launchd\n  42     1 -fish\n 100    42 /usr/bin/vim src/main.rs\n";
        let procs = parse_ps(table);
        assert_eq!(procs.len(), 3);
        assert_eq!(procs[&42].0, 1);
        assert_eq!(procs[&100].1, "/usr/bin/vim src/main.rs");
    }

    #[test]
    fn finds_grandchild_editor() {
        let table = "  1     0 /sbin/launchd\n  42     1 -fish\n 100    42 /usr/bin/vim src/main.rs\n";
        let procs = parse_ps(table);
        let (args, pid) = find_editor_process(&procs, 42).expect("editor");
        assert_eq!(pid, 100);
        assert!(args.contains("main.rs"));
    }

    #[test]
    fn does_not_match_shell() {
        let table = "  42     1 -fish\n 100    42 ls -la\n";
        let procs = parse_ps(table);
        assert!(find_editor_process(&procs, 42).is_none());
    }

    #[test]
    fn matches_root_editor() {
        let table = "  42     1 nvim --clean\n";
        let procs = parse_ps(table);
        assert!(find_editor_process(&procs, 42).is_some());
    }

    #[test]
    fn skips_flags_in_file_extract() {
        assert_eq!(
            extract_file("vim -u NONE -n src/main.rs").as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            extract_file("nvim --clean /tmp/a.txt").as_deref(),
            Some("/tmp/a.txt")
        );
        assert_eq!(
            extract_file("nvim --listen /tmp/sock /tmp/a.txt").as_deref(),
            Some("/tmp/a.txt")
        );
        assert_eq!(
            extract_file("vim +10 src/main.rs").as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(extract_file("vim -c q"), None);
    }
}
