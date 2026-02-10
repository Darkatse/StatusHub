use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    pub discord: DiscordSettings,
    pub webhook: WebhookSettings,
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
        "#;

        let settings: Settings = toml::from_str(raw).expect("config should parse");
        settings.validate().expect("config should validate");
        assert_eq!(settings.discord.user_id, 123456789);
        assert!(settings.discord.emit_initial_status);
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
