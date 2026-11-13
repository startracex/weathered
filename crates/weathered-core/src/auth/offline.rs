//! Offline usernames.
//!
//! Minecraft's offline UUID is a version-3 UUID of the UTF-8 bytes of
//! `OfflinePlayer:<username>`. It is not a random id, so the same name always
//! produces the same player data.

use md5::{Digest, Md5};

use super::Session;

/// Offline UUID for `username`, formatted with dashes.
pub fn offline_uuid(username: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(format!("OfflinePlayer:{username}").as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest);
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format_uuid(&bytes)
}

/// Session that does not contact Mojang or Microsoft.
pub fn offline_session(username: &str) -> Session {
    Session {
        username: username.to_string(),
        uuid: offline_uuid(username),
        access_token: "0".into(),
        user_type: "legacy".into(),
        xuid: String::new(),
    }
}

/// Format 16 UUID bytes as `8-4-4-4-12` lowercase hex.
pub fn format_uuid(bytes: &[u8; 16]) -> String {
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Insert dashes into a 32-digit UUID, or return `raw` when it is not one.
pub fn normalize_uuid(raw: &str) -> String {
    let hex: String = raw.chars().filter(|ch| *ch != '-').collect();
    if hex.len() != 32 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return raw.to_string();
    }
    let hex = hex.to_ascii_lowercase();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_matches_minecraft() {
        // Independent MD5 vector for OfflinePlayer:Steve.
        assert_eq!(
            offline_uuid("Steve"),
            "5627dd98-e6be-3c21-b8a8-e92344183641"
        );
        assert_eq!(offline_uuid("Steve"), offline_uuid("Steve"));
        assert_ne!(offline_uuid("Steve"), offline_uuid("Alex"));
    }

    #[test]
    fn offline_session_uses_the_legacy_user_type() {
        let session = offline_session("Steve");
        assert_eq!(session.username, "Steve");
        assert_eq!(session.access_token, "0");
        assert_eq!(session.user_type, "legacy");
        assert!(session.xuid.is_empty());
        assert_eq!(session.uuid, offline_uuid("Steve"));
    }

    #[test]
    fn normalizes_undashed_uuids() {
        assert_eq!(
            normalize_uuid("5627DD98E6BE3C21B8A8E92344183641"),
            "5627dd98-e6be-3c21-b8a8-e92344183641"
        );
        assert_eq!(normalize_uuid("not-a-uuid"), "not-a-uuid");
    }
}
