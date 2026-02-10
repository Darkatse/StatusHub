mod generic;
mod openclaw;

use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Url};

use crate::cache::CacheService;
use crate::config::{MessageTemplateSettings, SteamSettings, WebhookMode, WebhookSettings};
use crate::event::DiscordStatusChangedEvent;
use crate::webhook::generic::GenericJsonSender;
use crate::webhook::openclaw::OpenClawWakeSender;

#[async_trait]
pub trait WebhookSender: Send + Sync {
    async fn send(&self, event: &DiscordStatusChangedEvent) -> Result<()>;
}

pub fn build_sender(
    settings: &WebhookSettings,
    message: &MessageTemplateSettings,
    steam: &SteamSettings,
    cache_service: Arc<CacheService>,
) -> Result<Arc<dyn WebhookSender>> {
    let shared = SharedWebhookClient::new(settings)?;

    match settings.mode {
        WebhookMode::OpenclawWake => Ok(Arc::new(OpenClawWakeSender::new(
            shared,
            settings,
            message,
            steam,
            cache_service,
        )?)),
        WebhookMode::GenericJson => Ok(Arc::new(GenericJsonSender::new(shared))),
    }
}

#[derive(Debug, Clone)]
pub struct SharedWebhookClient {
    pub client: Client,
    pub url: Url,
}

impl SharedWebhookClient {
    fn new(settings: &WebhookSettings) -> Result<Self> {
        let mut headers = HeaderMap::new();

        if let Some(token) = settings
            .token
            .as_ref()
            .filter(|token| !token.trim().is_empty())
        {
            let bearer = format!("Bearer {token}");
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&bearer).context("invalid webhook token value")?,
            );
        }

        for (raw_name, raw_value) in &settings.headers {
            let name = HeaderName::from_bytes(raw_name.as_bytes())
                .with_context(|| format!("invalid webhook header name: {raw_name}"))?;
            let value = HeaderValue::from_str(raw_value)
                .with_context(|| format!("invalid webhook header value for: {raw_name}"))?;
            headers.insert(name, value);
        }

        let client = Client::builder()
            .default_headers(headers)
            .timeout(settings.timeout())
            .build()
            .context("failed to build webhook HTTP client")?;

        let url = Url::parse(&settings.url)
            .with_context(|| format!("invalid webhook URL: {}", settings.url))?;

        Ok(Self { client, url })
    }
}
