use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::warn;

use crate::cache::CacheService;
use crate::config::SteamSettings;

const STEAM_GAME_DETAILS_NAMESPACE: &str = "steam.game_details";

#[derive(Debug, Clone)]
pub struct SteamClient {
    client: Client,
    api_key: Option<String>,
    language: String,
    description_max_chars: usize,
    db_cache_ttl_seconds: u64,
    memory_cache_ttl: Duration,
    memory_cache_capacity: usize,
    memory_cache: Arc<RwLock<HashMap<u32, MemoryCacheEntry>>>,
    cache_service: Option<Arc<CacheService>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteamGameDetails {
    pub app_id: u32,
    pub name: String,
    pub short_description: Option<String>,
    pub current_players: Option<u32>,
}

#[derive(Debug, Clone)]
struct MemoryCacheEntry {
    value: SteamGameDetails,
    expires_at: Instant,
}

impl SteamClient {
    pub fn new(settings: &SteamSettings, cache_service: Option<Arc<CacheService>>) -> Result<Self> {
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
            db_cache_ttl_seconds: settings.db_cache_ttl_seconds,
            memory_cache_ttl: Duration::from_secs(settings.memory_cache_ttl_seconds),
            memory_cache_capacity: settings.memory_cache_capacity,
            memory_cache: Arc::new(RwLock::new(HashMap::new())),
            cache_service,
        })
    }

    pub async fn fetch_game_details(&self, app_id: u32) -> Result<Option<SteamGameDetails>> {
        if let Some(cached) = self.get_from_memory_cache(app_id).await {
            return Ok(Some(cached));
        }

        if let Some(cached) = self.get_from_database_cache(app_id).await {
            self.put_to_memory_cache(app_id, cached.clone()).await;
            return Ok(Some(cached));
        }

        let fetched = self.fetch_game_details_from_api(app_id).await?;
        if let Some(details) = fetched.as_ref() {
            self.put_to_memory_cache(app_id, details.clone()).await;
            self.put_to_database_cache(app_id, details).await;
        }

        Ok(fetched)
    }

    async fn fetch_game_details_from_api(&self, app_id: u32) -> Result<Option<SteamGameDetails>> {
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

    async fn get_from_memory_cache(&self, app_id: u32) -> Option<SteamGameDetails> {
        let mut cache = self.memory_cache.write().await;
        let cached = cache.get(&app_id).cloned()?;
        if cached.expires_at <= Instant::now() {
            cache.remove(&app_id);
            return None;
        }
        Some(cached.value)
    }

    async fn put_to_memory_cache(&self, app_id: u32, value: SteamGameDetails) {
        let mut cache = self.memory_cache.write().await;
        cache.retain(|_, entry| entry.expires_at > Instant::now());

        if cache.len() >= self.memory_cache_capacity && !cache.contains_key(&app_id) {
            if let Some(evict_key) = cache.keys().next().copied() {
                cache.remove(&evict_key);
            }
        }

        cache.insert(
            app_id,
            MemoryCacheEntry {
                value,
                expires_at: Instant::now() + self.memory_cache_ttl,
            },
        );
    }

    async fn get_from_database_cache(&self, app_id: u32) -> Option<SteamGameDetails> {
        let cache_service = self.cache_service.as_ref()?;
        let key = app_id.to_string();
        match cache_service
            .get_json::<SteamGameDetails>(STEAM_GAME_DETAILS_NAMESPACE, &key)
            .await
        {
            Ok(value) => value,
            Err(err) => {
                warn!(app_id, error = ?err, "failed to read Steam DB cache");
                None
            }
        }
    }

    async fn put_to_database_cache(&self, app_id: u32, value: &SteamGameDetails) {
        let Some(cache_service) = self.cache_service.as_ref() else {
            return;
        };
        let key = app_id.to_string();
        if let Err(err) = cache_service
            .set_json(
                STEAM_GAME_DETAILS_NAMESPACE,
                &key,
                value,
                Some(self.db_cache_ttl_seconds),
            )
            .await
        {
            warn!(app_id, error = ?err, "failed to write Steam DB cache");
        }
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
