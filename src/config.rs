use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    pub discord: DiscordSettings,
    pub webhook: WebhookSettings,
    #[serde(default)]
    pub message: MessageTemplateSettings,
    #[serde(default)]
    pub steam: SteamSettings,
    #[serde(default)]
    pub cache: CacheSettings,
    #[serde(default)]
    pub state_cache: StateCacheSettings,
}

impl Settings {
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file from {}", path.display()))?;
        let settings: Self =
            toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))?;
        settings.validate()?;
        Ok(settings)
    }

    fn validate(&self) -> Result<()> {
        self.discord.validate()?;
        self.webhook.validate()?;
        self.steam.validate()?;
        self.cache.validate()?;
        self.state_cache.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiscordSettings {
    pub bot_token: String,
    pub user_id: u64,
    pub guild_id: Option<u64>,
    #[serde(default)]
    pub emit_initial_status: bool,
}

impl DiscordSettings {
    fn validate(&self) -> Result<()> {
        if self.bot_token.trim().is_empty() {
            bail!("discord.bot_token cannot be empty");
        }
        if self.user_id == 0 {
            bail!("discord.user_id must be greater than 0");
        }
        if self.guild_id.is_some_and(|guild_id| guild_id == 0) {
            bail!("discord.guild_id must be greater than 0 when provided");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebhookSettings {
    #[serde(default = "default_webhook_mode")]
    pub mode: WebhookMode,
    pub url: String,
    pub token: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub openclaw: OpenClawSettings,
}

impl WebhookSettings {
    fn validate(&self) -> Result<()> {
        if self.url.trim().is_empty() {
            bail!("webhook.url cannot be empty");
        }
        reqwest::Url::parse(&self.url)
            .with_context(|| format!("webhook.url is not a valid URL: {}", self.url))?;
        if self.timeout_seconds == 0 {
            bail!("webhook.timeout_seconds must be greater than 0");
        }
        Ok(())
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum OpenClawWakeMode {
    #[default]
    Now,
    NextHeartbeat,
}

impl OpenClawWakeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Now => "now",
            Self::NextHeartbeat => "next-heartbeat",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct OpenClawSettings {
    #[serde(default)]
    pub wake_mode: OpenClawWakeMode,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookMode {
    OpenclawWake,
    GenericJson,
}

fn default_webhook_mode() -> WebhookMode {
    WebhookMode::OpenclawWake
}

fn default_timeout_seconds() -> u64 {
    10
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MessageTemplateSettings {
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SteamSettings {
    #[serde(default)]
    pub enabled: bool,
    pub api_key: Option<String>,
    #[serde(default = "default_steam_language")]
    pub language: String,
    #[serde(default = "default_steam_description_max_chars")]
    pub description_max_chars: usize,
    #[serde(default = "default_steam_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_steam_memory_cache_ttl_seconds")]
    pub memory_cache_ttl_seconds: u64,
    #[serde(default = "default_steam_memory_cache_capacity")]
    pub memory_cache_capacity: usize,
    #[serde(default = "default_steam_db_cache_ttl_seconds")]
    pub db_cache_ttl_seconds: u64,
}

impl Default for SteamSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: None,
            language: default_steam_language(),
            description_max_chars: default_steam_description_max_chars(),
            timeout_seconds: default_steam_timeout_seconds(),
            memory_cache_ttl_seconds: default_steam_memory_cache_ttl_seconds(),
            memory_cache_capacity: default_steam_memory_cache_capacity(),
            db_cache_ttl_seconds: default_steam_db_cache_ttl_seconds(),
        }
    }
}

impl SteamSettings {
    fn validate(&self) -> Result<()> {
        if self.enabled && self.language.trim().is_empty() {
            bail!("steam.language cannot be empty when steam.enabled=true");
        }
        if self
            .api_key
            .as_ref()
            .is_some_and(|key| key.trim().is_empty())
        {
            bail!("steam.api_key cannot be empty when provided");
        }
        if self.description_max_chars == 0 {
            bail!("steam.description_max_chars must be greater than 0");
        }
        if self.timeout_seconds == 0 {
            bail!("steam.timeout_seconds must be greater than 0");
        }
        if self.memory_cache_ttl_seconds == 0 {
            bail!("steam.memory_cache_ttl_seconds must be greater than 0");
        }
        if self.memory_cache_capacity == 0 {
            bail!("steam.memory_cache_capacity must be greater than 0");
        }
        if self.db_cache_ttl_seconds == 0 {
            bail!("steam.db_cache_ttl_seconds must be greater than 0");
        }
        Ok(())
    }
}

fn default_steam_language() -> String {
    "schinese".to_string()
}

fn default_steam_description_max_chars() -> usize {
    240
}

fn default_steam_timeout_seconds() -> u64 {
    8
}

fn default_steam_memory_cache_ttl_seconds() -> u64 {
    1800
}

fn default_steam_memory_cache_capacity() -> usize {
    512
}

fn default_steam_db_cache_ttl_seconds() -> u64 {
    86400
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CacheBackend {
    #[default]
    None,
    Sqlite,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CacheSettings {
    #[serde(default)]
    pub backend: CacheBackend,
    #[serde(default = "default_cache_sqlite_path")]
    pub sqlite_path: PathBuf,
}

impl Default for CacheSettings {
    fn default() -> Self {
        Self {
            backend: CacheBackend::None,
            sqlite_path: default_cache_sqlite_path(),
        }
    }
}

impl CacheSettings {
    fn validate(&self) -> Result<()> {
        if matches!(self.backend, CacheBackend::Sqlite) && self.sqlite_path.as_os_str().is_empty() {
            bail!("cache.sqlite_path cannot be empty when cache.backend=sqlite");
        }
        Ok(())
    }
}

fn default_cache_sqlite_path() -> PathBuf {
    PathBuf::from("./data/statushub-cache.sqlite3")
}

#[derive(Debug, Clone, Deserialize)]
pub struct StateCacheSettings {
    #[serde(default = "default_state_cache_enabled")]
    pub enabled: bool,
    #[serde(default = "default_state_cache_path")]
    pub path: PathBuf,
}

impl Default for StateCacheSettings {
    fn default() -> Self {
        Self {
            enabled: default_state_cache_enabled(),
            path: default_state_cache_path(),
        }
    }
}

impl StateCacheSettings {
    fn validate(&self) -> Result<()> {
        if self.enabled && self.path.as_os_str().is_empty() {
            bail!("state_cache.path cannot be empty when state_cache.enabled=true");
        }
        Ok(())
    }
}

fn default_state_cache_enabled() -> bool {
    true
}

fn default_state_cache_path() -> PathBuf {
    PathBuf::from("./data/status-state.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_config() {
        let raw = r#"
            [discord]
            bot_token = "discord-token"
            user_id = 123456789
            guild_id = 987654321
            emit_initial_status = true

            [webhook]
            mode = "openclaw_wake"
            url = "http://127.0.0.1:18789/hooks/wake"
            token = "secret"
            timeout_seconds = 10

            [webhook.openclaw]
            wake_mode = "now"

            [message]
            prefix = "[PREFIX]"
            suffix = "[SUFFIX]"

            [steam]
            enabled = true
            api_key = "steam-api-key"
            language = "schinese"
            description_max_chars = 200
            timeout_seconds = 5
            memory_cache_ttl_seconds = 60
            memory_cache_capacity = 128
            db_cache_ttl_seconds = 3600

            [cache]
            backend = "sqlite"
            sqlite_path = "./tmp/cache.sqlite3"

            [state_cache]
            enabled = true
            path = "./tmp/state.json"
        "#;

        let settings: Settings = toml::from_str(raw).expect("config should parse");
        settings.validate().expect("config should validate");
        assert_eq!(settings.discord.user_id, 123456789);
        assert!(settings.discord.emit_initial_status);
        assert_eq!(settings.message.prefix.as_deref(), Some("[PREFIX]"));
        assert!(settings.steam.enabled);
        assert!(matches!(settings.cache.backend, CacheBackend::Sqlite));
        assert!(settings.state_cache.enabled);
    }

    #[test]
    fn reject_invalid_webhook_url() {
        let raw = r#"
            [discord]
            bot_token = "discord-token"
            user_id = 123456789

            [webhook]
            url = "not-a-url"
        "#;

        let settings: Settings = toml::from_str(raw).expect("config should parse");
        let err = settings.validate().expect_err("config should fail");
        assert!(err.to_string().contains("webhook.url"));
    }
}
