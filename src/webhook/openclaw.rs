use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Serialize;
use tracing::warn;

use crate::cache::CacheService;
use crate::config::{MessageTemplateSettings, SteamSettings, WebhookSettings};
use crate::event::DiscordStatusChangedEvent;
use crate::steam::SteamClient;
use crate::webhook::{SharedWebhookClient, WebhookSender};

#[derive(Debug, Clone)]
pub struct OpenClawWakeSender {
    client: SharedWebhookClient,
    wake_mode: &'static str,
    prefix: Option<String>,
    suffix: Option<String>,
    steam_client: Option<SteamClient>,
}

impl OpenClawWakeSender {
    pub fn new(
        client: SharedWebhookClient,
        settings: &WebhookSettings,
        message: &MessageTemplateSettings,
        steam: &SteamSettings,
        cache_service: Arc<CacheService>,
    ) -> Result<Self> {
        let steam_client = if steam.enabled {
            Some(SteamClient::new(steam, Some(cache_service))?)
        } else {
            None
        };

        Ok(Self {
            client,
            wake_mode: settings.openclaw.wake_mode.as_str(),
            prefix: normalize_optional_text(message.prefix.clone()),
            suffix: normalize_optional_text(message.suffix.clone()),
            steam_client,
        })
    }
}

#[derive(Debug, Serialize)]
struct OpenClawWakePayload<'a> {
    text: &'a str,
    mode: &'a str,
}

#[async_trait]
impl WebhookSender for OpenClawWakeSender {
    async fn send(&self, event: &DiscordStatusChangedEvent) -> Result<()> {
        let text = self.build_text(event).await;
        let payload = OpenClawWakePayload {
            text: &text,
            mode: self.wake_mode,
        };

        let response = self
            .client
            .client
            .post(self.client.url.clone())
            .json(&payload)
            .send()
            .await
            .context("failed to call OpenClaw webhook")?;

        if response.status().is_success() {
            return Ok(());
        }

        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<failed to read response body>".to_string());
        bail!("OpenClaw webhook failed with HTTP {status}: {body}");
    }
}

impl OpenClawWakeSender {
    async fn build_text(&self, event: &DiscordStatusChangedEvent) -> String {
        let mut parts = Vec::new();
        if let Some(prefix) = &self.prefix {
            parts.push(prefix.clone());
        }

        parts.push(event.to_base_text());

        if let Some(activity_line) = build_activity_section(event) {
            parts.push(activity_line);
        }

        if let Some(steam_line) = self.build_steam_section(event).await {
            parts.push(steam_line);
        }

        if let Some(suffix) = &self.suffix {
            parts.push(suffix.clone());
        }

        parts.join("\n")
    }

    async fn build_steam_section(&self, event: &DiscordStatusChangedEvent) -> Option<String> {
        let steam_client = self.steam_client.as_ref()?;
        let activity = event.activity.as_ref()?;
        let app_id = activity.steam_app_id?;

        let fallback_name = activity.name.clone();

        match steam_client.fetch_game_details(app_id).await {
            Ok(Some(game)) => {
                let mut line = format!("Steam game: {} (app_id={})", game.name, game.app_id);
                if let Some(desc) = game.short_description {
                    line.push_str(&format!("\n简介: {desc}"));
                }
                if let Some(player_count) = game.current_players {
                    line.push_str(&format!("\n当前在线人数: {player_count}"));
                }
                Some(line)
            }
            Ok(None) => Some(format!("Steam game: {} (app_id={})", fallback_name, app_id)),
            Err(err) => {
                warn!(app_id, error = ?err, "failed to fetch Steam game details");
                Some(format!("Steam game: {} (app_id={})", fallback_name, app_id))
            }
        }
    }
}

fn build_activity_section(event: &DiscordStatusChangedEvent) -> Option<String> {
    let activity = event.activity.as_ref()?;
    let mut line = format!("Activity: {}", activity.name);
    if let Some(details) = activity
        .details
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        line.push_str(&format!("\nDetails: {details}"));
    }
    if let Some(state) = activity
        .state
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        line.push_str(&format!("\nState: {state}"));
    }
    Some(line)
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    let text = value?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{DiscordActivityContext, DiscordStatus};

    #[test]
    fn build_activity_section_contains_fields() {
        let event = DiscordStatusChangedEvent::new(
            1,
            None,
            Some(DiscordStatus::Online),
            DiscordStatus::Online,
            Some(DiscordActivityContext {
                name: "Visual Studio Code".to_string(),
                details: Some("Editing src/main.rs".to_string()),
                state: Some("Workspace: StatusHub".to_string()),
                steam_app_id: None,
            }),
            None,
        );

        let section = build_activity_section(&event).expect("section should exist");
        assert!(section.contains("Activity: Visual Studio Code"));
        assert!(section.contains("Details: Editing src/main.rs"));
        assert!(section.contains("State: Workspace: StatusHub"));
    }
}
