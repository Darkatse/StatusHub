use anyhow::{Context, Result, bail};
use async_trait::async_trait;

use crate::event::DiscordStatusChangedEvent;
use crate::webhook::{SharedWebhookClient, WebhookSender};

#[derive(Debug, Clone)]
pub struct GenericJsonSender {
    client: SharedWebhookClient,
}

impl GenericJsonSender {
    pub fn new(client: SharedWebhookClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl WebhookSender for GenericJsonSender {
    async fn send(&self, event: &DiscordStatusChangedEvent) -> Result<()> {
        let response = self
            .client
            .client
            .post(self.client.url.clone())
            .json(event)
            .send()
            .await
            .context("failed to call generic webhook")?;

        if response.status().is_success() {
            return Ok(());
        }

        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<failed to read response body>".to_string());
        bail!("Generic webhook failed with HTTP {status}: {body}");
    }
}
