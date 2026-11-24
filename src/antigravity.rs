use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use base64::engine::general_purpose::{STANDARD, URL_SAFE};
use base64::Engine;
use reqwest::Client;
use rusqlite::Connection;
use serde_json::Value;
use uuid::Uuid;

use crate::models::{
    now_unix_seconds, AccountStore, AntigravityAccount, AntigravityIdeAccount,
    AntigravityQuotaBucket, AntigravityUsage,
};
use crate::process;
use crate::store::{save_store, write_private_atomically};

const KEYCHAIN_SERVICE: &str = "gemini";
const KEYCHAIN_DEFAULT_ACCOUNT: &str = "antigravity";
const VSCDB_OAUTH_KEY: &str = "antigravityUnifiedStateSync.oauthToken";
const VSCDB_USER_STATUS_KEY: &str = "antigravityUnifiedStateSync.userStatus";
const VSCDB_MODEL_CREDITS_KEY: &str = "antigravityUnifiedStateSync.modelCredits";
const VSCDB_PROFILE_URL_KEY: &str = "antigravity.profileUrl";

fn antigravity_base_dir() -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    let dir = dirs::config_dir();
    #[cfg(not(target_os = "linux"))]
    let dir = dirs::data_dir();
    dir.ok_or_else(|| "无法读取应用数据目录".to_string())
}

fn antigravity_support_dir() -> Result<PathBuf, String> {
    Ok(antigravity_base_dir()?.join("Antigravity"))
}

fn antigravity_ide_vscdb_path() -> Result<PathBuf, String> {
    Ok(antigravity_base_dir()?
        .join("Antigravity IDE")
        .join("User")
        .join("globalStorage")
        .join("state.vscdb"))
}

fn read_credential_user_scope() -> Option<String> {
    let path = antigravity_support_dir().ok()?.join("credential-user-scope");
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(target_os = "macos")]
fn read_keychain_secret(service: &str, account: &str) -> Option<String> {
    let out = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let secret = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if secret.is_empty() {
        None
    } else {
        Some(secret)
    }
}

#[cfg(not(target_os = "macos"))]
fn read_keychain_secret(service: &str, account: &str) -> Option<String> {
    let entry = keyring::Entry::new(service, account).ok()?;
    let secret = entry.get_password().ok()?;
    if secret.trim().is_empty() {
        None
    } else {
        Some(secret)
    }
}

#[cfg(target_os = "macos")]
fn write_keychain_secret(service: &str, account: &str, secret: &str) -> Result<(), String> {
    let out = Command::new("/usr/bin/security")
        .args([
            "add-generic-password",
            "-U",
            "-s",
            service,
            "-a",
            account,
            "-w",
            secret,
        ])
        .output()
        .map_err(|e| format!("写入 Keychain 失败: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("写入 Keychain 返回错误: {}", stderr.trim()));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn write_keychain_secret(service: &str, account: &str, secret: &str) -> Result<(), String> {
    let entry =
        keyring::Entry::new(service, account).map_err(|e| format!("初始化系统凭据库失败: {e}"))?;
    entry
        .set_password(secret)
        .map_err(|e| format!("写入系统凭据库失败: {e}"))
}

fn read_vscdb_key(key: &str) -> Option<String> {
    let path = antigravity_ide_vscdb_path().ok()?;
    if !path.is_file() {
        return None;
    }
    let conn = Connection::open(&path).ok()?;
    conn.query_row(
        "SELECT value FROM ItemTable WHERE key = ?1",
        [key],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .filter(|v| !v.is_empty())
}

fn write_vscdb_key(conn: &Connection, key: &str, value: Option<&str>) -> Result<(), String> {
    match value {
        Some(val) => {
            conn.execute(
                "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?1, ?2)",
                [key, val],
            )
            .map_err(|e| format!("写入 state.vscdb ({key}) 失败: {e}"))?;
        }
        None => {
            conn.execute("DELETE FROM ItemTable WHERE key = ?1", [key])
                .map_err(|e| format!("清理 state.vscdb ({key}) 失败: {e}"))?;
        }
    }
    Ok(())
}

fn extract_email_from_bytes(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    for token in text.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-' | '@'))) {
        if let Some((local, domain)) = token.split_once('@') {
            if !local.is_empty()
                && domain.contains('.')
                && domain.len() >= 4
                && !domain.starts_with('.')
                && !domain.ends_with('.')
            {
                return Some(token.to_string());
            }
        }
    }
    None
}

pub fn extract_email_from_vscdb_user_status(b64: &str) -> Option<String> {
    let outer = STANDARD.decode(b64.trim()).ok()?;
    if let Some(email) = extract_email_from_bytes(&outer) {
        return Some(email);
    }
    let mut start = None;
    for (i, &b) in outer.iter().enumerate() {
        let is_b64 = b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'_' | b'-');
        if is_b64 && start.is_none() {
            start = Some(i);
        } else if !is_b64 {
            if let Some(s) = start.take() {
                if i - s >= 20 {
                    if let Ok(seg) = std::str::from_utf8(&outer[s..i]) {
                        let padded = format!("{seg}{}", "=".repeat((4 - seg.len() % 4) % 4));
                        if let Ok(inner) = STANDARD
                            .decode(&padded)
                            .or_else(|_| URL_SAFE.decode(&padded))
                        {
                            if let Some(email) = extract_email_from_bytes(&inner) {
                                return Some(email);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

pub fn read_app_storage_username() -> Option<String> {
    let path = antigravity_support_dir().ok()?.join("app_storage.json");
    let raw = fs::read_to_string(path).ok()?;
    let val: Value = serde_json::from_str(&raw).ok()?;
    val.get("jetski.onboarding.lastLoginUsername")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn write_app_storage_username(email: &str) -> Result<(), String> {
    let path = antigravity_support_dir()?.join("app_storage.json");
    if !path.is_file() {
        return Ok(());
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("读取 app_storage.json 失败: {e}"))?;
    let mut val: Value =
        serde_json::from_str(&raw).map_err(|e| format!("解析 app_storage.json 失败: {e}"))?;
    if let Some(obj) = val.as_object_mut() {
        obj.insert(
            "jetski.onboarding.lastLoginUsername".to_string(),
            Value::String(email.to_string()),
        );
    }
    let serialized = serde_json::to_string(&val).map_err(|e| format!("序列化失败: {e}"))?;
    write_private_atomically(&path, serialized.as_bytes())
}

pub struct LiveAntigravityStatus {
    pub email: String,
    pub plan_name: Option<String>,
    pub usage: AntigravityUsage,
}

#[cfg(unix)]
fn discover_language_server(for_ide: bool) -> Result<(String, Vec<u16>), String> {
    let ps_out = Command::new("ps")
        .args(["-eo", "pid,args"])
        .output()
        .map_err(|e| format!("执行 ps 失败: {e}"))?;
    let ps_text = String::from_utf8_lossy(&ps_out.stdout);

    let mut target_pid: Option<String> = None;
    let mut csrf_token: Option<String> = None;

    for line in ps_text.lines() {
        if !line.contains("language_server") || !line.contains("--csrf_token") {
            continue;
        }
        let is_ide_proc = line.contains("Antigravity IDE") || line.contains("antigravity-ide");
        if for_ide != is_ide_proc {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(pid) = parts.next() else { continue };
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if let Some(pos) = tokens.iter().position(|t| *t == "--csrf_token") {
            if let Some(tok) = tokens.get(pos + 1) {
                target_pid = Some(pid.to_string());
                csrf_token = Some((*tok).to_string());
                break;
            }
        }
    }

    let (Some(pid), Some(csrf)) = (target_pid, csrf_token) else {
        return Err(if for_ide {
            "未检测到运行中的 Antigravity IDE language_server".to_string()
        } else {
            "未检测到运行中的 Antigravity language_server".to_string()
        });
    };

    let lsof_out = Command::new("lsof")
        .args(["-nP", "-a", "-p", &pid, "-iTCP", "-sTCP:LISTEN"])
        .output()
        .map_err(|e| format!("执行 lsof 失败: {e}"))?;
    let lsof_text = String::from_utf8_lossy(&lsof_out.stdout);

    let mut ports = Vec::new();
    for line in lsof_text.lines() {
        if let Some(idx) = line.find("127.0.0.1:") {
            let rest = &line[idx + "127.0.0.1:".len()..];
            let port_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(port) = port_str.parse::<u16>() {
                if !ports.contains(&port) {
                    ports.push(port);
                }
            }
        }
    }

    if ports.is_empty() {
        return Err("未找到 Antigravity language_server 监听端口".to_string());
    }

    Ok((csrf, ports))
}

#[cfg(target_os = "windows")]
fn discover_language_server(for_ide: bool) -> Result<(String, Vec<u16>), String> {
    let system = sysinfo::System::new_all();
    let mut target_pid: Option<u32> = None;
    let mut csrf_token: Option<String> = None;

    for (pid, proc) in system.processes() {
        let name = proc.name().to_ascii_lowercase();
        if !name.contains("language_server") {
            continue;
        }
        let cmd = proc.cmd();
        let joined = cmd.join(" ");
        let is_ide_proc = joined.contains("Antigravity IDE") || joined.contains("antigravity-ide");
        if for_ide != is_ide_proc {
            continue;
        }
        if let Some(pos) = cmd.iter().position(|t| t == "--csrf_token") {
            if let Some(tok) = cmd.get(pos + 1) {
                target_pid = Some(pid.as_u32());
                csrf_token = Some(tok.clone());
                break;
            }
        }
    }

    let (Some(pid), Some(csrf)) = (target_pid, csrf_token) else {
        return Err("未检测到运行中的 Antigravity language_server".to_string());
    };

    let netstat_out = Command::new("netstat")
        .args(["-ano", "-p", "TCP"])
        .output()
        .map_err(|e| format!("执行 netstat 失败: {e}"))?;
    let text = String::from_utf8_lossy(&netstat_out.stdout);
    let pid_str = pid.to_string();

    let mut ports = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 5 && cols[3].eq_ignore_ascii_case("LISTENING") && cols[4] == pid_str {
            if let Some(rest) = cols[1].strip_prefix("127.0.0.1:") {
                if let Ok(port) = rest.parse::<u16>() {
                    if !ports.contains(&port) {
                        ports.push(port);
                    }
                }
            }
        }
    }

    if ports.is_empty() {
        return Err("未找到 Antigravity language_server 监听端口".to_string());
    }

    Ok((csrf, ports))
}

fn classify_model_family(label: &str) -> &'static str {
    let lower = label.to_ascii_lowercase();
    if lower.contains("claude") {
        "Claude"
    } else if lower.contains("gemini") {
        "Gemini"
    } else if lower.contains("gpt") || lower.contains("o1") || lower.contains("o3") {
        "GPT"
    } else {
        "Other"
    }
}

async fn fetch_live_status_for(for_ide: bool) -> Result<LiveAntigravityStatus, String> {
    let (csrf, ports) = discover_language_server(for_ide)?;
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;

    let body = serde_json::json!({
        "metadata": {
            "ideName": "antigravity",
            "extensionName": "antigravity",
            "locale": "en"
        }
    });

    let mut last_err = String::new();
    for port in ports {
        for scheme in ["https", "http"] {
            let url = format!(
                "{scheme}://127.0.0.1:{port}/exa.language_server_pb.LanguageServerService/GetUserStatus"
            );
            let res = client
                .post(&url)
                .header("Connect-Protocol-Version", "1")
                .header("X-Codeium-Csrf-Token", &csrf)
                .json(&body)
                .send()
                .await;

            match res {
                Ok(resp) if resp.status().is_success() => {
                    let payload: Value = resp
                        .json()
                        .await
                        .map_err(|e| format!("解析 GetUserStatus 失败: {e}"))?;
                    return parse_user_status_payload(&payload);
                }
                Ok(resp) => {
                    last_err = format!("HTTP {}", resp.status());
                }
                Err(e) => {
                    last_err = e.to_string();
                }
            }
        }
    }

    Err(format!("读取 Antigravity 状态失败: {last_err}"))
}

pub async fn fetch_live_status() -> Result<LiveAntigravityStatus, String> {
    fetch_live_status_for(false).await
}

pub async fn fetch_ide_live_status() -> Result<LiveAntigravityStatus, String> {
    fetch_live_status_for(true).await
}

fn parse_user_status_payload(payload: &Value) -> Result<LiveAntigravityStatus, String> {
    let us = payload
        .get("userStatus")
        .ok_or_else(|| "响应缺少 userStatus".to_string())?;

    let email = us
        .get("email")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(read_app_storage_username)
        .ok_or_else(|| "无法读取 Antigravity 账号邮箱".to_string())?;

    let plan_name = us
        .get("planStatus")
        .and_then(|p| p.get("planInfo"))
        .and_then(|i| i.get("planName"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut family_map: BTreeMap<&'static str, (f64, Option<String>)> = BTreeMap::new();
    if let Some(configs) = us
        .get("cascadeModelConfigData")
        .and_then(|d| d.get("clientModelConfigs"))
        .and_then(Value::as_array)
    {
        for cfg in configs {
            let Some(label) = cfg.get("label").and_then(Value::as_str) else {
                continue;
            };
            let Some(qi) = cfg.get("quotaInfo") else {
                continue;
            };
            let Some(remaining) = qi.get("remainingFraction").and_then(Value::as_f64) else {
                continue;
            };
            let pct = (remaining * 100.0).clamp(0.0, 100.0);
            let reset_time = qi
                .get("resetTime")
                .and_then(Value::as_str)
                .map(str::to_string);
            let family = classify_model_family(label);

            family_map
                .entry(family)
                .and_modify(|(cur_pct, cur_reset)| {
                    if pct < *cur_pct {
                        *cur_pct = pct;
                        *cur_reset = reset_time.clone();
                    }
                })
                .or_insert((pct, reset_time));
        }
    }

    let buckets = family_map
        .into_iter()
        .map(|(label, (remaining_percent, reset_time))| AntigravityQuotaBucket {
            label: label.to_string(),
            remaining_percent,
            reset_time,
        })
        .collect();

    Ok(LiveAntigravityStatus {
        email,
        plan_name: plan_name.clone(),
        usage: AntigravityUsage {
            fetched_at: now_unix_seconds(),
            plan_name,
            buckets,
        },
    })
}

pub fn capture_current_account(
    store: &mut AccountStore,
    live: Option<LiveAntigravityStatus>,
) -> Result<String, String> {
    let email = live
        .as_ref()
        .map(|l| l.email.clone())
        .or_else(read_app_storage_username)
        .ok_or_else(|| "无法识别当前 Antigravity 登录账号（请先打开 Antigravity）".to_string())?;

    let keychain_secret = read_keychain_secret(KEYCHAIN_SERVICE, KEYCHAIN_DEFAULT_ACCOUNT);
    let scoped_account = read_credential_user_scope().map(|s| format!("antigravity-{s}"));
    let scoped_secret = scoped_account
        .as_deref()
        .and_then(|acct| read_keychain_secret(KEYCHAIN_SERVICE, acct));

    if keychain_secret.is_none() && scoped_secret.is_none() {
        return Err("未在系统钥匙串中找到 Antigravity 凭证".to_string());
    }

    let plan_name = live.as_ref().and_then(|l| l.plan_name.clone());
    let usage = live.map(|l| l.usage);

    if let Some(existing) = store
        .antigravity_accounts
        .iter_mut()
        .find(|a| a.email.eq_ignore_ascii_case(&email))
    {
        if keychain_secret.is_some() {
            existing.keychain_secret = keychain_secret;
        }
        if scoped_secret.is_some() {
            existing.scoped_keychain_account = scoped_account;
            existing.scoped_keychain_secret = scoped_secret;
        }
        if plan_name.is_some() {
            existing.plan_name = plan_name;
        }
        if usage.is_some() {
            existing.usage = usage;
            existing.usage_error = None;
        }
        existing.updated_at = now_unix_seconds();
        store.active_antigravity_id = Some(existing.id.clone());
        return Ok(existing.email.clone());
    }

    let id = Uuid::new_v4().to_string();
    store.antigravity_accounts.push(AntigravityAccount {
        id: id.clone(),
        email: email.clone(),
        plan_name,
        keychain_secret,
        scoped_keychain_account: scoped_account,
        scoped_keychain_secret: scoped_secret,
        vscdb_oauth_token: None,
        usage,
        usage_error: None,
        updated_at: now_unix_seconds(),
    });
    store.active_antigravity_id = Some(id);
    Ok(email)
}

pub fn switch_account(store: &mut AccountStore, target_id: &str) -> Result<String, String> {
    let target_idx = store
        .antigravity_accounts
        .iter()
        .position(|a| a.id == target_id)
        .ok_or_else(|| "找不到目标 Antigravity 账号".to_string())?;

    process::stop_antigravity_desktop()?;

    // 切换前先将当前活跃账号的最新 Keychain 凭证回写保存
    if let Some(current_email) = read_app_storage_username() {
        if let Some(active) = store
            .antigravity_accounts
            .iter_mut()
            .find(|a| a.email.eq_ignore_ascii_case(&current_email))
        {
            if let Some(secret) = read_keychain_secret(KEYCHAIN_SERVICE, KEYCHAIN_DEFAULT_ACCOUNT) {
                active.keychain_secret = Some(secret);
            }
            if let Some(scope) = read_credential_user_scope() {
                let acct = format!("antigravity-{scope}");
                if let Some(sec) = read_keychain_secret(KEYCHAIN_SERVICE, &acct) {
                    active.scoped_keychain_account = Some(acct);
                    active.scoped_keychain_secret = Some(sec);
                }
            }
            active.updated_at = now_unix_seconds();
        }
    }

    let target = &store.antigravity_accounts[target_idx];
    if let Some(secret) = target.keychain_secret.as_deref() {
        write_keychain_secret(KEYCHAIN_SERVICE, KEYCHAIN_DEFAULT_ACCOUNT, secret)?;
    }
    if let (Some(acct), Some(sec)) = (
        target.scoped_keychain_account.as_deref(),
        target.scoped_keychain_secret.as_deref(),
    ) {
        write_keychain_secret(KEYCHAIN_SERVICE, acct, sec)?;
    }
    write_app_storage_username(&target.email)?;

    let email = target.email.clone();
    store.active_antigravity_id = Some(target.id.clone());
    save_store(store)?;

    if let Some(app_path) = process::find_antigravity_app_path() {
        process::launch_app(&app_path)?;
    }

    Ok(email)
}

pub fn capture_current_ide_account(
    store: &mut AccountStore,
    live: Option<LiveAntigravityStatus>,
) -> Result<String, String> {
    let oauth_token = read_vscdb_key(VSCDB_OAUTH_KEY)
        .ok_or_else(|| "未在 Antigravity IDE state.vscdb 中找到登录凭证".to_string())?;
    let user_status = read_vscdb_key(VSCDB_USER_STATUS_KEY);
    let model_credits = read_vscdb_key(VSCDB_MODEL_CREDITS_KEY);
    let profile_url = read_vscdb_key(VSCDB_PROFILE_URL_KEY);

    let email = live
        .as_ref()
        .map(|l| l.email.clone())
        .or_else(|| {
            user_status
                .as_deref()
                .and_then(extract_email_from_vscdb_user_status)
        })
        .ok_or_else(|| "无法从 Antigravity IDE 解析当前登录邮箱".to_string())?;

    let plan_name = live.as_ref().and_then(|l| l.plan_name.clone());
    let usage = live.map(|l| l.usage);

    if let Some(existing) = store
        .antigravity_ide_accounts
        .iter_mut()
        .find(|a| a.email.eq_ignore_ascii_case(&email))
    {
        existing.oauth_token = oauth_token;
        if user_status.is_some() {
            existing.user_status = user_status;
        }
        if model_credits.is_some() {
            existing.model_credits = model_credits;
        }
        if profile_url.is_some() {
            existing.profile_url = profile_url;
        }
        if plan_name.is_some() {
            existing.plan_name = plan_name;
        }
        if usage.is_some() {
            existing.usage = usage;
            existing.usage_error = None;
        }
        existing.updated_at = now_unix_seconds();
        store.active_antigravity_ide_id = Some(existing.id.clone());
        return Ok(existing.email.clone());
    }

    let id = Uuid::new_v4().to_string();
    store.antigravity_ide_accounts.push(AntigravityIdeAccount {
        id: id.clone(),
        email: email.clone(),
        plan_name,
        oauth_token,
        user_status,
        model_credits,
        profile_url,
        usage,
        usage_error: None,
        updated_at: now_unix_seconds(),
    });
    store.active_antigravity_ide_id = Some(id);
    Ok(email)
}

pub fn switch_ide_account(store: &mut AccountStore, target_id: &str) -> Result<String, String> {
    let target_idx = store
        .antigravity_ide_accounts
        .iter()
        .position(|a| a.id == target_id)
        .ok_or_else(|| "找不到目标 Antigravity IDE 账号".to_string())?;

    process::stop_antigravity_ide_desktop()?;

    // 切换前备份当前活跃 IDE 账号的最新 state.vscdb 状态
    if let (Some(cur_oauth), Some(cur_status)) = (
        read_vscdb_key(VSCDB_OAUTH_KEY),
        read_vscdb_key(VSCDB_USER_STATUS_KEY),
    ) {
        if let Some(cur_email) = extract_email_from_vscdb_user_status(&cur_status) {
            if let Some(active) = store
                .antigravity_ide_accounts
                .iter_mut()
                .find(|a| a.email.eq_ignore_ascii_case(&cur_email))
            {
                active.oauth_token = cur_oauth;
                active.user_status = Some(cur_status);
                active.model_credits = read_vscdb_key(VSCDB_MODEL_CREDITS_KEY);
                active.profile_url = read_vscdb_key(VSCDB_PROFILE_URL_KEY);
                active.updated_at = now_unix_seconds();
            }
        }
    }

    let vscdb_path = antigravity_ide_vscdb_path()?;
    if !vscdb_path.is_file() {
        return Err(format!("未找到 Antigravity IDE 数据库: {}", vscdb_path.display()));
    }
    let conn = Connection::open(&vscdb_path).map_err(|e| format!("打开 state.vscdb 失败: {e}"))?;

    let target = &store.antigravity_ide_accounts[target_idx];
    write_vscdb_key(&conn, VSCDB_OAUTH_KEY, Some(&target.oauth_token))?;
    write_vscdb_key(&conn, VSCDB_USER_STATUS_KEY, target.user_status.as_deref())?;
    write_vscdb_key(
        &conn,
        VSCDB_MODEL_CREDITS_KEY,
        target.model_credits.as_deref(),
    )?;
    write_vscdb_key(&conn, VSCDB_PROFILE_URL_KEY, target.profile_url.as_deref())?;
    drop(conn);

    let email = target.email.clone();
    store.active_antigravity_ide_id = Some(target.id.clone());
    save_store(store)?;

    if let Some(app_path) = process::find_antigravity_ide_app_path() {
        process::launch_app(&app_path)?;
    }

    Ok(email)
}

pub async fn login_new_ide_account_and_save(store: &mut AccountStore) -> Result<String, String> {
    // 先备份当前活跃 IDE 账号
    let prev_live = fetch_ide_live_status().await.ok();
    let _ = capture_current_ide_account(store, prev_live);
    let _ = save_store(store);

    let app_path = process::find_antigravity_ide_app_path()
        .ok_or_else(|| "未找到已安装的 Antigravity IDE.app".to_string())?;

    process::stop_antigravity_ide_desktop()?;

    // 清除 state.vscdb 认证键以触发 IDE 重新登录
    let vscdb_path = antigravity_ide_vscdb_path()?;
    if vscdb_path.is_file() {
        let conn = Connection::open(&vscdb_path).map_err(|e| format!("打开 state.vscdb 失败: {e}"))?;
        write_vscdb_key(&conn, VSCDB_OAUTH_KEY, None)?;
        write_vscdb_key(&conn, VSCDB_USER_STATUS_KEY, None)?;
        write_vscdb_key(&conn, VSCDB_MODEL_CREDITS_KEY, None)?;
        write_vscdb_key(&conn, VSCDB_PROFILE_URL_KEY, None)?;
    }

    process::launch_app(&app_path)?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if tokio::time::Instant::now() >= deadline {
            return Err("等待 Antigravity IDE 新账号登录超时（3分钟）".to_string());
        }

        if read_vscdb_key(VSCDB_OAUTH_KEY).is_some()
            && read_vscdb_key(VSCDB_USER_STATUS_KEY).is_some()
        {
            let live = fetch_ide_live_status().await.ok();
            if let Ok(email) = capture_current_ide_account(store, live) {
                save_store(store)?;
                return Ok(email);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn delete_keychain_secret(service: &str, account: &str) {
    let _ = Command::new("/usr/bin/security")
        .args(["delete-generic-password", "-s", service, "-a", account])
        .output();
}

#[cfg(not(target_os = "macos"))]
fn delete_keychain_secret(service: &str, account: &str) {
    if let Ok(entry) = keyring::Entry::new(service, account) {
        let _ = entry.delete_credential();
    }
}

pub async fn login_new_account_and_save(store: &mut AccountStore) -> Result<String, String> {
    // 先备份当前活跃账号，避免凭证丢失
    let prev_live = fetch_live_status().await.ok();
    let prev_email = prev_live
        .as_ref()
        .map(|l| l.email.clone())
        .or_else(read_app_storage_username);
    if prev_email.is_some() {
        let _ = capture_current_account(store, prev_live);
        let _ = save_store(store);
    }

    let app_path = process::find_antigravity_app_path()
        .ok_or_else(|| "未找到已安装的 Antigravity.app".to_string())?;

    process::stop_antigravity_desktop()?;

    // 清除当前活跃 Keychain 条目以触发 Antigravity 重新登录
    delete_keychain_secret(KEYCHAIN_SERVICE, KEYCHAIN_DEFAULT_ACCOUNT);
    if let Some(scope) = read_credential_user_scope() {
        delete_keychain_secret(KEYCHAIN_SERVICE, &format!("antigravity-{scope}"));
    }

    process::launch_app(&app_path)?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if tokio::time::Instant::now() >= deadline {
            return Err("等待 Antigravity 新账号登录超时（3分钟）".to_string());
        }

        let has_secret = read_keychain_secret(KEYCHAIN_SERVICE, KEYCHAIN_DEFAULT_ACCOUNT).is_some()
            || read_credential_user_scope().is_some_and(|s| {
                read_keychain_secret(KEYCHAIN_SERVICE, &format!("antigravity-{s}")).is_some()
            });
        if !has_secret {
            continue;
        }

        if let Ok(live) = fetch_live_status().await {
            let email = capture_current_account(store, Some(live))?;
            save_store(store)?;
            return Ok(email);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_user_status_payload;
    use serde_json::json;

    #[test]
    fn groups_antigravity_models_by_family_and_takes_min_remaining() {
        let payload = json!({
            "userStatus": {
                "email": "tester@gmail.com",
                "planStatus": { "planInfo": { "planName": "Pro" } },
                "cascadeModelConfigData": {
                    "clientModelConfigs": [
                        {
                            "label": "Gemini 4 Neon (Medium)",
                            "quotaInfo": { "remainingFraction": 0.92, "resetTime": "2026-11-24T09:34:01Z" }
                        },
                        {
                            "label": "Gemini 3.8 Flash (High)",
                            "quotaInfo": { "remainingFraction": 0.85, "resetTime": "2026-11-24T09:00:00Z" }
                        },
                        {
                            "label": "Claude Opus 5.5 (Low)",
                            "quotaInfo": { "remainingFraction": 1.0, "resetTime": "2026-11-24T10:08:56Z" }
                        }
                    ]
                }
            }
        });

        let status = parse_user_status_payload(&payload).expect("parse status");
        assert_eq!(status.email, "tester@gmail.com");
        assert_eq!(status.plan_name.as_deref(), Some("Pro"));
        assert_eq!(status.usage.buckets.len(), 2);
        assert_eq!(status.usage.buckets[0].label, "Claude");
        assert!((status.usage.buckets[0].remaining_percent - 100.0).abs() < 0.01);
        assert_eq!(status.usage.buckets[1].label, "Gemini");
        assert!((status.usage.buckets[1].remaining_percent - 85.0).abs() < 0.01);
    }
}
