use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

const APP_CONFIG_FILE: &str = "settings.json";
const MAX_RECENT_LADDERS: usize = 10;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub recent_ladders: Vec<String>,
    /// The acquisition plan: which channels to shoot and how to expose each.
    ///
    /// Exposure settings are a property of the bench — this camera, in this box,
    /// with these lamps — not of a document, so they outlive the session and
    /// live in the user's settings. `None` means this user has never run the
    /// instrument, and gets the defaults.
    #[serde(default)]
    pub acquisition_plan: Option<opengel::instrument::AcquisitionPlan>,
}

pub fn load_config() -> AppConfig {
    let Ok(path) = config_path() else {
        return AppConfig::default();
    };
    let Ok(bytes) = fs::read(path) else {
        return AppConfig::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub fn save_config(config: &AppConfig) -> Result<PathBuf> {
    let path = config_path()?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("app settings path has no parent"))?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let bytes = serde_json::to_vec_pretty(config)?;
    fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

pub fn sanitize_recent_ladders(names: Vec<String>) -> Vec<String> {
    let mut recent = Vec::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() || recent.iter().any(|n| n == name) {
            continue;
        }
        if opengel::core::ladders::by_name(name).is_some() {
            recent.push(name.to_string());
        }
        if recent.len() == MAX_RECENT_LADDERS {
            break;
        }
    }
    recent
}

pub fn remember_ladder(recent: &mut Vec<String>, name: &str) {
    let name = name.trim();
    if name.is_empty() || opengel::core::ladders::by_name(name).is_none() {
        return;
    }
    recent.retain(|n| n != name);
    recent.insert(0, name.to_string());
    recent.truncate(MAX_RECENT_LADDERS);
}

fn config_path() -> Result<PathBuf> {
    let base =
        config_base_dir().ok_or_else(|| anyhow!("could not locate user config directory"))?;
    Ok(base.join("OpenGel").join(APP_CONFIG_FILE))
}

#[cfg(target_os = "windows")]
fn config_base_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn config_base_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library").join("Application Support"))
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn config_base_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}
