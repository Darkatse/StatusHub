use std::fmt::{Display, Formatter};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscordStatus {
    Online,
    Idle,
    Dnd,
    Offline,
    Invisible,
    Unknown,
}

impl Display for DiscordStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Online => write!(f, "online"),
            Self::Idle => write!(f, "idle"),
            Self::Dnd => write!(f, "dnd"),
            Self::Offline => write!(f, "offline"),
            Self::Invisible => write!(f, "invisible"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordStatusChangedEvent {
    pub source: &'static str,
    pub user_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_status: Option<DiscordStatus>,
    pub current_status: DiscordStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<DiscordActivityContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reminder: Option<ReminderContext>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscordActivityContext {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steam_app_id: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReminderContext {
    pub elapsed_seconds: u64,
    pub interval_seconds: u64,
    pub sequence: u64,
}

impl DiscordStatusChangedEvent {
    pub fn new(
        user_id: u64,
        guild_id: Option<u64>,
        previous_status: Option<DiscordStatus>,
        current_status: DiscordStatus,
        activity: Option<DiscordActivityContext>,
        reminder: Option<ReminderContext>,
    ) -> Self {
        Self {
            source: "discord.status",
            user_id,
            guild_id,
            previous_status,
            current_status,
            activity,
            reminder,
            observed_at: Utc::now(),
        }
    }

    pub fn to_base_text(&self) -> String {
        if let Some(reminder) = &self.reminder {
            let elapsed = format_elapsed(reminder.elapsed_seconds);
            return match self.guild_id {
                Some(guild_id) => format!(
                    "Discord status reminder: user {} in guild {} is still {}. Elapsed: {} (reminder #{}) at {}",
                    self.user_id,
                    guild_id,
                    self.current_status,
                    elapsed,
                    reminder.sequence,
                    self.observed_at.to_rfc3339()
                ),
                None => format!(
                    "Discord status reminder: user {} is still {}. Elapsed: {} (reminder #{}) at {}",
                    self.user_id,
                    self.current_status,
                    elapsed,
                    reminder.sequence,
                    self.observed_at.to_rfc3339()
                ),
            };
        }

        if self.previous_status == Some(self.current_status) && self.activity.is_some() {
            return match self.guild_id {
                Some(guild_id) => format!(
                    "Discord activity changed: user {} in guild {} (status {}) at {}",
                    self.user_id,
                    guild_id,
                    self.current_status,
                    self.observed_at.to_rfc3339()
                ),
                None => format!(
                    "Discord activity changed: user {} (status {}) at {}",
                    self.user_id,
                    self.current_status,
                    self.observed_at.to_rfc3339()
                ),
            };
        }

        let old = self
            .previous_status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        match self.guild_id {
            Some(guild_id) => format!(
                "Discord status changed: user {} in guild {} from {} to {} at {}",
                self.user_id,
                guild_id,
                old,
                self.current_status,
                self.observed_at.to_rfc3339()
            ),
            None => format!(
                "Discord status changed: user {} from {} to {} at {}",
                self.user_id,
                old,
                self.current_status,
                self.observed_at.to_rfc3339()
            ),
        }
    }
}

fn format_elapsed(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if hours > 0 {
        format!("{hours}h {minutes}m {secs}s")
    } else if minutes > 0 {
        format!("{minutes}m {secs}s")
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openclaw_text_contains_status_transition() {
        let event = DiscordStatusChangedEvent::new(
            42,
            Some(99),
            Some(DiscordStatus::Offline),
            DiscordStatus::Online,
            None,
            None,
        );
        let text = event.to_base_text();
        assert!(text.contains("from offline to online"));
        assert!(text.contains("guild 99"));
    }

    #[test]
    fn reminder_text_contains_elapsed() {
        let event = DiscordStatusChangedEvent::new(
            42,
            None,
            None,
            DiscordStatus::Online,
            None,
            Some(ReminderContext {
                elapsed_seconds: 1800,
                interval_seconds: 1800,
                sequence: 1,
            }),
        );
        let text = event.to_base_text();
        assert!(text.contains("status reminder"));
        assert!(text.contains("30m"));
    }

    #[test]
    fn activity_change_text_for_same_status() {
        let event = DiscordStatusChangedEvent::new(
            42,
            None,
            Some(DiscordStatus::Online),
            DiscordStatus::Online,
            Some(DiscordActivityContext {
                name: "Cyberpunk 2077".to_string(),
                details: None,
                state: None,
                steam_app_id: Some(1091500),
            }),
            None,
        );
        let text = event.to_base_text();
        assert!(text.contains("activity changed"));
    }
}
