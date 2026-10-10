//! `~/.config/vendo/config.json`: the same file and shape the TypeScript CLI
//! uses (`src/config.ts`), so both binaries work against one config.
//!
//! The file is handled as an ordered JSON object, like the TS spread-merges:
//! unknown keys and key order survive every save, in JavaScript's order
//! (integer-like keys such as a profile named "2" first, ascending).

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::output::js_key_order;

pub const DEFAULT_BASE_URL: &str = "https://app2.vendodata.com";
pub const STAGING_BASE_URL: &str = "https://stg.vendodata.com";

/// The `VENDO_*` environment variables, read once at startup.
#[derive(Debug, Clone, Default)]
pub struct EnvVars {
    pub api_key: Option<String>,
    pub api_url: Option<String>,
    pub account_id: Option<String>,
    /// `VENDO_PROFILE`: the profile to use, as `--profile` names one (VE-3831).
    pub profile: Option<String>,
}

impl EnvVars {
    pub fn from_process() -> Self {
        EnvVars {
            api_key: std::env::var("VENDO_API_KEY").ok(),
            api_url: std::env::var("VENDO_API_URL").ok(),
            account_id: std::env::var("VENDO_ACCOUNT_ID").ok(),
            profile: std::env::var("VENDO_PROFILE").ok(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Env,
    Profile,
    Default,
    Missing,
}

#[derive(Debug, Clone)]
pub struct EffectiveConfig {
    pub config_exists: bool,
    pub config_path: PathBuf,
    pub selected_profile: Option<String>,
    pub selected_profile_exists: bool,
    /// `VENDO_PROFILE` chose `selected_profile` (no `--profile`), so errors and hints name it (VE-3831).
    pub selected_by_vendo_profile: bool,
    pub api_key: Option<String>,
    pub api_key_source: Source,
    pub base_url: String,
    pub base_url_source: Source,
    pub account_id: Option<String>,
    pub account_id_source: Source,
}

impl EffectiveConfig {
    /// The name `VENDO_PROFILE` gave when no profile has it (VE-3831): commands that need its key
    /// say so with [`unknown_vendo_profile`] instead of "no key" (Yalcin, 2026-10-06).
    pub fn unknown_vendo_profile(&self) -> Option<&str> {
        self.selected_profile.as_deref().filter(|_| self.selected_by_vendo_profile && !self.selected_profile_exists)
    }

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSummary {
    pub name: String,
    pub active: bool,
    pub account_id: Option<String>,
    pub base_url: String,
}

/// Values `vendo profile set` writes into the selected profile.
#[derive(Debug, Default)]
pub struct ConfigValueUpdates {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub account_id: Option<String>,
}

pub fn default_config_path(home: &Path) -> PathBuf {
    home.join(".config").join("vendo").join("config.json")
}

/// Config file access plus the `--profile` override and env snapshot.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
    profile_override: Option<String>,
    env: EnvVars,
}

impl ConfigStore {
    /// `--profile ""` is no override: the TS CLI checked `if (opts.profile)`. An empty
    /// `VENDO_PROFILE` is none either, like the other `VENDO_*` variables.
    pub fn new(path: PathBuf, profile_override: Option<String>, mut env: EnvVars) -> Self {
        env.profile = env.profile.filter(|name| !name.is_empty());
        ConfigStore { path, profile_override: profile_override.filter(|name| !name.is_empty()), env }
    }

    /// This store with `--profile <name>`: what `vendo --profile <name> …` reads. Login shows the
    /// workspace screen of the profile it saved this way (VE-4109).
    pub fn with_profile(&self, name: &str) -> ConfigStore {
        ConfigStore::new(self.path.clone(), Some(name.to_string()), self.env.clone())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The raw file as an object; `{}` when missing, unreadable or invalid.
    pub fn load(&self) -> Map<String, Value> {
        fs::read_to_string(&self.path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|value| match value {
                Value::Object(map) => Some(js_key_order(map)),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// Merge top-level `updates` into the file; `None` removes the key
    /// (the TS `saveConfig({ key: undefined })`).
    pub fn save(&self, updates: Vec<(&str, Option<Value>)>) -> Result<()> {
        let mut merged = self.load();
        for (key, value) in updates {
            match value {
                Some(value) => {
                    merged.insert(key.to_string(), value);
                }
                None => {
                    merged.shift_remove(key);
                }
            }
        }
        self.write(&merged)
    }

    fn write(&self, config: &Map<String, Value>) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let body = serde_json::to_string_pretty(&js_key_order(config.clone()))? + "\n";
        fs::write(&self.path, body).with_context(|| format!("writing {}", self.path.display()))
    }

    /// Read with legacy flat fields migrated into profiles. The migration is
    /// persisted best-effort: a read-only config dir must not break reads.
    pub fn read(&self) -> Map<String, Value> {
        let (config, migrated) = migrate_legacy_config(self.load());
        if migrated {
            let _ = self.write(&config);
        }
        config
    }

    /// `VENDO_PROFILE`'s name when it chose the profile: set, not empty, and no `--profile`
    /// (VE-3831). It never changes the saved `activeProfile` (Yalcin, 2026-10-06): `profile set`,
    /// `logout` and `login` act on profiles without making one active or unsetting it.
    pub fn vendo_profile(&self) -> Option<&str> {
        match self.profile_override {
            Some(_) => None,
            None => self.env.profile.as_deref(),
        }
    }

    /// The saved `activeProfile`, whatever `--profile` or `VENDO_PROFILE` select.
    pub fn saved_active_profile(&self) -> Option<String> {
        self.read().get("activeProfile").and_then(Value::as_str).map(str::to_string)
    }

    /// The profile commands use: `--profile` wins, then `VENDO_PROFILE` (VE-3831), then
    /// `activeProfile`. A name that no profile has is still the selection; when `VENDO_PROFILE`
    /// gave it, [`require_api_key`] says so.
    pub fn selected_profile_name(&self, config: &Map<String, Value>) -> Option<String> {
        self.profile_override
            .clone()
            .or_else(|| self.env.profile.clone())
            .or_else(|| config.get("activeProfile").and_then(Value::as_str).map(str::to_string))
    }

    pub fn effective(&self) -> EffectiveConfig {
        let config = self.read();
        let selected = self.selected_profile_name(&config);
        let profile = selected.as_deref().and_then(|name| profile_of(&config, name));
        let field = |key: &str| profile.and_then(|p| p.get(key)).and_then(Value::as_str);

        let (api_key, api_key_source) = resolve(self.env.api_key.as_deref(), field("apiKey"), None);
        let (base_url, base_url_source) =
            resolve(self.env.api_url.as_deref(), field("baseUrl"), Some(DEFAULT_BASE_URL));
        let (account_id, account_id_source) = resolve(self.env.account_id.as_deref(), field("accountId"), None);

        EffectiveConfig {
            config_exists: self.path.exists(),
            config_path: self.path.clone(),
            selected_profile_exists: profile.is_some(),
            selected_by_vendo_profile: self.vendo_profile().is_some(),
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
        let config = self.read();
        self.profile_summaries_of(&config)
    }

    fn profile_summaries_of(&self, config: &Map<String, Value>) -> Vec<ProfileSummary> {
        let active = self.selected_profile_name(config);
        let Some(Value::Object(profiles)) = config.get("profiles") else { return Vec::new() };
        profiles
            .iter()
            .map(|(name, profile)| ProfileSummary {
                name: name.clone(),
                active: Some(name) == active.as_ref(),
                account_id: profile.get("accountId").and_then(Value::as_str).map(str::to_string),
                base_url: profile.get("baseUrl").and_then(Value::as_str).unwrap_or(DEFAULT_BASE_URL).to_string(),
            })
            .collect()
    }

    pub fn find_profiles_by_account_id(&self, account_id: &str) -> Vec<ProfileSummary> {
        self.profile_summaries().into_iter().filter(|p| p.account_id.as_deref() == Some(account_id)).collect()
    }

    /// Save a named profile (replacing it) and make it active, unless `VENDO_PROFILE` chose the
    /// profile ([`Self::vendo_profile`]). Used by login.
    pub fn save_profile(&self, name: &str, profile: Map<String, Value>) -> Result<()> {
        let config = self.read();
        let mut profiles = profiles_map(&config);
        profiles.insert(name.to_string(), Value::Object(profile));
        self.save(self.with_active(vec![("profiles", Some(Value::Object(profiles)))], Some(name)))
    }

    /// `updates` and `activeProfile` set to `active` (`None` unsets it), unless `VENDO_PROFILE`
    /// chose the profile, which leaves `activeProfile` as saved.
    fn with_active<'a>(
        &self,
        mut updates: Vec<(&'a str, Option<Value>)>,
        active: Option<&str>,
    ) -> Vec<(&'a str, Option<Value>)> {
        if self.vendo_profile().is_none() {
            updates.push(("activeProfile", active.map(|name| Value::String(name.to_string()))));
        }
        updates
    }

    /// `vendo profile set`: write the given values into the selected profile,
    /// creating `default` when none is selected, and make it active unless `VENDO_PROFILE`
    /// chose it. Returns the profile name.
    pub fn save_resolved_values(&self, updates: ConfigValueUpdates) -> Result<String> {
        let config = self.read();
        let name = self.selected_profile_name(&config).unwrap_or_else(|| "default".to_string());
        let mut profiles = profiles_map(&config);
        let mut profile = match profiles.get(&name) {
            Some(Value::Object(existing)) => existing.clone(),
            _ => Map::new(),
        };
        for (key, value) in
            [("apiKey", updates.api_key), ("baseUrl", updates.base_url), ("accountId", updates.account_id)]
        {
            if let Some(value) = value {
                profile.insert(key.to_string(), Value::String(value));
            }
        }
        profiles.insert(name.clone(), Value::Object(profile));
        self.save(self.with_active(vec![("profiles", Some(Value::Object(profiles)))], Some(&name)))?;
        Ok(name)
    }

    /// Point `activeProfile` at an existing profile (`profile switch`).
    pub fn set_active_profile(&self, name: &str) -> Result<()> {
        self.save(vec![("activeProfile", Some(Value::String(name.to_string())))])
    }

    /// Remove the selected profile (logout) and unset `activeProfile`, which `VENDO_PROFILE`
    /// leaves as saved. Returns its name, or `None` when no profile is selected (an empty name
    /// counts as none) or it doesn't exist.
    pub fn clear_active_profile(&self) -> Result<Option<String>> {
        let config = self.read();
        let Some(name) = self.selected_profile_name(&config).filter(|name| !name.is_empty()) else { return Ok(None) };
        let mut profiles = profiles_map(&config);
        if profiles.shift_remove(&name).is_none() {
            return Ok(None);
        }
        self.save(self.with_active(vec![("profiles", Some(Value::Object(profiles)))], None))?;
        Ok(Some(name))
    }

    /// Delete the whole file. `true` when something was deleted.
    pub fn delete(&self) -> bool {
        fs::remove_file(&self.path).is_ok()
    }

    /// Base URL for a login: `--base-url` wins, then `--env`, then the normal
    /// chain. Explicitly empty values are errors, never a silent prod fallback
    /// (VE-1563, VE-1603).
    pub fn resolve_login_base_url(&self, env: Option<&str>, base_url: Option<&str>) -> Result<String> {
        if let Some(raw) = base_url {
            let parsed = reqwest::Url::parse(raw).map_err(|_| {
                anyhow::anyhow!("Invalid --base-url: \"{raw}\". Expected a full URL like {STAGING_BASE_URL}.")
            })?;
            if !matches!(parsed.scheme(), "http" | "https") {
                bail!("Invalid --base-url protocol: \"{raw}\". Use http(s).");
            }
            return Ok(parsed.origin().ascii_serialization());
        }
        if let Some(env) = env {
            return match env.to_lowercase().as_str() {
                "staging" | "stg" => Ok(STAGING_BASE_URL.to_string()),
                "prod" | "production" => Ok(DEFAULT_BASE_URL.to_string()),
                _ => bail!("Unknown --env \"{env}\". Use \"staging\" or \"prod\"."),
            };
        }
        Ok(self.effective().base_url)
    }
}

pub fn mask_api_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "****".to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}...{tail}")
}

/// The key commands send. Without one, the error says why: a `VENDO_PROFILE` that names no
/// profile is named, with how to fix it (Yalcin, 2026-10-06); otherwise there is no key.
pub fn require_api_key(effective: &EffectiveConfig) -> Result<String> {
    if let Some(key) = &effective.api_key {
        return Ok(key.clone());
    }
    match effective.unknown_vendo_profile() {
        Some(name) => bail!(unknown_vendo_profile(name)),
        None => bail!(
            "No API key configured. Run `vendo login` or `vendo profile set --api-key <key>` or set VENDO_API_KEY."
        ),
    }
}

/// The error for a `VENDO_PROFILE` that names no profile, wherever the CLI would otherwise say it
/// has no key: [`require_api_key`], `logout` and `mcp`'s hint (Yalcin, 2026-10-06).
pub fn unknown_vendo_profile(name: &str) -> String {
    format!(
        "Profile \"{name}\" not found (VENDO_PROFILE selects it).\n  Run `vendo profile list` to see your profiles, or unset VENDO_PROFILE to use the active profile."
    )
}

/// What hints about switching profiles, or checking one with `vendo workspace`, add while
/// `VENDO_PROFILE` chose the profile (Yalcin, 2026-10-06).
pub fn vendo_profile_overrides(name: &str) -> String {
    format!("VENDO_PROFILE={name} overrides the active profile in this shell")
}

fn profile_of<'a>(config: &'a Map<String, Value>, name: &str) -> Option<&'a Map<String, Value>> {
    config.get("profiles")?.as_object()?.get(name)?.as_object()
}

fn profiles_map(config: &Map<String, Value>) -> Map<String, Value> {
    match config.get("profiles") {
        Some(Value::Object(profiles)) => profiles.clone(),
        _ => Map::new(),
    }
}

fn resolve(env: Option<&str>, profile: Option<&str>, default: Option<&str>) -> (Option<String>, Source) {
    if let Some(value) = env.filter(|v| !v.is_empty()) {
        return (Some(value.to_string()), Source::Env);
    }
    if let Some(value) = profile.filter(|v| !v.is_empty()) {
        return (Some(value.to_string()), Source::Profile);
    }
    match default {
        Some(value) => (Some(value.to_string()), Source::Default),
        None => (None, Source::Missing),
    }
}

/// Fold legacy flat fields (`apiKey`/`baseUrl`/`accountId`) into a profile:
/// backfill gaps in the active profile (profile wins), otherwise create
/// `default` (or `default-N`). The result is profiles-only.
pub fn migrate_legacy_config(config: Map<String, Value>) -> (Map<String, Value>, bool) {
    const LEGACY: [&str; 3] = ["apiKey", "baseUrl", "accountId"];
    if !LEGACY.iter().any(|key| config.contains_key(*key)) {
        return (config, false);
    }

    let mut legacy = Map::new();
    for key in LEGACY {
        if let Some(value) = config.get(key) {
            legacy.insert(key.to_string(), value.clone());
        }
    }
    let mut profiles = profiles_map(&config);
    let mut active = config.get("activeProfile").and_then(Value::as_str).map(str::to_string);

    match active.as_ref().and_then(|name| profiles.get_mut(name)).and_then(Value::as_object_mut) {
        Some(profile) => {
            let mut merged = legacy;
            for (key, value) in profile.iter() {
                merged.insert(key.clone(), value.clone());
            }
            *profile = merged;
        }
        None => {
            let mut name = "default".to_string();
            let mut suffix = 2;
            while profiles.contains_key(&name) {
                name = format!("default-{suffix}");
                suffix += 1;
            }
            profiles.insert(name.clone(), Value::Object(legacy));
            active = Some(name);
        }
    }

    let mut migrated = Map::new();
    migrated.insert("profiles".to_string(), Value::Object(profiles));
    if let Some(active) = active {
        migrated.insert("activeProfile".to_string(), Value::String(active));
    }
    (migrated, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fixture {
        _dir: tempfile::TempDir,
        path: PathBuf,
    }

    fn fixture(contents: Option<Value>) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".config/vendo/config.json");
        if let Some(contents) = contents {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents.to_string()).unwrap();
        }
        Fixture { _dir: dir, path }
    }

    fn store(f: &Fixture) -> ConfigStore {
        ConfigStore::new(f.path.clone(), None, EnvVars::default())
    }

    fn on_disk(f: &Fixture) -> Value {
        serde_json::from_str(&fs::read_to_string(&f.path).unwrap()).unwrap()
    }

    #[test]
    fn default_path_is_under_dot_config() {
        assert_eq!(default_config_path(Path::new("/mock-home")), PathBuf::from("/mock-home/.config/vendo/config.json"));
    }

    #[test]
    fn load_returns_empty_for_missing_or_invalid_files() {
        assert!(store(&fixture(None)).load().is_empty());
        let f = fixture(None);
        fs::create_dir_all(f.path.parent().unwrap()).unwrap();
        fs::write(&f.path, "not json").unwrap();
        assert!(store(&f).load().is_empty());
    }

    #[test]
    fn save_merges_with_existing_and_keeps_unknown_keys() {
        let f = fixture(Some(json!({ "activeProfile": "a", "extra": 1 })));
        store(&f).save(vec![("profiles", Some(json!({ "a": {} })))]).unwrap();
        assert_eq!(on_disk(&f), json!({ "activeProfile": "a", "extra": 1, "profiles": { "a": {} } }));
        assert!(fs::read_to_string(&f.path).unwrap().ends_with("}\n"));
    }

    #[test]
    fn save_creates_the_file_when_missing() {
        let f = fixture(None);
        store(&f).save(vec![("activeProfile", Some(json!("x")))]).unwrap();
        assert_eq!(on_disk(&f), json!({ "activeProfile": "x" }));
    }

    #[test]
    fn save_profile_sets_it_active_and_keeps_others() {
        let f = fixture(Some(json!({ "profiles": { "old": { "apiKey": "k1" } }, "activeProfile": "old" })));
        let mut profile = Map::new();
        profile.insert("apiKey".into(), json!("k2"));
        store(&f).save_profile("new", profile).unwrap();
        assert_eq!(
            on_disk(&f),
            json!({ "profiles": { "old": { "apiKey": "k1" }, "new": { "apiKey": "k2" } }, "activeProfile": "new" })
        );
    }

    #[test]
    fn list_profiles_reports_active_account_and_default_base_url() {
        let f = fixture(Some(json!({
            "profiles": {
                "a": { "apiKey": "k", "accountId": "acct-a" },
                "b": { "apiKey": "k", "baseUrl": STAGING_BASE_URL },
            },
            "activeProfile": "b",
        })));
        assert_eq!(
            store(&f).profile_summaries(),
            vec![
                ProfileSummary {
                    name: "a".into(),
                    active: false,
                    account_id: Some("acct-a".into()),
                    base_url: DEFAULT_BASE_URL.into()
                },
                ProfileSummary { name: "b".into(), active: true, account_id: None, base_url: STAGING_BASE_URL.into() },
            ]
        );
        assert!(store(&fixture(None)).profile_summaries().is_empty());
    }

    #[test]
    fn find_profiles_by_account_id_matches_all() {
        let f = fixture(Some(json!({ "profiles": {
            "a": { "accountId": "x" }, "b": { "accountId": "y" }, "c": { "accountId": "x" },
        } })));
        let names: Vec<_> = store(&f).find_profiles_by_account_id("x").into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["a", "c"]);
    }

    #[test]
    fn login_base_url_maps_env_flags() {
        let s = store(&fixture(Some(json!({}))));
        assert_eq!(s.resolve_login_base_url(Some("staging"), None).unwrap(), STAGING_BASE_URL);
        assert_eq!(s.resolve_login_base_url(Some("stg"), None).unwrap(), STAGING_BASE_URL);
        assert_eq!(s.resolve_login_base_url(Some("prod"), None).unwrap(), DEFAULT_BASE_URL);
        assert_eq!(s.resolve_login_base_url(Some("Production"), None).unwrap(), DEFAULT_BASE_URL);
        assert!(s.resolve_login_base_url(Some("qa"), None).unwrap_err().to_string().contains("Unknown --env"));
    }

    #[test]
    fn login_base_url_prefers_base_url_and_normalizes_to_origin() {
        let s = store(&fixture(Some(json!({}))));
        assert_eq!(
            s.resolve_login_base_url(Some("prod"), Some("https://stg.vendodata.com/some/path")).unwrap(),
            "https://stg.vendodata.com"
        );
        assert!(
            s.resolve_login_base_url(None, Some("not a url")).unwrap_err().to_string().contains("Invalid --base-url")
        );
        assert!(s.resolve_login_base_url(None, Some("ftp://x.com")).unwrap_err().to_string().contains("protocol"));
    }

    #[test]
    fn login_base_url_rejects_explicitly_empty_values() {
        let s = store(&fixture(Some(json!({}))));
        assert!(s.resolve_login_base_url(None, Some("")).unwrap_err().to_string().contains("Invalid --base-url"));
        assert!(s.resolve_login_base_url(Some(""), None).unwrap_err().to_string().contains("Unknown --env"));
    }

    #[test]
    fn login_base_url_falls_back_to_env_then_prod() {
        let f = fixture(Some(json!({})));
        let env = EnvVars { api_url: Some("https://env.example.com".into()), ..Default::default() };
        assert_eq!(
            ConfigStore::new(f.path.clone(), None, env).resolve_login_base_url(None, None).unwrap(),
            "https://env.example.com"
        );
        assert_eq!(store(&f).resolve_login_base_url(None, None).unwrap(), DEFAULT_BASE_URL);
    }

    #[test]
    fn env_vars_win_over_the_profile() {
        let f = fixture(Some(
            json!({ "profiles": { "p": { "apiKey": "file_key", "accountId": "file-id", "baseUrl": "https://file.com" } }, "activeProfile": "p" }),
        ));
        let env = EnvVars {
            api_key: Some("env_key".into()),
            api_url: Some("https://env.com".into()),
            account_id: Some("env-id".into()),
            profile: None,
        };
        let e = ConfigStore::new(f.path.clone(), None, env).effective();
        assert_eq!(
            (e.api_key.as_deref(), e.base_url.as_str(), e.account_id.as_deref()),
            (Some("env_key"), "https://env.com", Some("env-id"))
        );
        let e = store(&f).effective();
        assert_eq!(
            (e.api_key.as_deref(), e.base_url.as_str(), e.account_id.as_deref()),
            (Some("file_key"), "https://file.com", Some("file-id"))
        );
    }

    #[test]
    fn empty_env_values_count_as_unset() {
        let f = fixture(Some(json!({ "profiles": { "p": { "apiKey": "file_key" } }, "activeProfile": "p" })));
        let env = EnvVars { api_key: Some(String::new()), ..Default::default() };
        assert_eq!(ConfigStore::new(f.path.clone(), None, env).effective().api_key.as_deref(), Some("file_key"));
    }

    #[test]
    fn default_base_url_when_nothing_is_configured() {
        let e = store(&fixture(Some(json!({})))).effective();
        assert_eq!(e.base_url, DEFAULT_BASE_URL);
        assert_eq!(e.base_url_source, Source::Default);
    }

    #[test]
    fn legacy_config_with_missing_active_profile_still_resolves() {
        let f = fixture(Some(
            json!({ "apiKey": "legacy_key", "accountId": "legacy-id", "activeProfile": "nonexistent", "profiles": {} }),
        ));
        let e = store(&f).effective();
        assert_eq!(e.api_key.as_deref(), Some("legacy_key"));
        assert_eq!(e.account_id.as_deref(), Some("legacy-id"));
    }

    #[test]
    fn profile_override_wins_over_active_profile() {
        let f = fixture(Some(json!({ "profiles": {
            "default": { "apiKey": "default_key", "accountId": "default-id" },
            "override": { "apiKey": "override_key", "accountId": "override-id" },
        }, "activeProfile": "default" })));
        let s = ConfigStore::new(f.path.clone(), Some("override".into()), EnvVars::default());
        let e = s.effective();
        assert_eq!((e.api_key.as_deref(), e.account_id.as_deref()), (Some("override_key"), Some("override-id")));
        assert_eq!(store(&f).selected_profile_name(&store(&f).read()).as_deref(), Some("default"));
        assert_eq!(s.selected_profile_name(&s.read()).as_deref(), Some("override"));
    }

    fn with_env_profile(f: &Fixture, flag: Option<&str>, env: Option<&str>) -> ConfigStore {
        let env = EnvVars { profile: env.map(Into::into), ..Default::default() };
        ConfigStore::new(f.path.clone(), flag.map(Into::into), env)
    }

    #[test]
    fn vendo_profile_selects_a_profile_and_the_flag_wins_over_it() {
        // VE-3831: --profile > VENDO_PROFILE > activeProfile.
        let f = fixture(Some(json!({ "profiles": {
            "alpha": { "apiKey": "alpha_key", "accountId": "alpha-id" },
            "beta": { "apiKey": "beta_key", "accountId": "beta-id" },
            "gamma": { "apiKey": "gamma_key", "accountId": "gamma-id" },
        }, "activeProfile": "alpha" })));
        let selection = |flag, env| {
            let e = with_env_profile(&f, flag, env).effective();
            (e.selected_profile, e.account_id)
        };
        let some = |name: &str, id: &str| (Some(name.to_string()), Some(id.to_string()));
        assert_eq!(selection(None, None), some("alpha", "alpha-id"));
        assert_eq!(selection(None, Some("beta")), some("beta", "beta-id"));
        assert_eq!(selection(Some("gamma"), Some("beta")), some("gamma", "gamma-id"));
        assert_eq!(selection(Some("gamma"), None), some("gamma", "gamma-id"));
        // Empty is unset, for either: `--profile ""` falls through to VENDO_PROFILE.
        assert_eq!(selection(None, Some("")), some("alpha", "alpha-id"));
        assert_eq!(selection(Some(""), Some("beta")), some("beta", "beta-id"));
        // The profile it names is the one profile commands act on, as with --profile.
        let s = with_env_profile(&f, None, Some("beta"));
        assert_eq!(s.selected_profile_name(&s.read()).as_deref(), Some("beta"));
        let active: Vec<String> = s.profile_summaries().into_iter().filter(|p| p.active).map(|p| p.name).collect();
        assert_eq!(active, ["beta"]);
    }

    #[test]
    fn an_unknown_vendo_profile_is_unresolved_and_its_error_names_it() {
        let f = fixture(Some(json!({ "profiles": { "alpha": { "apiKey": "a" } }, "activeProfile": "alpha" })));
        let from_env = with_env_profile(&f, None, Some("missing")).effective();
        let from_flag = with_env_profile(&f, Some("missing"), None).effective();
        let flag_over_env = with_env_profile(&f, Some("missing"), Some("alpha")).effective();
        for e in [&from_env, &from_flag, &flag_over_env] {
            assert_eq!(e.selected_profile.as_deref(), Some("missing"));
            assert!(!e.selected_profile_exists);
            assert_eq!((e.api_key.as_deref(), e.api_key_source), (None, Source::Missing));
        }
        assert_eq!((from_env.selected_by_vendo_profile, from_flag.selected_by_vendo_profile), (true, false));
        assert!(!flag_over_env.selected_by_vendo_profile);
        let unknown = |e: &EffectiveConfig| e.unknown_vendo_profile().map(str::to_string);
        assert_eq!([&from_env, &from_flag, &flag_over_env].map(unknown), [Some("missing".to_string()), None, None]);
        assert_eq!(unknown(&with_env_profile(&f, None, Some("alpha")).effective()), None);
        // Decided by Yalcin, 2026-10-06: the error names the profile and VENDO_PROFILE, and how to fix it.
        assert_eq!(
            require_api_key(&from_env).unwrap_err().to_string(),
            "Profile \"missing\" not found (VENDO_PROFILE selects it).\n  Run `vendo profile list` to see your profiles, or unset VENDO_PROFILE to use the active profile."
        );
        for e in [&from_flag, &flag_over_env] {
            assert!(require_api_key(e).unwrap_err().to_string().starts_with("No API key configured."));
        }
        // A key from VENDO_API_KEY is still sent, as with an unknown --profile.
        let env = EnvVars { api_key: Some("env_key".into()), profile: Some("missing".into()), ..Default::default() };
        let e = ConfigStore::new(f.path.clone(), None, env).effective();
        assert_eq!(require_api_key(&e).unwrap(), "env_key");
    }

    #[test]
    fn vendo_profile_never_changes_the_saved_active_profile() {
        let config =
            json!({ "profiles": { "alpha": { "apiKey": "a" }, "beta": { "apiKey": "b" } }, "activeProfile": "alpha" });
        let f = fixture(Some(config.clone()));
        let s = with_env_profile(&f, None, Some("beta"));
        assert_eq!(s.vendo_profile(), Some("beta"));
        // profile set writes to beta, login saves its profile, logout removes beta: alpha stays active.
        s.save_resolved_values(ConfigValueUpdates { account_id: Some("b-1".into()), ..Default::default() }).unwrap();
        s.save_profile("gamma", Map::new()).unwrap();
        assert_eq!(s.clear_active_profile().unwrap().as_deref(), Some("beta"));
        assert_eq!(
            on_disk(&f),
            json!({ "profiles": { "alpha": { "apiKey": "a" }, "gamma": {} }, "activeProfile": "alpha" })
        );
        // Removing the active profile through VENDO_PROFILE leaves activeProfile as saved too.
        assert_eq!(with_env_profile(&f, None, Some("alpha")).clear_active_profile().unwrap().as_deref(), Some("alpha"));
        assert_eq!(on_disk(&f), json!({ "profiles": { "gamma": {} }, "activeProfile": "alpha" }));
        // --profile, which wins over VENDO_PROFILE, makes the profile it writes active as before.
        let f = fixture(Some(config));
        let s = with_env_profile(&f, Some("beta"), Some("alpha"));
        assert_eq!(s.vendo_profile(), None);
        s.save_resolved_values(ConfigValueUpdates { account_id: Some("b-1".into()), ..Default::default() }).unwrap();
        assert_eq!(on_disk(&f)["activeProfile"], "beta");
        s.clear_active_profile().unwrap();
        assert_eq!(on_disk(&f), json!({ "profiles": { "alpha": { "apiKey": "a" } } }));
    }

    #[test]
    fn effective_reports_sources_and_file_presence() {
        let missing = fixture(None);
        assert!(!store(&missing).effective().config_exists);

        let f = fixture(Some(
            json!({ "profiles": { "team": { "apiKey": "k", "accountId": "acct-1", "baseUrl": "https://profile.com" } }, "activeProfile": "team" }),
        ));
        let e = store(&f).effective();
        assert!(e.config_exists);
        assert_eq!(e.selected_profile.as_deref(), Some("team"));
        assert!(e.selected_profile_exists);
        assert_eq!(
            (e.api_key_source, e.base_url_source, e.account_id_source),
            (Source::Profile, Source::Profile, Source::Profile)
        );
    }

    #[test]
    fn missing_override_profile_is_unresolved() {
        let f = fixture(Some(json!({ "profiles": { "alpha": { "apiKey": "a" } }, "activeProfile": "alpha" })));
        let e = ConfigStore::new(f.path.clone(), Some("missing".into()), EnvVars::default()).effective();
        assert_eq!(e.selected_profile.as_deref(), Some("missing"));
        assert!(!e.selected_profile_exists);
        assert_eq!((e.api_key_source, e.account_id_source), (Source::Missing, Source::Missing));
    }

    #[test]
    fn require_api_key_message() {
        let e = store(&fixture(Some(json!({})))).effective();
        assert!(require_api_key(&e).unwrap_err().to_string().starts_with("No API key configured."));
    }

    #[test]
    fn delete_reports_whether_a_file_was_removed() {
        let f = fixture(Some(json!({})));
        assert!(store(&f).delete());
        assert!(!store(&f).delete());
    }

    #[test]
    fn mask_api_key_keeps_four_characters_each_side() {
        assert_eq!(mask_api_key("vendo_sk_abcdefghijkl"), "vend...ijkl");
        assert_eq!(mask_api_key("short"), "****");
        assert_eq!(mask_api_key("12345678"), "****");
        assert_eq!(mask_api_key("123456789"), "1234...6789");
    }

    #[test]
    fn migration_leaves_profiles_only_configs_alone() {
        let config = json!({ "profiles": { "a": { "apiKey": "k" } }, "activeProfile": "a" });
        let (out, migrated) = migrate_legacy_config(config.as_object().unwrap().clone());
        assert!(!migrated);
        assert_eq!(Value::Object(out), config);
    }

    #[test]
    fn migration_folds_flat_config_into_default() {
        let config = json!({ "apiKey": "k", "baseUrl": "https://x.com", "accountId": "a" });
        let (out, migrated) = migrate_legacy_config(config.as_object().unwrap().clone());
        assert!(migrated);
        assert_eq!(
            Value::Object(out),
            json!({ "profiles": { "default": { "apiKey": "k", "baseUrl": "https://x.com", "accountId": "a" } }, "activeProfile": "default" })
        );
    }

    #[test]
    fn migration_backfills_the_active_profile_and_profile_wins() {
        let config = json!({ "apiKey": "legacy", "accountId": "legacy-acct", "activeProfile": "p", "profiles": { "p": { "apiKey": "profile" } } });
        let (out, _) = migrate_legacy_config(config.as_object().unwrap().clone());
        assert_eq!(out["profiles"]["p"], json!({ "apiKey": "profile", "accountId": "legacy-acct" }));
        assert_eq!(out["activeProfile"], json!("p"));
    }

    #[test]
    fn migration_creates_default_when_active_profile_is_missing() {
        let config = json!({ "apiKey": "k", "activeProfile": "ghost", "profiles": {} });
        let (out, _) = migrate_legacy_config(config.as_object().unwrap().clone());
        assert_eq!(
            Value::Object(out),
            json!({ "profiles": { "default": { "apiKey": "k" } }, "activeProfile": "default" })
        );
    }

    #[test]
    fn migration_handles_account_only_configs() {
        let config = json!({ "accountId": "a" });
        let (out, _) = migrate_legacy_config(config.as_object().unwrap().clone());
        assert_eq!(out["profiles"]["default"], json!({ "accountId": "a" }));
    }

    #[test]
    fn migration_does_not_clobber_an_existing_default() {
        let config = json!({ "apiKey": "k", "profiles": { "default": { "apiKey": "x" }, "default-2": {} } });
        let (out, _) = migrate_legacy_config(config.as_object().unwrap().clone());
        assert_eq!(out["profiles"]["default"], json!({ "apiKey": "x" }));
        assert_eq!(out["profiles"]["default-3"], json!({ "apiKey": "k" }));
        assert_eq!(out["activeProfile"], json!("default-3"));
    }

    #[test]
    fn migration_is_persisted_on_read() {
        let f = fixture(Some(json!({ "apiKey": "k", "accountId": "a" })));
        store(&f).effective();
        assert_eq!(
            on_disk(&f),
            json!({ "profiles": { "default": { "apiKey": "k", "accountId": "a" } }, "activeProfile": "default" })
        );
    }

    #[cfg(unix)]
    #[test]
    fn migration_still_resolves_when_the_write_fails() {
        use std::os::unix::fs::PermissionsExt;
        let f = fixture(Some(json!({ "apiKey": "k", "accountId": "a" })));
        let dir = f.path.parent().unwrap();
        fs::set_permissions(&f.path, fs::Permissions::from_mode(0o444)).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
        let e = store(&f).effective();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(e.api_key.as_deref(), Some("k"));
        assert_eq!(on_disk(&f), json!({ "apiKey": "k", "accountId": "a" }));
    }

    #[test]
    fn save_resolved_values_targets_the_selected_or_default_profile() {
        let f = fixture(None);
        let name = store(&f)
            .save_resolved_values(ConfigValueUpdates { api_key: Some("k".into()), ..Default::default() })
            .unwrap();
        assert_eq!(name, "default");
        let name = store(&f)
            .save_resolved_values(ConfigValueUpdates { account_id: Some("a".into()), ..Default::default() })
            .unwrap();
        assert_eq!(name, "default");
        assert_eq!(
            on_disk(&f),
            json!({ "profiles": { "default": { "apiKey": "k", "accountId": "a" } }, "activeProfile": "default" })
        );
    }

    #[test]
    fn clear_active_profile_removes_it_and_unsets_active() {
        let f = fixture(Some(
            json!({ "profiles": { "a": { "apiKey": "1" }, "b": { "apiKey": "2" } }, "activeProfile": "a" }),
        ));
        assert_eq!(store(&f).clear_active_profile().unwrap().as_deref(), Some("a"));
        assert_eq!(on_disk(&f), json!({ "profiles": { "b": { "apiKey": "2" } } }));
        assert_eq!(store(&f).clear_active_profile().unwrap(), None);
    }

    #[test]
    fn an_empty_profile_override_is_no_override() {
        let f = fixture(Some(json!({ "profiles": { "alpha": { "apiKey": "a" } }, "activeProfile": "alpha" })));
        let s = ConfigStore::new(f.path.clone(), Some(String::new()), EnvVars::default());
        assert_eq!(s.effective().selected_profile.as_deref(), Some("alpha"));
        s.save_resolved_values(ConfigValueUpdates { account_id: Some("x".into()), ..Default::default() }).unwrap();
        assert_eq!(
            on_disk(&f),
            json!({ "profiles": { "alpha": { "apiKey": "a", "accountId": "x" } }, "activeProfile": "alpha" })
        );
    }

    #[test]
    fn logout_leaves_an_empty_active_profile_name_alone() {
        let config = json!({ "profiles": { "": { "apiKey": "k" } }, "activeProfile": "" });
        let f = fixture(Some(config.clone()));
        assert_eq!(store(&f).clear_active_profile().unwrap(), None);
        assert_eq!(on_disk(&f), config);
    }

    #[test]
    fn integer_like_profile_names_come_first_as_in_javascript() {
        let f = fixture(Some(json!({
            "profiles": { "beta": {}, "10": {}, "2": {}, "alpha": {}, "01": {}, "4294967295": {}, "4294967294": {} },
            "activeProfile": "beta",
            "7": 1,
        })));
        let names: Vec<String> = store(&f).profile_summaries().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["2", "10", "4294967294", "beta", "alpha", "01", "4294967295"]);
        store(&f).set_active_profile("2").unwrap();
        // What `JSON.stringify({ ...config, activeProfile })` writes in Node.
        assert_eq!(
            serde_json::to_string(&on_disk(&f)).unwrap(),
            r#"{"7":1,"profiles":{"2":{},"10":{},"4294967294":{},"beta":{},"alpha":{},"01":{},"4294967295":{}},"activeProfile":"2"}"#
        );
    }
}
