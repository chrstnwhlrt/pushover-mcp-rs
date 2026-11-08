//! Tool parameters and results, and the Pushover API responses they are
//! built from. The doc comments on the fields of tool types are the
//! descriptions the model reads in the JSON schemas; rmcp drops the doc
//! comment of the type itself.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

// Limits from the Pushover API documentation (https://pushover.net/api).

/// Maximum message length in characters.
pub const MAX_MESSAGE_CHARS: usize = 1024;
/// Maximum title length in characters.
pub const MAX_TITLE_CHARS: usize = 250;
/// Maximum sender length in characters; the sender starts the title, which
/// a phone cuts off after a few dozen characters.
pub const MAX_SENDER_CHARS: usize = 64;
/// Maximum supplementary URL length in characters.
pub const MAX_URL_CHARS: usize = 512;
/// Maximum URL title length in characters.
pub const MAX_URL_TITLE_CHARS: usize = 100;
/// Maximum device name length in characters.
pub const MAX_DEVICE_NAME_CHARS: usize = 25;
/// Maximum attachment size in bytes (5 MB).
pub const MAX_ATTACHMENT_BYTES: u64 = 5_242_880;
/// Minimum retry interval of emergency notifications in seconds.
pub const MIN_RETRY_SECONDS: u32 = 30;
/// Maximum time emergency notifications are retried, in seconds.
pub const MAX_EXPIRE_SECONDS: u32 = 10_800;
/// Retry interval used when an emergency notification names none.
pub const DEFAULT_RETRY_SECONDS: u32 = 60;
/// Retry duration used when an emergency notification names none.
pub const DEFAULT_EXPIRE_SECONDS: u32 = 3_600;

/// Notification priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// No alert at all; the message only shows up in the Pushover app.
    Silent,
    /// Shown without sound or vibration.
    Low,
    /// Sound and vibration as set on the device (the default). Quiet hours apply.
    Normal,
    /// Bypasses quiet hours, always plays a sound and vibrates.
    High,
    /// Like high, but repeated until acknowledged. Returns a receipt.
    Emergency,
}

impl Priority {
    /// Value of the API's `priority` parameter.
    #[must_use]
    pub const fn to_api_value(self) -> i8 {
        match self {
            Self::Silent => -2,
            Self::Low => -1,
            Self::Normal => 0,
            Self::High => 1,
            Self::Emergency => 2,
        }
    }

    /// The name the tools use.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Silent => "silent",
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
            Self::Emergency => "emergency",
        }
    }

    /// The priority with this name or API value.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "silent" | "-2" => Some(Self::Silent),
            "low" | "-1" => Some(Self::Low),
            "normal" | "0" => Some(Self::Normal),
            "high" | "1" => Some(Self::High),
            "emergency" | "2" => Some(Self::Emergency),
            _ => None,
        }
    }
}

/// Takes the names and, as models send them, Pushover's numbers -2 to 2.
impl<'de> Deserialize<'de> for Priority {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let parsed = match &value {
            serde_json::Value::String(text) => Self::parse(text),
            serde_json::Value::Number(number) => Self::parse(&number.to_string()),
            _ => None,
        };
        parsed.ok_or_else(|| {
            serde::de::Error::custom("priority must be one of silent, low, normal, high, emergency")
        })
    }
}

/// Parameters of `send_notification`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SendNotificationParams {
    /// The notification text, at most 1024 characters.
    #[schemars(length(min = 1, max = 1024))]
    pub message: String,

    // The descriptions set with `schemars(description)` are the texts measured
    // with Gemma 4 E4B in otomo (November 2026), word for word and on one line.
    #[schemars(
        length(min = 1, max = 64),
        description = "Who sends it, shown first in the title: your own name, with the session or \
                       project you work in if there is one, e.g. \"Claude (shop backend)\". At most \
                       64 characters."
    )]
    pub sender: String,

    /// Title after the sender: "Build finished" becomes "Claude (pushover mcp): Build finished".
    /// Sender and title together at most 250 characters. Omit to show only the sender.
    #[serde(default)]
    #[schemars(length(max = 247))]
    pub title: Option<String>,

    /// Priority; omit for normal.
    #[serde(default)]
    pub priority: Option<Priority>,

    #[serde(default)]
    #[schemars(
        description = "Sound name. Built in: pushover, bike (bicycle bell), bugle, cashregister, \
                       classical, cosmic, falling, gamelan, incoming, intermission, magic, \
                       mechanical, pianobar, siren, spacealarm, tugboat, alien, climb, persistent, \
                       echo, updown, vibrate (vibration only), none (silent). The account's own \
                       sounds, such as a gong, only list_sounds names. Omit for the user's default \
                       sound."
    )]
    pub sound: Option<String>,

    #[serde(default)]
    #[schemars(
        description = "Only to reach one device and not the others: its name as list_devices gives \
                       it (several joined with commas). Device names are the user's own, such as \
                       iphone15 or work-mbp, never a word like phone or laptop, nor a person: call \
                       list_devices first. Omit to reach all devices; a notification \"to my \
                       phone\" needs none."
    )]
    pub device: Option<String>,

    /// Supplementary URL shown below the message, at most 512 characters.
    #[serde(default)]
    #[schemars(length(max = 512))]
    pub url: Option<String>,

    /// Link text for url, at most 100 characters. Requires url.
    #[serde(default)]
    #[schemars(length(max = 100))]
    pub url_title: Option<String>,

    /// true renders the message as HTML: `<b>`, `<i>`, `<u>`, `<font color="#rrggbb">` and
    /// `<a href="...">` are supported. Not together with monospace.
    #[serde(default)]
    pub html: Option<bool>,

    /// true shows the message in a monospace font. Not together with html.
    #[serde(default)]
    pub monospace: Option<bool>,

    /// Absolute path of an image to attach: JPEG, PNG, GIF or WebP, at most 5 MB.
    #[serde(default)]
    pub attachment_path: Option<String>,

    /// Seconds after which the message is deleted from the devices. Ignored for emergency
    /// priority.
    #[serde(default)]
    #[schemars(range(min = 1))]
    pub ttl: Option<u32>,

    /// Unix timestamp shown as the message time instead of the time of sending, for an event
    /// that happened earlier.
    #[serde(default)]
    pub timestamp: Option<u64>,

    /// Emergency only: seconds between the repeated alerts, at least 30. Default 60.
    #[serde(default)]
    #[schemars(range(min = 30))]
    pub retry: Option<u32>,

    /// Emergency only: seconds the alert keeps repeating, at most 10800 (Pushover stops after
    /// 50 repetitions). Default 3600.
    #[serde(default)]
    #[schemars(range(min = 1, max = 10800))]
    pub expire: Option<u32>,

    /// Emergency only: tags (without commas or slashes) stored with the receipt.
    /// `cancel_emergency_notification` with one of them cancels every emergency notification
    /// that has it.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

/// Result of `send_notification`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SendNotificationResult {
    /// Pushover's ID of this request, for support inquiries.
    pub request_id: String,
    /// Only for emergency priority: the receipt for `check_emergency_receipt` and
    /// `cancel_emergency_notification`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    /// Only when the notification went out with a lower priority than asked for, because
    /// louder ones are switched off for this client: what to tell the user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Result of `list_devices`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ListDevicesResult {
    /// Names of the active devices, usable as the device parameter of `send_notification`.
    pub devices: Vec<String>,
    /// Whether the configured key is a delivery group. Pushover ignores the device parameter
    /// for groups outside Pushover for Teams.
    pub group: bool,
}

/// A notification sound.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SoundInfo {
    /// Value for the sound parameter of `send_notification`.
    pub name: String,
    /// Display name.
    pub description: String,
}

/// Result of `list_sounds`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ListSoundsResult {
    /// Built-in sounds and the custom sounds of the account, by name.
    pub sounds: Vec<SoundInfo>,
}

/// Result of `get_message_limits`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct MessageLimitsResult {
    /// Messages the account may send per month, shared by all its applications.
    pub limit: u64,
    /// Messages left this month.
    pub remaining: u64,
    /// When the count starts over (UTC, RFC 3339).
    pub reset_at: String,
}

/// Parameters of `check_emergency_receipt`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckReceiptParams {
    #[serde(default)]
    #[schemars(
        description = "Receipt of the emergency notification; omit for the latest one sent."
    )]
    pub receipt: Option<String>,
}

/// Result of `check_emergency_receipt`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CheckReceiptResult {
    /// Whether the notification was acknowledged; that ends the repetition.
    pub acknowledged: bool,
    /// When it was acknowledged (UTC, RFC 3339).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_at: Option<String>,
    /// Name of the device it was acknowledged on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_by_device: Option<String>,
    /// Whether the repetition ended without acknowledgment.
    pub expired: bool,
    /// When the repetition ends or ended (UTC, RFC 3339).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// When the notification was last delivered (UTC, RFC 3339).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_delivered_at: Option<String>,
}

/// Parameters of `cancel_emergency_notification`: a receipt, a tag or neither
/// (the latest emergency notification).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CancelEmergencyParams {
    #[serde(default)]
    #[schemars(
        description = "Receipt of the emergency notification to stop; omit receipt and tag for the \
                       latest one sent."
    )]
    pub receipt: Option<String>,
    #[serde(default)]
    #[schemars(description = "Stop every emergency notification sent with this tag.")]
    pub tag: Option<String>,
}

/// Result of `cancel_emergency_notification`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CancelEmergencyResult {
    /// What was canceled.
    pub message: String,
}

// --- Pushover API responses ---

/// The fields every Pushover API response has.
#[derive(Debug, Deserialize)]
pub struct ApiStatus {
    /// 1 on success
    pub status: i64,
    /// Request ID for support inquiries
    #[serde(default)]
    pub request: Option<String>,
    /// Why the request was rejected
    #[serde(default)]
    pub errors: Option<Vec<String>>,
}

/// Response of `messages.json`.
#[derive(Debug, Deserialize)]
pub struct MessageResponse {
    pub request: String,
    /// Only for emergency priority
    #[serde(default)]
    pub receipt: Option<String>,
}

/// Response of `users/validate.json`.
#[derive(Debug, Deserialize)]
pub struct ValidateResponse {
    #[serde(default, deserialize_with = "flag")]
    pub group: bool,
    #[serde(default)]
    pub devices: Option<Vec<String>>,
}

/// Response of `sounds.json`: sound name to display name.
#[derive(Debug, Deserialize)]
pub struct SoundsResponse {
    pub sounds: BTreeMap<String, String>,
}

/// Response of `apps/limits.json`.
#[derive(Debug, Deserialize)]
pub struct LimitsResponse {
    pub limit: u64,
    pub remaining: u64,
    /// Unix time of the next reset
    pub reset: i64,
}

/// Response of `receipts/<receipt>.json`. Timestamps are Unix times, 0 for
/// "not yet". `acknowledged_by`, the user key of whoever acknowledged, is
/// deliberately not read: user keys are private.
#[derive(Debug, Deserialize)]
pub struct ReceiptResponse {
    #[serde(default, deserialize_with = "flag")]
    pub acknowledged: bool,
    #[serde(default)]
    pub acknowledged_at: Option<i64>,
    #[serde(default)]
    pub acknowledged_by_device: Option<String>,
    #[serde(default)]
    pub last_delivered_at: Option<i64>,
    #[serde(default, deserialize_with = "flag")]
    pub expired: bool,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

/// Response of the two cancel calls.
#[derive(Debug, Deserialize)]
pub struct CancelResponse {
    /// Number of canceled notifications, if the API reports it
    #[serde(default)]
    pub canceled: Option<u64>,
}

/// Reads Pushover's 0/1 flags; also accepts booleans.
fn flag<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Bool(value) => value,
        serde_json::Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        serde_json::Value::String(text) => text == "1" || text.eq_ignore_ascii_case("true"),
        _ => false,
    })
}

impl From<ValidateResponse> for ListDevicesResult {
    fn from(response: ValidateResponse) -> Self {
        Self {
            devices: response.devices.unwrap_or_default(),
            group: response.group,
        }
    }
}

impl From<SoundsResponse> for ListSoundsResult {
    fn from(response: SoundsResponse) -> Self {
        Self {
            sounds: response
                .sounds
                .into_iter()
                .map(|(name, description)| SoundInfo { name, description })
                .collect(),
        }
    }
}

impl From<LimitsResponse> for MessageLimitsResult {
    fn from(response: LimitsResponse) -> Self {
        Self {
            limit: response.limit,
            remaining: response.remaining,
            reset_at: utc_timestamp(response.reset),
        }
    }
}

impl From<ReceiptResponse> for CheckReceiptResult {
    fn from(response: ReceiptResponse) -> Self {
        let time = |value: Option<i64>| value.filter(|&t| t > 0).map(utc_timestamp);
        Self {
            acknowledged: response.acknowledged,
            acknowledged_at: time(response.acknowledged_at),
            acknowledged_by_device: response
                .acknowledged_by_device
                .filter(|device| !device.is_empty()),
            expired: response.expired,
            expires_at: time(response.expires_at),
            last_delivered_at: time(response.last_delivered_at),
        }
    }
}

/// Formats a Unix time as RFC 3339 in UTC, e.g. `2013-02-04T23:07:18Z`.
pub fn utc_timestamp(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0).map_or_else(
        || unix.to_string(),
        |time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priorities_take_names_and_pushover_numbers() {
        for (json, expected) in [
            ("\"high\"", Priority::High),
            ("\" Emergency \"", Priority::Emergency),
            ("-2", Priority::Silent),
            ("0", Priority::Normal),
            ("\"-1\"", Priority::Low),
        ] {
            let priority: Priority = serde_json::from_str(json).unwrap();
            assert_eq!(priority, expected, "{json}");
        }
        for json in ["\"urgent\"", "3", "1.5", "true", "null"] {
            let error = serde_json::from_str::<Priority>(json).unwrap_err();
            assert_eq!(
                error.to_string(),
                "priority must be one of silent, low, normal, high, emergency",
                "{json}"
            );
        }
    }

    #[test]
    fn utc_timestamps_match_known_dates() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc_timestamp(1_360_019_238), "2013-02-04T23:07:18Z");
        assert_eq!(utc_timestamp(1_393_653_600), "2014-03-01T06:00:00Z");
        assert_eq!(utc_timestamp(1_798_761_599), "2026-12-31T23:59:59Z");
        assert_eq!(utc_timestamp(-1), "1969-12-31T23:59:59Z");
        // Out of chrono's range: the number itself.
        assert_eq!(utc_timestamp(i64::MAX), i64::MAX.to_string());
    }

    #[test]
    fn receipts_drop_zero_times_and_the_user_key() {
        let response: ReceiptResponse = serde_json::from_str(
            r#"{"status":1,"acknowledged":1,"acknowledged_at":1360019238,
                "acknowledged_by":"uQiRzpo4DXghDmr9QzzfQu27cmVRsG",
                "acknowledged_by_device":"iphone","last_delivered_at":0,
                "expired":0,"expires_at":1360019290,"called_back":0,"called_back_at":0}"#,
        )
        .unwrap();
        let result = CheckReceiptResult::from(response);
        assert!(result.acknowledged);
        assert!(!result.expired);
        assert_eq!(
            result.acknowledged_at.as_deref(),
            Some("2013-02-04T23:07:18Z")
        );
        assert_eq!(result.acknowledged_by_device.as_deref(), Some("iphone"));
        assert_eq!(result.expires_at.as_deref(), Some("2013-02-04T23:08:10Z"));
        assert_eq!(result.last_delivered_at, None);
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("uQiRzpo4DXghDmr9QzzfQu27cmVRsG"), "{json}");
    }

    #[test]
    fn flags_accept_numbers_and_booleans() {
        for (json, expected) in [
            (r#"{"group":1}"#, true),
            (r#"{"group":0}"#, false),
            (r#"{"group":true}"#, true),
            (r#"{"group":null}"#, false),
            ("{}", false),
        ] {
            let response: ValidateResponse = serde_json::from_str(json).unwrap();
            assert_eq!(response.group, expected, "{json}");
        }
    }

    #[test]
    fn sounds_are_listed_by_name() {
        let response: SoundsResponse = serde_json::from_str(
            r#"{"sounds":{"siren":"Siren","bike":"Bike","my_bell":"My Bell"},"status":1}"#,
        )
        .unwrap();
        let names: Vec<String> = ListSoundsResult::from(response)
            .sounds
            .into_iter()
            .map(|sound| sound.name)
            .collect();
        assert_eq!(names, ["bike", "my_bell", "siren"]);
    }
}
