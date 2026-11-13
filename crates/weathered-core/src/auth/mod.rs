//! Accounts used to launch the game.
//!
//! Offline mode builds a stable UUID from the username. Microsoft accounts go
//! through the device-code flow in [`microsoft`].

pub mod microsoft;
pub mod offline;
pub mod store;

pub use microsoft::MicrosoftAuth;
pub use offline::{normalize_uuid, offline_session, offline_uuid};
pub use store::{AccountStore, StoredAccount};

use std::time::{SystemTime, UNIX_EPOCH};

/// Identity passed to the game through the launch arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Player name.
    pub username: String,
    /// UUID with dashes.
    pub uuid: String,
    /// Minecraft access token. Offline launches use `0`.
    pub access_token: String,
    /// `legacy` for offline mode, `msa` for a Microsoft account.
    pub user_type: String,
    /// Xbox user id. Empty for offline mode.
    pub xuid: String,
}

pub(crate) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
