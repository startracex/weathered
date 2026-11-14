//! Saved Microsoft accounts.

use serde::{Deserialize, Serialize};

use super::offline::normalize_uuid;
use super::{Session, unix_now};
use crate::config::{self, Config};
use crate::error::{Error, IoResultExt, Result};

/// A Microsoft account that has already been exchanged for a Minecraft token.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredAccount {
    /// Profile UUID, with dashes.
    pub uuid: String,
    /// Profile name.
    pub username: String,
    /// Xbox user id, when Xbox returned one.
    #[serde(default)]
    pub xuid: String,
    /// Xbox user hash used to build the identity token.
    #[serde(default)]
    pub user_hash: String,
    /// Microsoft refresh token.
    pub refresh_token: String,
    /// Minecraft access token.
    pub access_token: String,
    /// Unix time when `access_token` expires.
    pub expires_at: i64,
}

impl StoredAccount {
    /// Whether the Minecraft token should be refreshed before launch.
    pub fn is_expired(&self) -> bool {
        unix_now() + 60 >= self.expires_at
    }

    /// Session passed to the game.
    pub fn session(&self) -> Session {
        Session {
            username: self.username.clone(),
            uuid: normalize_uuid(&self.uuid),
            access_token: self.access_token.clone(),
            user_type: "msa".into(),
            xuid: self.xuid.clone(),
        }
    }
}

/// All saved accounts and the one selected by default.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AccountStore {
    /// Saved accounts.
    #[serde(default)]
    pub accounts: Vec<StoredAccount>,
    /// UUID of the account used when launch does not name one.
    #[serde(default)]
    pub active: Option<String>,
}

impl AccountStore {
    /// Load the store, or an empty one when the file is absent.
    pub fn load() -> Result<Self> {
        let path = Config::accounts_path();
        if !path.is_file() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path).io_at(&path)?;
        serde_json::from_str(&text)
            .map_err(|err| Error::message(format!("could not parse {}: {err}", path.display())))
    }

    /// Write the store with user-only permissions.
    pub fn save(&self) -> Result<()> {
        let path = Config::accounts_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).io_at(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| Error::message(format!("could not encode accounts: {err}")))?;
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, text).io_at(&tmp)?;
        config::restrict_private(&tmp);
        #[cfg(windows)]
        if path.exists() {
            std::fs::remove_file(&path).io_at(&path)?;
        }
        std::fs::rename(&tmp, &path).io_at(&path)?;
        Ok(())
    }

    /// Insert or replace an account with the same UUID.
    pub fn upsert(&mut self, account: StoredAccount) {
        if let Some(existing) = self
            .accounts
            .iter_mut()
            .find(|saved| saved.uuid == account.uuid)
        {
            *existing = account;
        } else {
            self.accounts.push(account);
        }
    }

    /// Remember which account launch should use by default.
    pub fn set_active(&mut self, uuid: &str) {
        self.active = Some(uuid.to_string());
    }

    /// Find an account by UUID or username.
    pub fn get(&self, query: &str) -> Option<&StoredAccount> {
        let query = query.trim();
        self.accounts
            .iter()
            .find(|account| account.uuid == query || account.username.eq_ignore_ascii_case(query))
    }

    /// The active account, when it is still in the store.
    pub fn active_account(&self) -> Option<&StoredAccount> {
        let id = self.active.as_deref()?;
        self.accounts.iter().find(|account| account.uuid == id)
    }

    /// Remove an account by UUID or username.
    pub fn remove(&mut self, query: &str) -> bool {
        let query = query.trim();
        let removed: Vec<String> = self
            .accounts
            .iter()
            .filter(|account| account.uuid == query || account.username.eq_ignore_ascii_case(query))
            .map(|account| account.uuid.clone())
            .collect();
        if removed.is_empty() {
            return false;
        }
        self.accounts
            .retain(|account| !removed.iter().any(|uuid| uuid == &account.uuid));
        if self
            .active
            .as_ref()
            .is_some_and(|uuid| removed.iter().any(|removed| removed == uuid))
        {
            self.active = self.accounts.first().map(|account| account.uuid.clone());
        }
        true
    }
}
