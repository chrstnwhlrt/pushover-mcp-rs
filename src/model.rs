use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Maximum message length in characters.
pub const MAX_MESSAGE_LENGTH: usize = 1024;

/// Notification priority level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// No alert generated. Message only visible when user opens the app.
    Silent,
    /// Quiet notification. No sound but may show on screen.
    Low,
    /// Default notification with sound and vibration.
    Normal,
    /// Important: always plays sound and vibrates, even during quiet hours.
    High,
    /// Critical: repeats every 60s for 1 hour until acknowledged. Returns receipt ID.
    Emergency,
}

impl Priority {
    /// Convert to Pushover API integer value.
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

    /// Check if this is emergency priority.
    #[must_use]
    pub const fn is_emergency(self) -> bool {
        matches!(self, Self::Emergency)
    }
}

/// Maximum title length in characters.
pub const MAX_TITLE_LENGTH: usize = 250;
/// Maximum URL length in bytes.
pub const MAX_URL_LENGTH: usize = 512;
/// Maximum URL title length in characters.
pub const MAX_URL_TITLE_LENGTH: usize = 100;
/// Minimum retry interval for emergency priority in seconds.
pub const MIN_RETRY_SECONDS: u32 = 30;
/// Maximum acknowledgment timeout for emergency priority in seconds.
pub const MAX_EXPIRE_SECONDS: u32 = 10800;

/// Parameters for sending a push notification.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendNotificationParams {
    /// The notification message (max 1024 characters)
    pub message: String,

    /// Message title (max 250 characters, defaults to app name)
    #[serde(default)]
    pub title: Option<String>,

    /// Target specific device by name. Only use if user explicitly requests a specific device.
    #[serde(default)]
    pub device: Option<String>,

    /// Notification priority level. Defaults to "normal" if not specified.
    #[serde(default)]
    pub priority: Option<Priority>,

    /// Notification sound identifier. Use list_sounds to discover built-in and custom sounds. Omit or use an empty value to use the user's default sound configured in the Pushover app.
    #[serde(default)]
    pub sound: Option<String>,

    /// Supplementary URL to include with the message (max 512 bytes, per Pushover API)
    #[serde(default)]
    pub url: Option<String>,

    /// Title for the supplementary URL, requires url to be set (max 100 characters)
    #[serde(default)]
    pub url_title: Option<String>,

    /// Set to true to enable HTML formatting (supports b, i, u, a href, font color tags). Cannot be used with monospace.
    #[serde(default)]
    pub html: Option<bool>,

    /// Set to true to display message in monospace font. Cannot be used with html.
    #[serde(default)]
    pub monospace: Option<bool>,

    /// Time until notification is auto-deleted from device, in seconds. Unrelated to emergency expire parameter.
    #[serde(default)]
    pub ttl: Option<u32>,

    /// Unix timestamp to show as sent time. Useful for delayed notifications where you want to show when the event occurred, not when delivered.
    #[serde(default)]
    pub timestamp: Option<i64>,

    /// Emergency only: retry interval in seconds (min 30, default 60). Ignored for other priorities.
    #[serde(default)]
    pub retry: Option<u32>,

    /// Emergency only: how long to keep retrying in seconds (max 10800, default 3600). Unrelated to ttl. Ignored for other priorities.
    #[serde(default)]
    pub expire: Option<u32>,
}

/// Parameters for listing user devices.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListDevicesParams {}

/// Parameters for listing notification sounds.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListSoundsParams {
    /// Set true to reload sounds from Pushover instead of using the startup cache.
    #[serde(default)]
    pub refresh: Option<bool>,
}

/// One notification sound supported by the configured Pushover application.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AvailableSound {
    #[allow(clippy::doc_markdown)] // Plain text for LLM readability in MCP schema
    /// Identifier to pass as the sound parameter of send_notification.
    pub name: String,
    /// Human-readable sound description from Pushover.
    pub description: String,
}

/// Response from Pushover sounds API.
#[derive(Debug, Deserialize)]
pub struct PushoverSoundsResponse {
    pub status: i32,
    #[serde(default)]
    pub sounds: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub errors: Option<Vec<String>>,
}

/// Response for `list_sounds` tool.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListSoundsResponse {
    /// Whether the request succeeded.
    pub success: bool,
    /// Human-readable status message.
    pub message: String,
    /// Sound identifiers and descriptions that can be used with the sound parameter of send_notification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sounds: Option<Vec<AvailableSound>>,
}

/// Response from Pushover validation API.
#[derive(Debug, Deserialize)]
pub struct PushoverValidateResponse {
    pub status: i32,
    #[serde(default)]
    pub devices: Option<Vec<String>>,
    #[serde(default)]
    pub errors: Option<Vec<String>>,
}

/// Response for `list_devices` tool.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListDevicesResponse {
    /// Whether the request succeeded.
    pub success: bool,
    /// Human-readable status message.
    pub message: String,
    #[allow(clippy::doc_markdown)] // Plain text for LLM readability in MCP schema
    /// Device names that can be used with the device parameter of send_notification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub devices: Option<Vec<String>>,
}

/// Parameters for checking emergency notification receipt status.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckReceiptParams {
    #[allow(clippy::doc_markdown)] // Plain text for LLM readability in MCP schema
    /// The receipt ID returned from send_notification when using priority="emergency".
    pub receipt: String,
}

/// Response from Pushover receipt API.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct PushoverReceiptResponse {
    pub status: i32,
    /// 1 if the user has acknowledged the notification.
    #[serde(default)]
    pub acknowledged: Option<i32>,
    /// Unix timestamp when the notification was acknowledged.
    #[serde(default)]
    pub acknowledged_at: Option<i64>,
    /// Device name that acknowledged the notification.
    #[serde(default)]
    pub acknowledged_by: Option<String>,
    /// Unix timestamp of the last delivery attempt.
    #[serde(default)]
    pub last_delivered_at: Option<i64>,
    /// 1 if the notification has expired without acknowledgment.
    #[serde(default)]
    pub expired: Option<i32>,
    /// Unix timestamp when the notification expires.
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub errors: Option<Vec<String>>,
}

/// Response for `check_emergency_receipt` tool.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CheckReceiptResponse {
    /// Whether the request succeeded.
    pub success: bool,
    /// Human-readable status message describing the acknowledgment state.
    pub message: String,
    /// Whether the notification was acknowledged by the user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged: Option<bool>,
    /// Device that acknowledged the notification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_by: Option<String>,
    /// Whether the notification has expired without acknowledgment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expired: Option<bool>,
}

/// Parameters for canceling an emergency notification.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CancelEmergencyParams {
    #[allow(clippy::doc_markdown)] // Plain text for LLM readability in MCP schema
    /// The receipt ID returned from send_notification when using priority="emergency".
    pub receipt: String,
}

/// Response for `cancel_emergency_notification` tool.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CancelEmergencyResponse {
    /// Whether the cancellation succeeded.
    pub success: bool,
    /// Human-readable status message.
    pub message: String,
}

/// Response from Pushover cancel API.
#[derive(Debug, Deserialize)]
pub struct PushoverCancelResponse {
    pub status: i32,
    #[serde(default)]
    pub errors: Option<Vec<String>>,
}

/// Response from Pushover API.
#[derive(Debug, Deserialize)]
pub struct PushoverApiResponse {
    pub status: i32,
    pub request: String,
    #[serde(default)]
    pub errors: Option<Vec<String>>,
    #[serde(default)]
    pub receipt: Option<String>,
}

/// Response for `send_notification` tool.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SendNotificationResponse {
    /// Whether the notification was sent successfully.
    pub success: bool,
    /// Human-readable status message.
    pub message: String,
    /// Pushover request ID for this API call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[allow(clippy::doc_markdown)] // Plain text for LLM readability in MCP schema
    /// Receipt ID for tracking emergency priority notifications. Only present when priority="emergency". Use with check_emergency_receipt or cancel_emergency_notification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
}
