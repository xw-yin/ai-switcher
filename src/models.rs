use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Codex,
    Antigravity,
    AntigravityIde,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexQuotaWindow {
    pub used_percent: f64,
    pub window_seconds: i64,
    pub reset_at: i64,
}

impl CodexQuotaWindow {
    pub fn remaining_percent(&self) -> f64 {
        (100.0 - self.used_percent).clamp(0.0, 100.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexUsage {
    pub fetched_at: i64,
    pub plan_type: Option<String>,
    pub five_hour: Option<CodexQuotaWindow>,
    pub one_week: Option<CodexQuotaWindow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAccount {
    pub id: String,
    pub email: Option<String>,
    pub account_id: String,
    pub plan_type: Option<String>,
    pub auth_json: Value,
    pub usage: Option<CodexUsage>,
    pub usage_error: Option<String>,
    pub updated_at: i64,
}

impl CodexAccount {
    pub fn display_name(&self) -> String {
        self.email
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.account_id)
            .to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityQuotaBucket {
    pub label: String,
    pub remaining_percent: f64,
    pub reset_time: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityUsage {
    pub fetched_at: i64,
    pub plan_name: Option<String>,
    pub buckets: Vec<AntigravityQuotaBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityAccount {
    pub id: String,
    pub email: String,
    pub plan_name: Option<String>,
    pub keychain_secret: Option<String>,
    pub scoped_keychain_account: Option<String>,
    pub scoped_keychain_secret: Option<String>,
    #[serde(default)]
    pub vscdb_oauth_token: Option<String>,
    pub usage: Option<AntigravityUsage>,
    pub usage_error: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityIdeAccount {
    pub id: String,
    pub email: String,
    pub plan_name: Option<String>,
    pub oauth_token: String,
    pub user_status: Option<String>,
    pub model_credits: Option<String>,
    pub profile_url: Option<String>,
    pub usage: Option<AntigravityUsage>,
    pub usage_error: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountStore {
    #[serde(default)]
    pub codex_accounts: Vec<CodexAccount>,
    #[serde(default)]
    pub active_codex_id: Option<String>,
    #[serde(default)]
    pub antigravity_accounts: Vec<AntigravityAccount>,
    #[serde(default)]
    pub active_antigravity_id: Option<String>,
    #[serde(default)]
    pub antigravity_ide_accounts: Vec<AntigravityIdeAccount>,
    #[serde(default)]
    pub active_antigravity_ide_id: Option<String>,
}

pub fn now_unix_seconds() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn format_reset_countdown(reset_unix: i64) -> String {
    let diff = reset_unix - now_unix_seconds();
    if diff <= 0 {
        return "已重置".to_string();
    }
    let days = diff / 86400;
    let hours = (diff % 86400) / 3600;
    let mins = (diff % 3600) / 60;
    if days > 0 {
        format!("{days}天 {hours}小时")
    } else if hours > 0 {
        format!("{hours}小时 {mins}分")
    } else {
        format!("{mins}分钟")
    }
}

pub fn format_rfc3339_countdown(rfc3339: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(rfc3339) {
        Ok(dt) => format_reset_countdown(dt.timestamp()),
        Err(_) => rfc3339.to_string(),
    }
}
