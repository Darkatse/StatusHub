use std::fmt::{Display, Formatter};

use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
pub struct DiscordStatusChangedEvent {
    pub source: &'static str,
    pub user_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_status: Option<DiscordStatus>,
    pub current_status: DiscordStatus,
    pub observed_at: DateTime<Utc>,
}

impl DiscordStatusChangedEvent {
    pub fn new(
        user_id: u64,
        guild_id: Option<u64>,
        previous_status: Option<DiscordStatus>,
        current_status: DiscordStatus,
    ) -> Self {
        Self {
            source: "discord.status",
            user_id,
            guild_id,
            previous_status,
            current_status,
            observed_at: Utc::now(),
        }
    }

    pub fn to_openclaw_text(&self) -> String {
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
        );
        let text = event.to_openclaw_text();
        assert!(text.contains("from offline to online"));
        assert!(text.contains("guild 99"));
    }
}
