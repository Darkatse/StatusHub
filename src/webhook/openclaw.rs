use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Serialize;

use crate::config::WebhookSettings;
use crate::event::DiscordStatusChangedEvent;
use crate::webhook::{SharedWebhookClient, WebhookSender};

#[derive(Debug, Clone)]
pub struct OpenClawWakeSender {
    client: SharedWebhookClient,
    wake_mode: &'static str,
}

impl OpenClawWakeSender {
    pub fn new(client: SharedWebhookClient, settings: &WebhookSettings) -> Self {
        Self {
            client,
            wake_mode: settings.openclaw.wake_mode.as_str(),
        }
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
        let text = event.to_openclaw_text();
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
