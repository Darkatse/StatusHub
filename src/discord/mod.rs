use std::sync::Arc;

use anyhow::{Context as AnyhowContext, Result};
use serenity::all::{
    Activity, ActivityType, Client, Context, EventHandler, GatewayIntents, GuildId, OnlineStatus,
    Presence, Ready, UserId,
};
use serenity::async_trait;
use tokio::sync::{Mutex, mpsc};
use tracing::{error, info, warn};

use crate::config::DiscordSettings;
use crate::event::{DiscordActivityContext, DiscordStatus, DiscordStatusChangedEvent};
use crate::webhook::WebhookSender;

pub async fn run(settings: DiscordSettings, sender: Arc<dyn WebhookSender>) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<DiscordStatusChangedEvent>(256);
    let target_user_id = UserId::new(settings.user_id);
    let target_guild_id = settings.guild_id.map(GuildId::new);

    let handler = PresenceEventHandler {
        target_user_id,
        target_guild_id,
        emit_initial_status: settings.emit_initial_status,
        tx,
        last_status: Arc::new(Mutex::new(None)),
    };

    let intents = GatewayIntents::GUILDS | GatewayIntents::GUILD_PRESENCES;
    let mut client = Client::builder(&settings.bot_token, intents)
        .event_handler(handler)
        .await
        .context("failed to create Discord client")?;

    let send_loop = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match sender.send(&event).await {
                Ok(()) => {
                    info!(
                        user_id = event.user_id,
                        status = %event.current_status,
                        "webhook delivered"
                    );
                }
                Err(err) => {
                    error!(
                        user_id = event.user_id,
                        error = ?err,
                        "webhook delivery failed"
                    );
                }
            }
        }
    });

    info!(
        user_id = settings.user_id,
        guild_id = ?settings.guild_id,
        "starting Discord presence monitor"
    );
    let client_result = client
        .start()
        .await
        .context("Discord client exited unexpectedly");

    drop(client);
    let _ = send_loop.await;
    client_result
}

struct PresenceEventHandler {
    target_user_id: UserId,
    target_guild_id: Option<GuildId>,
    emit_initial_status: bool,
    tx: mpsc::Sender<DiscordStatusChangedEvent>,
    last_status: Arc<Mutex<Option<DiscordStatus>>>,
}

impl PresenceEventHandler {
    fn is_target_presence(&self, presence: &Presence) -> bool {
        if presence.user.id != self.target_user_id {
            return false;
        }
        match self.target_guild_id {
            Some(target_guild_id) => presence.guild_id == Some(target_guild_id),
            None => true,
        }
    }

    async fn handle_status_update(
        &self,
        guild_id: Option<GuildId>,
        raw_status: OnlineStatus,
        activity: Option<DiscordActivityContext>,
    ) {
        let next_status = normalize_status(raw_status);
        let mut last = self.last_status.lock().await;
        let previous = *last;

        if previous == Some(next_status) {
            return;
        }
        if previous.is_none() && !self.emit_initial_status {
            *last = Some(next_status);
            info!(status = %next_status, "captured initial status without emitting");
            return;
        }

        *last = Some(next_status);
        drop(last);

        let event = DiscordStatusChangedEvent::new(
            self.target_user_id.get(),
            guild_id.map(GuildId::get),
            previous,
            next_status,
            activity,
        );

        if let Err(err) = self.tx.send(event).await {
            warn!(error = ?err, "status event channel closed");
        }
    }
}

#[async_trait]
impl EventHandler for PresenceEventHandler {
    async fn ready(&self, _: Context, ready: Ready) {
        info!(
            user = %ready.user.name,
            id = ready.user.id.get(),
            "Discord gateway connected"
        );
    }

    async fn cache_ready(&self, ctx: Context, guilds: Vec<GuildId>) {
        if !self.emit_initial_status {
            return;
        }

        if let Some(target_guild_id) = self.target_guild_id {
            let initial_presence = ctx.cache.guild(target_guild_id).and_then(|guild| {
                guild.presences.get(&self.target_user_id).map(|presence| {
                    (
                        presence.status,
                        extract_activity_context(&presence.activities),
                    )
                })
            });
            if let Some((status, activity)) = initial_presence {
                self.handle_status_update(Some(target_guild_id), status, activity)
                    .await;
            }
            return;
        }

        for guild_id in guilds {
            let initial_presence = ctx.cache.guild(guild_id).and_then(|guild| {
                guild.presences.get(&self.target_user_id).map(|presence| {
                    (
                        presence.status,
                        extract_activity_context(&presence.activities),
                    )
                })
            });
            if let Some((status, activity)) = initial_presence {
                self.handle_status_update(Some(guild_id), status, activity)
                    .await;
                break;
            }
        }
    }

    async fn presence_update(&self, _: Context, new_data: Presence) {
        if !self.is_target_presence(&new_data) {
            return;
        }

        let activity = extract_activity_context(&new_data.activities);

        self.handle_status_update(new_data.guild_id, new_data.status, activity)
            .await;
    }
}

fn normalize_status(status: OnlineStatus) -> DiscordStatus {
    match status {
        OnlineStatus::Online => DiscordStatus::Online,
        OnlineStatus::Idle => DiscordStatus::Idle,
        OnlineStatus::DoNotDisturb => DiscordStatus::Dnd,
        OnlineStatus::Offline => DiscordStatus::Offline,
        OnlineStatus::Invisible => DiscordStatus::Invisible,
        _ => DiscordStatus::Unknown,
    }
}

fn extract_activity_context(activities: &[Activity]) -> Option<DiscordActivityContext> {
    let activity = activities
        .iter()
        .find(|activity| activity.kind == ActivityType::Playing)
        .or_else(|| activities.first())?;

    Some(DiscordActivityContext {
        name: activity.name.clone(),
        details: activity.details.clone(),
        state: activity.state.clone(),
        steam_app_id: extract_steam_app_id(activity),
    })
}

fn extract_steam_app_id(activity: &Activity) -> Option<u32> {
    let assets = activity.assets.as_ref()?;

    assets
        .large_image
        .as_deref()
        .and_then(parse_steam_asset_app_id)
        .or_else(|| {
            assets
                .small_image
                .as_deref()
                .and_then(parse_steam_asset_app_id)
        })
}

fn parse_steam_asset_app_id(raw: &str) -> Option<u32> {
    raw.strip_prefix("steam:")?.parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_status_values() {
        assert_eq!(
            normalize_status(OnlineStatus::Online),
            DiscordStatus::Online
        );
        assert_eq!(normalize_status(OnlineStatus::Idle), DiscordStatus::Idle);
        assert_eq!(
            normalize_status(OnlineStatus::DoNotDisturb),
            DiscordStatus::Dnd
        );
    }

    #[test]
    fn parse_steam_app_id_from_asset() {
        assert_eq!(parse_steam_asset_app_id("steam:570"), Some(570));
        assert_eq!(parse_steam_asset_app_id("foo"), None);
        assert_eq!(parse_steam_asset_app_id("steam:not-number"), None);
    }
}
