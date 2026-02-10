use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::task;
use tracing::warn;

use crate::cache::CacheService;
use crate::event::DiscordStatus;

const STATUS_CACHE_NAMESPACE: &str = "status.last";

#[derive(Debug, Clone)]
pub struct PersistentStatusCache {
    path: PathBuf,
    state: Arc<Mutex<HashMap<String, StatusRecord>>>,
    cache_service: Option<Arc<CacheService>>,
}

impl PersistentStatusCache {
    pub fn load(path: impl AsRef<Path>, cache_service: Option<Arc<CacheService>>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let state = read_file(&path)?;
        Ok(Self {
            path,
            state: Arc::new(Mutex::new(state)),
            cache_service,
        })
    }

    pub async fn get_status(&self, key: &str) -> Option<DiscordStatus> {
        {
            let state = self.state.lock().await;
            if let Some(status) = state.get(key).map(|record| record.status) {
                return Some(status);
            }
        }

        let cache_service = self.cache_service.as_ref()?;
        let db_status = match cache_service
            .get_json::<DiscordStatus>(STATUS_CACHE_NAMESPACE, key)
            .await
        {
            Ok(value) => value,
            Err(err) => {
                warn!(error = ?err, key, "failed to read status from DB cache");
                None
            }
        }?;

        let mut state = self.state.lock().await;
        state.insert(
            key.to_string(),
            StatusRecord {
                status: db_status,
                updated_at: Utc::now(),
            },
        );
        Some(db_status)
    }

    pub async fn set_status(&self, key: String, status: DiscordStatus) -> Result<()> {
        let snapshot = {
            let mut state = self.state.lock().await;
            state.insert(
                key.clone(),
                StatusRecord {
                    status,
                    updated_at: Utc::now(),
                },
            );
            state.clone()
        };

        let path = self.path.clone();
        task::spawn_blocking(move || write_file(&path, &snapshot))
            .await
            .context("status cache write task join failed")??;

        if let Some(cache_service) = self.cache_service.as_ref() {
            if let Err(err) = cache_service
                .set_json(STATUS_CACHE_NAMESPACE, &key, &status, None)
                .await
            {
                warn!(error = ?err, key, "failed to write status to DB cache");
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StatusRecord {
    status: DiscordStatus,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StatusCacheFile {
    version: u32,
    records: HashMap<String, StatusRecord>,
}

fn read_file(path: &Path) -> Result<HashMap<String, StatusRecord>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read state cache file {}", path.display()))?;
    let parsed: StatusCacheFile = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse state cache file {}", path.display()))?;
    Ok(parsed.records)
}

fn write_file(path: &Path, records: &HashMap<String, StatusRecord>) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create state cache directory: {}",
                parent.display()
            )
        })?;
    }

    let payload = StatusCacheFile {
        version: 1,
        records: records.clone(),
    };
    let json =
        serde_json::to_string_pretty(&payload).context("failed to serialize status cache")?;

    let temp_path = path.with_extension("tmp");
    fs::write(&temp_path, json).with_context(|| {
        format!(
            "failed to write temp state cache file {}",
            temp_path.display()
        )
    })?;
    if path.exists() {
        fs::remove_file(path)
            .with_context(|| format!("failed to replace state cache file {}", path.display()))?;
    }
    fs::rename(&temp_path, path).with_context(|| {
        format!(
            "failed to move temp state cache {} -> {}",
            temp_path.display(),
            path.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn persist_and_reload_status() {
        let path = std::env::temp_dir().join(format!(
            "statushub_state_{}.json",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let cache = PersistentStatusCache::load(&path, None).expect("cache load should succeed");
        cache
            .set_status("discord:1:*".to_string(), DiscordStatus::Online)
            .await
            .expect("status set should succeed");

        let reloaded = PersistentStatusCache::load(&path, None).expect("reload should succeed");
        let status = reloaded.get_status("discord:1:*").await;
        assert_eq!(status, Some(DiscordStatus::Online));

        if path.exists() {
            let _ = fs::remove_file(path);
        }
    }
}
