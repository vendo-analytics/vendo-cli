//! `~/.config/vendo/config.json` — same file, same shape as the TypeScript CLI,
//! so both binaries can run side by side against one config.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const DEFAULT_BASE_URL: &str = "https://app2.vendodata.com";
pub const STAGING_BASE_URL: &str = "https://stg.vendodata.com";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigFile {
    // Legacy flat fields (pre-profiles): read once, folded into a profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profiles: Option<IndexMap<String, Profile>>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Env,
    Profile,
    Default,
    Missing,
}

#[derive(Debug, Clone)]
pub struct Effective {
    pub selected_profile: Option<String>,
    pub api_key: Option<String>,
    pub api_key_source: Source,
    pub base_url: String,
    pub base_url_source: Source,
    pub account_id: Option<String>,
    pub account_id_source: Source,
}

impl Effective {
    pub fn env_override_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.api_key_source == Source::Env {
            names.push("VENDO_API_KEY");
        }
        if self.base_url_source == Source::Env {
            names.push("VENDO_API_URL");
        }
        if self.account_id_source == Source::Env {
            names.push("VENDO_ACCOUNT_ID");
        }
        names
    }
}

pub struct ProfileSummary {
    pub name: String,
    pub active: bool,
    pub account_id: Option<String>,
    pub base_url: String,
}

pub struct Config {
    path: PathBuf,
    file: ConfigFile,
    profile_override: Option<String>,
}

fn config_path() -> PathBuf {
    std::env::home_dir()
        .unwrap_or_default()
        .join(".config")
        .join("vendo")
        .join("config.json")
}

impl Config {
    pub fn load(profile_override: Option<String>) -> Self {
        let path = config_path();
        let file: ConfigFile = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        let (file, migrated) = migrate_legacy(file);
        let config = Config { path, file, profile_override };
        if migrated {
            // Best-effort, like the TS CLI: a read-only config dir must not fail reads.
            let _ = config.write();
        }
        config
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn selected_profile_name(&self) -> Option<&str> {
        self.profile_override
            .as_deref()
            .or(self.file.active_profile.as_deref())
    }

    pub fn effective(&self) -> Effective {
        let selected = self.selected_profile_name().map(str::to_string);
        let profile = selected
            .as_ref()
            .and_then(|name| self.file.profiles.as_ref()?.get(name));

        let (api_key, api_key_source) =
            resolve("VENDO_API_KEY", profile.and_then(|p| p.api_key.as_deref()), None);
        let (base_url, base_url_source) = resolve(
            "VENDO_API_URL",
            profile.and_then(|p| p.base_url.as_deref()),
            Some(DEFAULT_BASE_URL),
        );
        let (account_id, account_id_source) = resolve(
            "VENDO_ACCOUNT_ID",
            profile.and_then(|p| p.account_id.as_deref()),
            None,
        );

        Effective {
            selected_profile: selected,
            api_key,
            api_key_source,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            base_url_source,
            account_id,
            account_id_source,
        }
    }

    pub fn profile_summaries(&self) -> Vec<ProfileSummary> {
        let active = self.selected_profile_name();
        self.file
            .profiles
            .iter()
            .flatten()
            .map(|(name, profile)| ProfileSummary {
                name: name.clone(),
                active: Some(name.as_str()) == active,
                account_id: profile.account_id.clone(),
                base_url: profile
                    .base_url
                    .clone()
                    .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            })
            .collect()
    }

    /// Save a named profile and make it active (used by login).
    pub fn save_profile(&mut self, name: &str, profile: Profile) -> Result<()> {
        self.file
            .profiles
            .get_or_insert_with(IndexMap::new)
            .insert(name.to_string(), profile);
        self.file.active_profile = Some(name.to_string());
        self.write()
    }

    fn write(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let body = serde_json::to_string_pretty(&self.file)? + "\n";
        fs::write(&self.path, body).with_context(|| format!("writing {}", self.path.display()))
    }
}

fn resolve(env: &str, profile: Option<&str>, default: Option<&str>) -> (Option<String>, Source) {
    if let Some(value) = std::env::var(env).ok().filter(|v| !v.is_empty()) {
        return (Some(value), Source::Env);
    }
    if let Some(value) = profile.filter(|v| !v.is_empty()) {
        return (Some(value.to_string()), Source::Profile);
    }
    match default {
        Some(value) => (Some(value.to_string()), Source::Default),
        None => (None, Source::Missing),
    }
}

/// Fold legacy flat fields into a profile: backfill gaps in the active profile,
/// otherwise create `default` (or `default-N`). Mirrors `migrateLegacyConfig`.
fn migrate_legacy(mut file: ConfigFile) -> (ConfigFile, bool) {
    if file.api_key.is_none() && file.base_url.is_none() && file.account_id.is_none() {
        return (file, false);
    }
    let legacy = Profile {
        api_key: file.api_key.take(),
        base_url: file.base_url.take(),
        account_id: file.account_id.take(),
        extra: Map::new(),
    };
    let profiles = file.profiles.get_or_insert_with(IndexMap::new);
    match file
        .active_profile
        .as_ref()
        .and_then(|name| profiles.get_mut(name))
    {
        Some(active) => {
            active.api_key = active.api_key.take().or(legacy.api_key);
            active.base_url = active.base_url.take().or(legacy.base_url);
            active.account_id = active.account_id.take().or(legacy.account_id);
        }
        None => {
            let mut name = "default".to_string();
            let mut suffix = 2;
            while profiles.contains_key(&name) {
                name = format!("default-{suffix}");
                suffix += 1;
            }
            profiles.insert(name.clone(), legacy);
            file.active_profile = Some(name);
        }
    }
    (file, true)
}
