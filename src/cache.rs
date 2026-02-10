use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::task;

use crate::config::{CacheBackend, CacheSettings};

const CACHE_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS cache_entries (
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    expires_at INTEGER,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (namespace, key)
);
CREATE INDEX IF NOT EXISTS idx_cache_entries_expires_at ON cache_entries(expires_at);
"#;

#[derive(Clone, Debug, Default)]
pub struct CacheService {
    sqlite: Option<Arc<SqliteCacheStore>>,
}

impl CacheService {
    pub async fn from_settings(settings: &CacheSettings) -> Result<Self> {
        match settings.backend {
            CacheBackend::None => Ok(Self::default()),
            CacheBackend::Sqlite => {
                let store = SqliteCacheStore::new(settings.sqlite_path.clone());
                store.init().await?;
                Ok(Self {
                    sqlite: Some(Arc::new(store)),
                })
            }
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.sqlite.is_some()
    }

    pub async fn get_json<T: DeserializeOwned>(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Option<T>> {
        let Some(store) = self.sqlite.as_ref() else {
            return Ok(None);
        };

        let raw = store.get(namespace, key).await?;
        let value = match raw {
            Some(raw) => Some(serde_json::from_str(&raw).with_context(|| {
                format!("failed to deserialize cache value for {namespace}/{key}")
            })?),
            None => None,
        };
        Ok(value)
    }

    pub async fn set_json<T: Serialize>(
        &self,
        namespace: &str,
        key: &str,
        value: &T,
        ttl_seconds: Option<u64>,
    ) -> Result<()> {
        let Some(store) = self.sqlite.as_ref() else {
            return Ok(());
        };

        let raw = serde_json::to_string(value)
            .with_context(|| format!("failed to serialize cache value for {namespace}/{key}"))?;
        store.set(namespace, key, &raw, ttl_seconds).await
    }
}

#[derive(Debug)]
struct SqliteCacheStore {
    path: PathBuf,
}

impl SqliteCacheStore {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    async fn init(&self) -> Result<()> {
        let path = self.path.clone();
        task::spawn_blocking(move || -> Result<()> {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "failed to create cache directory for sqlite database: {}",
                        parent.display()
                    )
                })?;
            }

            let conn = rusqlite::Connection::open(&path)
                .with_context(|| format!("failed to open sqlite cache DB at {}", path.display()))?;
            conn.execute_batch(CACHE_TABLE_SQL)
                .context("failed to initialize sqlite cache schema")?;
            Ok(())
        })
        .await
        .context("sqlite init task join failed")??;

        Ok(())
    }

    async fn get(&self, namespace: &str, key: &str) -> Result<Option<String>> {
        let path = self.path.clone();
        let namespace = namespace.to_string();
        let key = key.to_string();

        task::spawn_blocking(move || -> Result<Option<String>> {
            let now = now_unix_seconds();
            let conn = rusqlite::Connection::open(&path)
                .with_context(|| format!("failed to open sqlite cache DB at {}", path.display()))?;

            let row: Option<(String, Option<i64>)> = conn
                .query_row(
                    "SELECT value, expires_at FROM cache_entries WHERE namespace = ?1 AND key = ?2",
                    params![namespace, key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .context("failed to query sqlite cache entry")?;

            let Some((value, expires_at)) = row else {
                return Ok(None);
            };

            if expires_at.is_some_and(|expires_at| expires_at <= now) {
                conn.execute(
                    "DELETE FROM cache_entries WHERE namespace = ?1 AND key = ?2",
                    params![namespace, key],
                )
                .context("failed to delete expired sqlite cache entry")?;
                return Ok(None);
            }

            Ok(Some(value))
        })
        .await
        .context("sqlite get task join failed")?
    }

    async fn set(
        &self,
        namespace: &str,
        key: &str,
        value: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<()> {
        let path = self.path.clone();
        let namespace = namespace.to_string();
        let key = key.to_string();
        let value = value.to_string();

        task::spawn_blocking(move || -> Result<()> {
            let now = now_unix_seconds();
            let expires_at = ttl_seconds.map(|ttl| now.saturating_add(ttl as i64));
            let conn = rusqlite::Connection::open(&path)
                .with_context(|| format!("failed to open sqlite cache DB at {}", path.display()))?;

            conn.execute(
                r#"
                INSERT INTO cache_entries (namespace, key, value, expires_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(namespace, key) DO UPDATE SET
                    value = excluded.value,
                    expires_at = excluded.expires_at,
                    updated_at = excluded.updated_at
                "#,
                params![namespace, key, value, expires_at, now],
            )
            .context("failed to upsert sqlite cache entry")?;

            conn.execute(
                "DELETE FROM cache_entries WHERE expires_at IS NOT NULL AND expires_at <= ?1",
                params![now],
            )
            .context("failed to clean expired sqlite cache entries")?;

            Ok(())
        })
        .await
        .context("sqlite set task join failed")??;

        Ok(())
    }
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn make_test_path(file: &str) -> PathBuf {
        let unique = format!("statushub_test_{}_{}", file, now_unix_seconds());
        std::env::temp_dir().join(unique)
    }

    #[tokio::test]
    async fn sqlite_cache_roundtrip() {
        let path = make_test_path("cache.sqlite3");
        let settings = CacheSettings {
            backend: CacheBackend::Sqlite,
            sqlite_path: path.clone(),
        };
        let service = CacheService::from_settings(&settings)
            .await
            .expect("cache init should succeed");

        service
            .set_json(
                "steam",
                "570",
                &serde_json::json!({"name":"Dota 2"}),
                Some(60),
            )
            .await
            .expect("cache set should succeed");
        let value: Option<serde_json::Value> = service
            .get_json("steam", "570")
            .await
            .expect("cache get should succeed");

        assert_eq!(
            value.and_then(|v| v.get("name").cloned()),
            Some("Dota 2".into())
        );
        if Path::new(&path).exists() {
            let _ = fs::remove_file(path);
        }
    }
}
