use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as AnyhowContext, Result};
use chrono::Utc;
use serenity::all::{
    Activity, ActivityType, Client, Context, EventHandler, GatewayIntents, GuildId, OnlineStatus,
    Presence, Ready, UserId,
};
use serenity::async_trait;
use tokio::sync::{Mutex, mpsc};
use tokio::time::MissedTickBehavior;
use tracing::{debug, error, info, warn};

use crate::config::{DiscordSettings, ReminderSettings};
use crate::event::{
    DiscordActivityContext, DiscordStatus, DiscordStatusChangedEvent, ReminderContext,
};
use crate::state_cache::PersistentStatusCache;
use crate::webhook::WebhookSender;

pub async fn run(
    settings: DiscordSettings,
    reminder: ReminderSettings,
    sender: Arc<dyn WebhookSender>,
    state_cache: Option<Arc<PersistentStatusCache>>,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<DiscordStatusChangedEvent>(256);
    let target_user_id = UserId::new(settings.user_id);
    let target_guild_id = settings.guild_id.map(GuildId::new);
    let status_cache_key = make_status_cache_key(settings.user_id, settings.guild_id);
    let initial_status = if let Some(cache) = state_cache.as_ref() {
        cache.get_status(&status_cache_key).await
    } else {
        None
    };
    if let Some(status) = initial_status {
        info!(
            key = %status_cache_key,
            status = %status,
            "restored persisted status cache"
        );
    }

    let runtime_state = Arc::new(Mutex::new(build_initial_runtime_state(
        initial_status,
        settings.rich_presence_only,
        &reminder,
    )));

    let handler = PresenceEventHandler {
        target_user_id,
        target_guild_id,
        emit_initial_status: settings.emit_initial_status,
        emit_on_activity_change: settings.emit_on_activity_change,
        rich_presence_only: settings.rich_presence_only,
        reminder: reminder.clone(),
        tx: tx.clone(),
        runtime_state: runtime_state.clone(),
        state_cache,
        status_cache_key,
    };

    let reminder_loop = if reminder.enabled {
        Some(tokio::spawn(run_reminder_loop(
            reminder.clone(),
            target_user_id.get(),
            tx.clone(),
            runtime_state,
        )))
    } else {
        None
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
                    let activity_name = event.activity.as_ref().map(|a| a.name.as_str());
                    let steam_app_id = event.activity.as_ref().and_then(|a| a.steam_app_id);
                    info!(
                        user_id = event.user_id,
                        status = %event.current_status,
                        has_activity = event.activity.is_some(),
                        activity_name = ?activity_name,
                        steam_app_id = ?steam_app_id,
                        reminder = event.reminder.is_some(),
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
        emit_on_activity_change = settings.emit_on_activity_change,
        rich_presence_only = settings.rich_presence_only,
        reminder_enabled = reminder.enabled,
        reminder_interval_minutes = reminder.interval_minutes,
        reminder_steam_only = reminder.steam_only,
        "starting Discord presence monitor"
    );
    let client_result = client
        .start()
        .await
        .context("Discord client exited unexpectedly");

    if let Some(handle) = reminder_loop {
        handle.abort();
        let _ = handle.await;
    }
    drop(tx);
    drop(client);
    let _ = send_loop.await;
    client_result
}

#[derive(Debug, Clone)]
struct RuntimePresenceState {
    current_status: Option<DiscordStatus>,
    current_guild_id: Option<u64>,
    current_activity: Option<DiscordActivityContext>,
    current_activity_fingerprint: Option<String>,
    reminder_anchor: Option<ReminderAnchor>,
}

#[derive(Debug, Clone)]
struct ReminderAnchor {
    key: String,
    started_at_unix: i64,
    last_sequence: u64,
}

struct PresenceEventHandler {
    target_user_id: UserId,
    target_guild_id: Option<GuildId>,
    emit_initial_status: bool,
    emit_on_activity_change: bool,
    rich_presence_only: bool,
    reminder: ReminderSettings,
    tx: mpsc::Sender<DiscordStatusChangedEvent>,
    runtime_state: Arc<Mutex<RuntimePresenceState>>,
    state_cache: Option<Arc<PersistentStatusCache>>,
    status_cache_key: String,
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

    async fn handle_presence_update(
        &self,
        guild_id: Option<GuildId>,
        raw_status: OnlineStatus,
        activity: Option<DiscordActivityContext>,
        activity_fingerprint: String,
    ) {
        let next_status = normalize_status(raw_status);
        let now = Utc::now();
        let has_activity = !activity_fingerprint.is_empty();

        let (previous, should_emit, status_changed) = {
            let mut state = self.runtime_state.lock().await;
            let previous = state.current_status;
            let status_changed = previous != Some(next_status);
            let activity_changed = state
                .current_activity_fingerprint
                .as_ref()
                .map(|v| v != &activity_fingerprint)
                .unwrap_or(!activity_fingerprint.is_empty());

            let next_anchor_key = reminder_anchor_key(
                &self.reminder,
                self.rich_presence_only,
                next_status,
                activity.as_ref(),
            );
            let current_anchor_key = state
                .reminder_anchor
                .as_ref()
                .map(|anchor| anchor.key.as_str());
            if current_anchor_key != next_anchor_key.as_deref() {
                state.reminder_anchor = next_anchor_key.map(|key| ReminderAnchor {
                    key,
                    started_at_unix: now.timestamp(),
                    last_sequence: 0,
                });
            }

            state.current_status = Some(next_status);
            state.current_guild_id = guild_id.map(GuildId::get);
            state.current_activity = activity.clone();
            state.current_activity_fingerprint = Some(activity_fingerprint.clone());

            let should_emit = should_emit_presence_event(
                status_changed,
                activity_changed,
                self.emit_on_activity_change,
                self.rich_presence_only,
                has_activity,
                self.emit_initial_status,
                previous.is_some(),
            );
            (previous, should_emit, status_changed)
        };

        if status_changed {
            self.persist_status(next_status).await;
        }

        if previous.is_none() && !self.emit_initial_status {
            info!(status = %next_status, "captured initial status without emitting");
            return;
        }
        if !should_emit {
            return;
        }

        let event = DiscordStatusChangedEvent::new(
            self.target_user_id.get(),
            guild_id.map(GuildId::get),
            previous,
            next_status,
            activity,
            None,
        );

        if let Err(err) = self.tx.send(event).await {
            warn!(error = ?err, "status event channel closed");
        }
    }

    async fn persist_status(&self, status: DiscordStatus) {
        let Some(state_cache) = self.state_cache.as_ref() else {
            return;
        };
        if let Err(err) = state_cache
            .set_status(self.status_cache_key.clone(), status)
            .await
        {
            warn!(error = ?err, "failed to persist status cache");
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
        if let Some(target_guild_id) = self.target_guild_id {
            let initial_presence = ctx.cache.guild(target_guild_id).and_then(|guild| {
                guild.presences.get(&self.target_user_id).map(|presence| {
                    (
                        presence.status,
                        extract_activity_context(&presence.activities),
                        build_activity_fingerprint(&presence.activities),
                    )
                })
            });
            if let Some((status, activity, activity_fingerprint)) = initial_presence {
                self.handle_presence_update(
                    Some(target_guild_id),
                    status,
                    activity,
                    activity_fingerprint,
                )
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
                        build_activity_fingerprint(&presence.activities),
                    )
                })
            });
            if let Some((status, activity, activity_fingerprint)) = initial_presence {
                self.handle_presence_update(Some(guild_id), status, activity, activity_fingerprint)
                    .await;
                break;
            }
        }
    }

    async fn presence_update(&self, _: Context, new_data: Presence) {
        if !self.is_target_presence(&new_data) {
            return;
        }

        debug!(
            user_id = new_data.user.id.get(),
            guild_id = ?new_data.guild_id.map(GuildId::get),
            status = %normalize_status(new_data.status),
            activities_count = new_data.activities.len(),
            activities = %summarize_activities(&new_data.activities),
            "received presence update"
        );

        let activity = extract_activity_context(&new_data.activities);
        let activity_fingerprint = build_activity_fingerprint(&new_data.activities);
        self.handle_presence_update(
            new_data.guild_id,
            new_data.status,
            activity,
            activity_fingerprint,
        )
        .await;
    }
}

async fn run_reminder_loop(
    settings: ReminderSettings,
    target_user_id: u64,
    tx: mpsc::Sender<DiscordStatusChangedEvent>,
    runtime_state: Arc<Mutex<RuntimePresenceState>>,
) {
    let mut ticker = tokio::time::interval(Duration::from_secs(settings.check_interval_seconds));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let interval_seconds = settings.interval_seconds();

    loop {
        ticker.tick().await;
        let maybe_event = {
            let mut state = runtime_state.lock().await;
            if state.current_status.is_none() || state.reminder_anchor.is_none() {
                None
            } else {
                let current_status = state.current_status.unwrap_or(DiscordStatus::Unknown);
                let guild_id = state.current_guild_id;
                let activity = state.current_activity.clone();
                let anchor = state
                    .reminder_anchor
                    .as_mut()
                    .expect("anchor checked to exist");
                let elapsed = Utc::now()
                    .timestamp()
                    .saturating_sub(anchor.started_at_unix) as u64;
                let sequence = elapsed / interval_seconds;
                if sequence == 0 || sequence <= anchor.last_sequence {
                    None
                } else {
                    anchor.last_sequence = sequence;
                    Some(DiscordStatusChangedEvent::new(
                        target_user_id,
                        guild_id,
                        None,
                        current_status,
                        activity,
                        Some(ReminderContext {
                            elapsed_seconds: sequence.saturating_mul(interval_seconds),
                            interval_seconds,
                            sequence,
                        }),
                    ))
                }
            }
        };

        if let Some(event) = maybe_event {
            if tx.send(event).await.is_err() {
                break;
            }
        }
    }
}

fn build_initial_runtime_state(
    initial_status: Option<DiscordStatus>,
    rich_presence_only: bool,
    reminder: &ReminderSettings,
) -> RuntimePresenceState {
    let reminder_anchor = if reminder.enabled && !reminder.steam_only && !rich_presence_only {
        initial_status.map(|status| ReminderAnchor {
            key: format!("status:{status}"),
            started_at_unix: Utc::now().timestamp(),
            last_sequence: 0,
        })
    } else {
        None
    };

    RuntimePresenceState {
        current_status: initial_status,
        current_guild_id: None,
        current_activity: None,
        current_activity_fingerprint: None,
        reminder_anchor,
    }
}

fn reminder_anchor_key(
    reminder: &ReminderSettings,
    rich_presence_only: bool,
    status: DiscordStatus,
    activity: Option<&DiscordActivityContext>,
) -> Option<String> {
    if !reminder.enabled {
        return None;
    }

    if rich_presence_only && activity.is_none() {
        return None;
    }

    if reminder.steam_only {
        activity
            .and_then(|activity| activity.steam_app_id)
            .map(|app_id| format!("steam:{app_id}:{status}"))
    } else {
        Some(format!("status:{status}"))
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
    let activity = pick_primary_activity(activities)?;

    Some(DiscordActivityContext {
        name: activity.name.clone(),
        details: activity.details.clone(),
        state: activity.state.clone(),
        steam_app_id: extract_steam_app_id(activity),
    })
}

fn pick_primary_activity(activities: &[Activity]) -> Option<&Activity> {
    activities
        .iter()
        .find(|activity| activity.kind == ActivityType::Playing)
        .or_else(|| {
            activities.iter().find(|activity| {
                matches!(
                    activity.kind,
                    ActivityType::Streaming
                        | ActivityType::Listening
                        | ActivityType::Watching
                        | ActivityType::Competing
                )
            })
        })
        .or_else(|| activities.first())
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

fn make_status_cache_key(user_id: u64, guild_id: Option<u64>) -> String {
    match guild_id {
        Some(guild_id) => format!("discord:{user_id}:{guild_id}"),
        None => format!("discord:{user_id}:*"),
    }
}

fn summarize_activities(activities: &[Activity]) -> String {
    if activities.is_empty() {
        return "[]".to_string();
    }

    let parts: Vec<String> = activities
        .iter()
        .take(5)
        .map(|activity| {
            let app_id = activity.application_id.map(|id| id.get());
            let assets = activity.assets.as_ref();
            let large = assets.and_then(|a| a.large_image.as_deref()).unwrap_or("-");
            let small = assets.and_then(|a| a.small_image.as_deref()).unwrap_or("-");
            format!(
                "{{kind={:?},name={},details={:?},state={:?},app_id={:?},large_image={},small_image={}}}",
                activity.kind,
                activity.name,
                activity.details,
                activity.state,
                app_id,
                large,
                small
            )
        })
        .collect();

    if activities.len() > 5 {
        format!("[{} ... +{} more]", parts.join(", "), activities.len() - 5)
    } else {
        format!("[{}]", parts.join(", "))
    }
}

fn build_activity_fingerprint(activities: &[Activity]) -> String {
    let mut parts = Vec::with_capacity(activities.len());
    for activity in activities {
        let app_id = activity.application_id.map(|id| id.get());
        let assets = activity.assets.as_ref();
        let large = assets.and_then(|a| a.large_image.as_deref()).unwrap_or("-");
        let small = assets.and_then(|a| a.small_image.as_deref()).unwrap_or("-");
        parts.push(format!(
            "{:?}|{}|{:?}|{:?}|{:?}|{}|{}",
            activity.kind, activity.name, activity.details, activity.state, app_id, large, small
        ));
    }
    parts.join("||")
}

fn should_emit_presence_event(
    status_changed: bool,
    activity_changed: bool,
    emit_on_activity_change: bool,
    rich_presence_only: bool,
    has_activity: bool,
    emit_initial_status: bool,
    has_previous_status: bool,
) -> bool {
    let trigger_change = if rich_presence_only {
        emit_on_activity_change && activity_changed && has_activity
    } else {
        status_changed || (emit_on_activity_change && activity_changed)
    };
    trigger_change && (emit_initial_status || has_previous_status)
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

    #[test]
    fn status_cache_key_works() {
        assert_eq!(make_status_cache_key(1, Some(2)), "discord:1:2");
        assert_eq!(make_status_cache_key(1, None), "discord:1:*");
    }

    #[test]
    fn reminder_anchor_key_steam_only() {
        let settings = ReminderSettings {
            enabled: true,
            interval_minutes: 30,
            steam_only: true,
            check_interval_seconds: 30,
        };
        let key = reminder_anchor_key(
            &settings,
            false,
            DiscordStatus::Online,
            Some(&DiscordActivityContext {
                name: "Dota 2".to_string(),
                details: None,
                state: None,
                steam_app_id: Some(570),
            }),
        );
        assert_eq!(key.as_deref(), Some("steam:570:online"));
    }

    #[test]
    fn emit_on_activity_change_when_enabled() {
        assert!(should_emit_presence_event(
            false, true, true, false, true, false, true
        ));
        assert!(!should_emit_presence_event(
            false, true, false, false, true, false, true
        ));
    }

    #[test]
    fn rich_presence_only_blocks_status_only_events() {
        assert!(!should_emit_presence_event(
            true, false, true, true, false, false, true
        ));
        assert!(should_emit_presence_event(
            false, true, true, true, true, false, true
        ));
    }

    #[test]
    fn summarize_activities_empty() {
        assert_eq!(summarize_activities(&[]), "[]");
    }

    #[test]
    fn activity_fingerprint_empty_when_no_activities() {
        assert!(build_activity_fingerprint(&[]).is_empty());
    }
}
