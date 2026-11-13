//! Microsoft account login.
//!
//! The chain is the one the official launcher uses:
//!
//! 1. Microsoft device-code OAuth (a public Azure application id).
//! 2. Xbox Live user authentication.
//! 3. XSTS authorization for `rp://api.minecraftservices.com/`.
//! 4. Minecraft Services `login_with_xbox`.
//! 5. The Minecraft profile, which supplies the player name and UUID.
//!
//! Weathered does not ship an Azure client id. Create a public client and pass
//! it with `--client-id`, `WEATHERED_MS_CLIENT_ID`, or `ms_client_id` in the
//! config file. Xbox Live rejects some applications with XErr 2148916233;
//! that limitation is Microsoft's, and offline mode does not need this flow.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use super::offline::normalize_uuid;
use super::store::StoredAccount;
use super::unix_now;
use crate::error::{Error, Result};

const DEVICE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBOX_USER_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MINECRAFT_AUTH_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MINECRAFT_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

/// Scopes requested from Microsoft identity.
pub const MS_SCOPE: &str = "XboxLive.signin offline_access";

/// Device code the user enters at Microsoft's verification page.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    /// Short code shown to the user.
    pub user_code: String,
    /// Code the launcher sends while polling.
    pub device_code: String,
    /// Page where the user enters `user_code`.
    #[serde(alias = "verification_url")]
    pub verification_uri: String,
    /// Message Microsoft asked us to show. May be absent.
    pub message: Option<String>,
    /// Seconds until `device_code` expires.
    pub expires_in: u64,
    /// Minimum seconds between poll requests.
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}

/// Microsoft device-code client.
#[derive(Debug, Clone)]
pub struct MicrosoftAuth {
    client: reqwest::Client,
    client_id: String,
}

impl MicrosoftAuth {
    /// Build a client. An empty id is rejected here so the failure is obvious.
    pub fn new(client: reqwest::Client, client_id: impl Into<String>) -> Result<Self> {
        let client_id = client_id.into();
        if client_id.trim().is_empty() {
            return Err(Error::MissingClientId);
        }
        Ok(Self { client, client_id })
    }

    /// Run the device-code flow and return a Minecraft account.
    ///
    /// `on_code` is called once, before polling starts, so the UI can show
    /// the verification URL and user code.
    pub async fn login(&self, mut on_code: impl FnMut(&DeviceCode)) -> Result<StoredAccount> {
        let code = self.request_device_code().await?;
        on_code(&code);
        let token = self.poll_device_code(&code).await?;
        self.minecraft_from_ms(&token).await
    }

    /// Exchange a stored refresh token for a new Minecraft token.
    pub async fn refresh(&self, account: &StoredAccount) -> Result<StoredAccount> {
        if account.refresh_token.is_empty() {
            return Err(Error::auth(
                "this account has no refresh token. Run `weathered login` again",
            ));
        }
        let mut token = self.refresh_ms(&account.refresh_token).await?;
        if token
            .refresh_token
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            token.refresh_token = Some(account.refresh_token.clone());
        }
        self.minecraft_from_ms(&token).await
    }

    async fn request_device_code(&self) -> Result<DeviceCode> {
        let response = self
            .client
            .post(DEVICE_URL)
            .form(&[("client_id", self.client_id.as_str()), ("scope", MS_SCOPE)])
            .send()
            .await
            .map_err(|source| Error::Http {
                url: DEVICE_URL.into(),
                source,
            })?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|source| Error::Http {
            url: DEVICE_URL.into(),
            source,
        })?;
        if !status.is_success() {
            return Err(oauth_failure(DEVICE_URL, status.as_u16(), &bytes));
        }
        serde_json::from_slice(&bytes).map_err(|err| Error::json("device code response", err))
    }

    async fn poll_device_code(&self, code: &DeviceCode) -> Result<MsToken> {
        let mut interval = Duration::from_secs(code.interval.max(1));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(code.expires_in.max(30));
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::auth(
                    "the device code expired before sign-in completed",
                ));
            }
            tokio::time::sleep(interval).await;
            let response = self
                .client
                .post(TOKEN_URL)
                .form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", self.client_id.as_str()),
                    ("device_code", code.device_code.as_str()),
                ])
                .send()
                .await
                .map_err(|source| Error::Http {
                    url: TOKEN_URL.into(),
                    source,
                })?;
            let status = response.status();
            let bytes = response.bytes().await.map_err(|source| Error::Http {
                url: TOKEN_URL.into(),
                source,
            })?;
            if status.is_success() {
                return serde_json::from_slice(&bytes)
                    .map_err(|err| Error::json("Microsoft token response", err));
            }
            let failure: OAuthError = serde_json::from_slice(&bytes).unwrap_or(OAuthError {
                error: "unknown".into(),
                error_description: Some(String::from_utf8_lossy(&bytes).into_owned()),
            });
            match failure.error.as_str() {
                "authorization_pending" => continue,
                "slow_down" => {
                    interval += Duration::from_secs(5);
                    continue;
                }
                "expired_token" => {
                    return Err(Error::auth(
                        "the device code expired. Run `weathered login` again",
                    ));
                }
                "access_denied" => {
                    return Err(Error::auth("sign-in was denied"));
                }
                _ => {
                    return Err(Error::auth(format!(
                        "Microsoft login failed: {} {}",
                        failure.error,
                        failure.error_description.unwrap_or_default()
                    )));
                }
            }
        }
    }

    async fn refresh_ms(&self, refresh_token: &str) -> Result<MsToken> {
        let response = self
            .client
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("scope", MS_SCOPE),
            ])
            .send()
            .await
            .map_err(|source| Error::Http {
                url: TOKEN_URL.into(),
                source,
            })?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|source| Error::Http {
            url: TOKEN_URL.into(),
            source,
        })?;
        if !status.is_success() {
            return Err(oauth_failure(TOKEN_URL, status.as_u16(), &bytes));
        }
        serde_json::from_slice(&bytes).map_err(|err| Error::json("Microsoft refresh response", err))
    }

    async fn minecraft_from_ms(&self, ms: &MsToken) -> Result<StoredAccount> {
        let xbl = self.xbox_user(&ms.access_token).await?;
        let xsts = self.xsts(&xbl.token).await?;
        let user_hash = xsts
            .uhs
            .clone()
            .or(xbl.uhs)
            .ok_or_else(|| Error::auth("Xbox Live did not return a user hash"))?;
        let identity = minecraft_identity_token(&user_hash, &xsts.token);
        let minecraft = self.minecraft_token(&identity).await?;
        let profile = self.profile(&minecraft.access_token).await?;
        Ok(StoredAccount {
            uuid: normalize_uuid(&profile.id),
            username: profile.name,
            xuid: xsts.xid.unwrap_or_default(),
            user_hash,
            refresh_token: ms.refresh_token.clone().unwrap_or_default(),
            access_token: minecraft.access_token,
            expires_at: unix_now() + minecraft.expires_in.unwrap_or(86_400),
        })
    }

    async fn xbox_user(&self, ms_access_token: &str) -> Result<XboxAuth> {
        let body = serde_json::json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={ms_access_token}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        });
        self.post_xbox(XBOX_USER_URL, body).await
    }

    async fn xsts(&self, xbl_token: &str) -> Result<XboxAuth> {
        let body = serde_json::json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbl_token],
            },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        });
        self.post_xbox(XSTS_URL, body).await
    }

    async fn post_xbox(&self, url: &str, body: serde_json::Value) -> Result<XboxAuth> {
        let response = self
            .client
            .post(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|source| Error::Http {
                url: url.to_string(),
                source,
            })?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|source| Error::Http {
            url: url.to_string(),
            source,
        })?;
        if !status.is_success() {
            if let Ok(failure) = serde_json::from_slice::<XboxError>(&bytes) {
                if let Some(code) = failure.xerr {
                    return Err(Error::auth(xbox_error_message(code)));
                }
            }
            return Err(Error::auth(format!(
                "Xbox authentication failed with HTTP {status}: {}",
                clip_bytes(&bytes)
            )));
        }
        let parsed: XboxResponse =
            serde_json::from_slice(&bytes).map_err(|err| Error::json("Xbox response", err))?;
        let claim = parsed.display_claims.xui.into_iter().next();
        Ok(XboxAuth {
            token: parsed.token,
            uhs: claim.as_ref().and_then(|claim| claim.uhs.clone()),
            xid: claim.and_then(|claim| claim.xid),
        })
    }

    async fn minecraft_token(&self, identity_token: &str) -> Result<MinecraftToken> {
        let response = self
            .client
            .post(MINECRAFT_AUTH_URL)
            .json(&serde_json::json!({ "identityToken": identity_token }))
            .send()
            .await
            .map_err(|source| Error::Http {
                url: MINECRAFT_AUTH_URL.into(),
                source,
            })?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|source| Error::Http {
            url: MINECRAFT_AUTH_URL.into(),
            source,
        })?;
        if !status.is_success() {
            return Err(Error::auth(format!(
                "Minecraft authentication failed with HTTP {status}: {}",
                clip_bytes(&bytes)
            )));
        }
        serde_json::from_slice(&bytes).map_err(|err| Error::json("Minecraft token response", err))
    }

    async fn profile(&self, access_token: &str) -> Result<MinecraftProfile> {
        let response = self
            .client
            .get(MINECRAFT_PROFILE_URL)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|source| Error::Http {
                url: MINECRAFT_PROFILE_URL.into(),
                source,
            })?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|source| Error::Http {
            url: MINECRAFT_PROFILE_URL.into(),
            source,
        })?;
        if status.as_u16() == 404 {
            return Err(Error::auth(
                "this Microsoft account does not own Minecraft: Java Edition",
            ));
        }
        if !status.is_success() {
            return Err(Error::auth(format!(
                "could not read the Minecraft profile (HTTP {status}): {}",
                clip_bytes(&bytes)
            )));
        }
        serde_json::from_slice(&bytes).map_err(|err| Error::json("Minecraft profile", err))
    }
}

/// Identity token Minecraft Services expects.
pub fn minecraft_identity_token(user_hash: &str, xsts_token: &str) -> String {
    format!("XBL3.0 x={user_hash};{xsts_token}")
}

/// Explain an XErr code from XSTS.
pub fn xbox_error_message(code: i64) -> String {
    match code {
        2148916233 => "Xbox Live rejected this sign-in (XErr 2148916233). The account has no Xbox profile, or this Azure application is not allowed to use Xbox Live. Sign in at https://www.xbox.com once to create a profile. If the profile already exists, Microsoft has not approved this client id for Xbox Live.".into(),
        2148916235 => "Xbox Live is not available in this account's country (XErr 2148916235).".into(),
        2148916236 => "This account must complete adult verification with Xbox Live (XErr 2148916236).".into(),
        2148916237 => "This account is a child account and must be added to a Microsoft Family (XErr 2148916237).".into(),
        2148916238 => "This child account cannot sign in until a guardian approves it (XErr 2148916238).".into(),
        other => format!("Xbox Live rejected the sign-in (XErr {other})."),
    }
}

/// How to create the Azure application id `weathered login` needs.
pub fn microsoft_setup_help(config_path: &Path) -> String {
    format!(
        "\
Microsoft login needs an Azure public client id.

Set it in one of these ways:
  weathered login --client-id <application-id>
  export WEATHERED_MS_CLIENT_ID=<application-id>
  ms_client_id = \"<application-id>\" in {}

Create an application:
  1. Open https://portal.azure.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade
  2. New registration. Supported account types: Personal Microsoft accounts only.
  3. Leave the redirect URI empty. Device-code login does not use one.
  4. Copy the Application (client) ID.
  5. Authentication, then Advanced settings, then Allow public client flows: Yes.

Xbox Live may still reject an application Microsoft has not approved (XErr 2148916233).
Offline play does not need this login:
  weathered launch <version> --username <name>",
        config_path.display()
    )
}

#[derive(Debug, Deserialize)]
struct MsToken {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAuthError {
    error: String,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct XboxResponse {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    display_claims: DisplayClaims,
}

#[derive(Debug, Deserialize)]
struct DisplayClaims {
    xui: Vec<XuiClaim>,
}

#[derive(Debug, Deserialize)]
struct XuiClaim {
    uhs: Option<String>,
    xid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct XboxError {
    #[serde(rename = "XErr")]
    xerr: Option<i64>,
}

struct XboxAuth {
    token: String,
    uhs: Option<String>,
    xid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MinecraftToken {
    access_token: String,
    expires_in: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MinecraftProfile {
    id: String,
    name: String,
}

fn oauth_failure(url: &str, status: u16, bytes: &[u8]) -> Error {
    if let Ok(failure) = serde_json::from_slice::<OAuthError>(bytes) {
        return Error::auth(format!(
            "Microsoft authentication failed ({status} {}): {}",
            failure.error,
            failure.error_description.unwrap_or_default()
        ));
    }
    Error::HttpStatus {
        url: url.to_string(),
        status,
        body: clip_bytes(bytes),
    }
}

fn clip_bytes(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 300 {
        flat
    } else {
        let clipped: String = flat.chars().take(300).collect();
        format!("{clipped}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_requests_xbox_and_a_refresh_token() {
        assert!(MS_SCOPE.contains("XboxLive.signin"));
        assert!(MS_SCOPE.contains("offline_access"));
    }

    #[test]
    fn identity_token_uses_the_minecraft_services_format() {
        assert_eq!(
            minecraft_identity_token("userhash", "xsts-token"),
            "XBL3.0 x=userhash;xsts-token"
        );
    }

    #[test]
    fn xerr_2148916233_mentions_the_azure_app() {
        let message = xbox_error_message(2148916233);
        assert!(message.contains("2148916233"));
        assert!(message.contains("Azure"));
    }

    #[test]
    fn empty_client_id_is_rejected() {
        let client = reqwest::Client::new();
        let error = MicrosoftAuth::new(client, "  ").unwrap_err();
        assert!(matches!(error, Error::MissingClientId));
    }
}
