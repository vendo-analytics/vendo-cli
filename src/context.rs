//! Per-run state every command needs: the config store (with the `--profile`
//! override and env snapshot), the home directory and the debug switch.

use std::path::PathBuf;

use crate::{
    client::Client,
    config::{ConfigStore, EffectiveConfig, EnvVars, default_config_path, require_api_key},
};

pub struct Ctx {
    pub store: ConfigStore,
    pub home: PathBuf,
    pub debug: bool,
}

impl Ctx {
    /// The state of one run: the config store with `--profile` (`profile`; else `VENDO_PROFILE`, else
    /// the active profile) and this process's environment, the home directory, and the debug switch,
    /// on with `--debug` or `VENDO_DEBUG`. The command and the questions before it ([`crate::ask`])
    /// make it alike.
    pub fn new(profile: Option<String>, debug: bool) -> Self {
        let debug = debug
            || std::env::var("VENDO_DEBUG")
                .map(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(false);
        let home = std::env::home_dir().unwrap_or_default();
        Ctx { store: ConfigStore::new(default_config_path(&home), profile, EnvVars::from_process()), home, debug }
    }

    pub fn effective(&self) -> EffectiveConfig {
        self.store.effective()
    }

    /// The API client for the effective profile; errors when no key is set.
    pub fn client(&self) -> anyhow::Result<Client> {
        let effective = self.effective();
        let api_key = require_api_key(&effective)?;
        Ok(Client::new(api_key, effective.base_url, effective.account_id, self.debug))
    }

    /// `~/.config/vendo/.update-check`, next to the config file.
    pub fn update_cache_path(&self) -> PathBuf {
        self.store.path().with_file_name(".update-check")
    }

    /// `~/.local/bin/vendo`, where the installer puts the binary.
    pub fn standard_binary_path(&self) -> PathBuf {
        self.home.join(".local").join("bin").join("vendo")
    }
}
