use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::{Client, Url};
use serde::Deserialize;

use crate::config::SteamSettings;

#[derive(Debug, Clone)]
pub struct SteamClient {
    client: Client,
    api_key: Option<String>,
    language: String,
    description_max_chars: usize,
}

#[derive(Debug, Clone)]
pub struct SteamGameDetails {
    pub app_id: u32,
    pub name: String,
    pub short_description: Option<String>,
    pub current_players: Option<u32>,
}

impl SteamClient {
    pub fn new(settings: &SteamSettings) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(settings.timeout_seconds))
            .user_agent("statushub/0.1")
            .build()
            .context("failed to build steam HTTP client")?;

        Ok(Self {
            client,
            api_key: settings.api_key.clone(),
            language: settings.language.clone(),
            description_max_chars: settings.description_max_chars,
        })
    }

    pub async fn fetch_game_details(&self, app_id: u32) -> Result<Option<SteamGameDetails>> {
        let mut url = Url::parse("https://store.steampowered.com/api/appdetails")
            .context("failed to parse Steam appdetails URL")?;
        url.query_pairs_mut()
            .append_pair("appids", &app_id.to_string())
            .append_pair("l", &self.language);

        let response: HashMap<String, AppDetailsEnvelope> = self
            .client
            .get(url)
            .send()
            .await
            .context("failed to query Steam appdetails API")?
            .error_for_status()
            .context("Steam appdetails API returned an error status")?
            .json()
            .await
            .context("failed to parse Steam appdetails response")?;

        let key = app_id.to_string();
        let Some(entry) = response.get(&key) else {
            return Ok(None);
        };
        if !entry.success {
            return Ok(None);
        }
        let Some(data) = entry.data.as_ref() else {
            return Ok(None);
        };

        let short_description = non_empty_trimmed(&data.short_description)
            .map(|text| truncate_chars(text, self.description_max_chars));
        let current_players = if self.api_key.is_some() {
            self.fetch_current_players(app_id).await.ok().flatten()
        } else {
            None
        };

        Ok(Some(SteamGameDetails {
            app_id,
            name: data.name.clone(),
            short_description,
            current_players,
        }))
    }

    async fn fetch_current_players(&self, app_id: u32) -> Result<Option<u32>> {
        let Some(api_key) = self.api_key.as_deref() else {
            return Ok(None);
        };

        let mut url = Url::parse(
            "https://api.steampowered.com/ISteamUserStats/GetNumberOfCurrentPlayers/v1/",
        )
        .context("failed to parse Steam current players URL")?;
        url.query_pairs_mut()
            .append_pair("key", api_key)
            .append_pair("appid", &app_id.to_string());

        let response: CurrentPlayersRoot = self
            .client
            .get(url)
            .send()
            .await
            .context("failed to query Steam current players API")?
            .error_for_status()
            .context("Steam current players API returned an error status")?
            .json()
            .await
            .context("failed to parse Steam current players response")?;

        Ok(response.response.player_count)
    }
}

#[derive(Debug, Deserialize)]
struct AppDetailsEnvelope {
    success: bool,
    data: Option<AppDetailsData>,
}

#[derive(Debug, Deserialize)]
struct AppDetailsData {
    name: String,
    #[serde(default)]
    short_description: String,
}

#[derive(Debug, Deserialize)]
struct CurrentPlayersRoot {
    response: CurrentPlayersEnvelope,
}

#[derive(Debug, Deserialize)]
struct CurrentPlayersEnvelope {
    #[serde(default)]
    player_count: Option<u32>,
}

fn non_empty_trimmed(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in input.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_description() {
        assert_eq!(truncate_chars("abcdef", 3), "abc...");
        assert_eq!(truncate_chars("abc", 3), "abc");
    }
}
