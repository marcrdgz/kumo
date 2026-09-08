use std::collections::HashSet;
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct UiState {
    #[serde(default)]
    pub collapsed_projects: HashSet<String>,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct StoredUiState {
    version: u32,
    #[serde(default)]
    collapsed_projects: HashSet<String>,
}

fn file() -> PathBuf {
    crate::config::state_dir().join("ui-state.json")
}

pub fn canonical_project_key(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("/"))
                    .join(path)
            }
        })
        .to_string_lossy()
        .into_owned()
}

pub fn load() -> UiState {
    let Ok(bytes) = std::fs::read(file()) else {
        return UiState::default();
    };
    let Ok(stored) = serde_json::from_slice::<StoredUiState>(&bytes) else {
        return UiState::default();
    };
    if stored.version != VERSION {
        return UiState::default();
    }
    UiState {
        collapsed_projects: stored.collapsed_projects,
    }
}

pub fn save(state: &UiState) -> Result<(), String> {
    let path = file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(&StoredUiState {
        version: VERSION,
        collapsed_projects: state.collapsed_projects.clone(),
    })
    .map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

pub fn set_project_collapsed(project: &Path, collapsed: bool) -> Result<UiState, String> {
    let mut state = load();
    let key = canonical_project_key(project);
    if collapsed {
        state.collapsed_projects.insert(key);
    } else {
        state.collapsed_projects.remove(&key);
    }
    save(&state)?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_key_keeps_absolute_fallback() {
        assert_eq!(canonical_project_key(Path::new("/does/not/exist")), "/does/not/exist");
    }
}
