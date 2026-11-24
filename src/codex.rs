use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use uuid::Uuid;

use crate::models::{now_unix_seconds, AccountStore, CodexAccount, CodexQuotaWindow, CodexUsage};
use crate::process;
use crate::store::{save_store, write_private_atomically};

const OAUTH_ISSUER: &str = "https://auth.openai.com";
const OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OAUTH_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const OAUTH_ORIGINATOR: &str = "codex_vscode";
const OAUTH_PREFERRED_PORT: u16 = 1455;
const OAUTH_TIMEOUT_SECS: u64 = 180;
const USAGE_URLS: [&str; 2] = [
    "https://chatgpt.com/backend-api/wham/usage",
    "https://chatgpt.com/backend-api/api/codex/usage",
];

pub fn codex_auth_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "无法读取 HOME 目录".to_string())?;
    Ok(home.join(".codex").join("auth.json"))
}

pub fn read_current_auth() -> Result<Option<Value>, String> {
    let path = codex_auth_path()?;
    if !path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("读取 auth.json 失败: {e}"))?;
    let val: Value = serde_json::from_str(&raw).map_err(|e| format!("解析 auth.json 失败: {e}"))?;
    Ok(Some(val))
}

fn decode_jwt_payload(token: &str) -> Option<Value> {
    let payload_b64 = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub struct ParsedCodexAuth {
    pub account_id: String,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub exp: Option<i64>,
}

pub fn parse_auth_json(auth_json: &Value) -> Result<ParsedCodexAuth, String> {
    let tokens = auth_json
        .get("tokens")
        .and_then(Value::as_object)
        .ok_or_else(|| "auth.json 缺少 tokens 字段".to_string())?;

    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| "auth.json 缺少 access_token".to_string())?
        .to_string();

    let refresh_token = tokens
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::to_string);

    let access_claims = decode_jwt_payload(&access_token);
    let id_claims = tokens
        .get("id_token")
        .and_then(Value::as_str)
        .and_then(decode_jwt_payload);

    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            access_claims
                .as_ref()
                .and_then(|c| c.get("https://api.openai.com/auth"))
                .and_then(|a| a.get("chatgpt_account_id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| "无法解析 Codex account_id".to_string())?;

    let email = id_claims
        .as_ref()
        .and_then(|c| c.get("email"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            access_claims
                .as_ref()
                .and_then(|c| c.get("https://api.openai.com/profile"))
                .and_then(|p| p.get("email"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });

    let plan_type = access_claims
        .as_ref()
        .and_then(|c| c.get("https://api.openai.com/auth"))
        .and_then(|a| a.get("chatgpt_plan_type"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let exp = access_claims
        .as_ref()
        .and_then(|c| c.get("exp"))
        .and_then(Value::as_i64);

    Ok(ParsedCodexAuth {
        account_id,
        email,
        plan_type,
        access_token,
        refresh_token,
        exp,
    })
}

pub fn capture_current_account(store: &mut AccountStore) -> Result<String, String> {
    let auth_json = read_current_auth()?.ok_or_else(|| "未找到 ~/.codex/auth.json".to_string())?;
    let parsed = parse_auth_json(&auth_json)?;

    if let Some(existing) = store
        .codex_accounts
        .iter_mut()
        .find(|a| a.account_id == parsed.account_id)
    {
        existing.auth_json = auth_json;
        if parsed.email.is_some() {
            existing.email = parsed.email;
        }
        if parsed.plan_type.is_some() {
            existing.plan_type = parsed.plan_type;
        }
        existing.updated_at = now_unix_seconds();
        store.active_codex_id = Some(existing.id.clone());
        return Ok(existing.display_name());
    }

    let id = Uuid::new_v4().to_string();
    let account = CodexAccount {
        id: id.clone(),
        email: parsed.email,
        account_id: parsed.account_id,
        plan_type: parsed.plan_type,
        auth_json,
        usage: None,
        usage_error: None,
        updated_at: now_unix_seconds(),
    };
    let name = account.display_name();
    store.codex_accounts.push(account);
    store.active_codex_id = Some(id);
    Ok(name)
}

pub fn create_http_client() -> Result<Client, String> {
    Client::builder()
        .user_agent("ai-switcher/0.1")
        .connect_timeout(Duration::from_secs(6))
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))
}

#[derive(Debug, Deserialize)]
struct RefreshTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
}

pub async fn refresh_auth_if_needed(client: &Client, auth_json: &Value) -> Result<(Value, bool), String> {
    let parsed = parse_auth_json(auth_json)?;
    let needs_refresh = parsed
        .exp
        .is_some_and(|exp| exp <= now_unix_seconds() + 300);

    if !needs_refresh {
        return Ok((auth_json.clone(), false));
    }

    let Some(refresh_token) = parsed.refresh_token else {
        return Ok((auth_json.clone(), false));
    };

    let resp = client
        .post(OAUTH_TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", OAUTH_CLIENT_ID),
            ("refresh_token", refresh_token.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("刷新令牌网络失败: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("刷新令牌失败 HTTP {status}: {}", &body[..body.len().min(120)]));
    }

    let refreshed: RefreshTokenResponse = resp
        .json()
        .await
        .map_err(|e| format!("解析刷新响应失败: {e}"))?;

    let mut updated = auth_json.clone();
    if let Some(tokens) = updated.get_mut("tokens").and_then(Value::as_object_mut) {
        tokens.insert("access_token".to_string(), json!(refreshed.access_token));
        if let Some(rt) = refreshed.refresh_token {
            tokens.insert("refresh_token".to_string(), json!(rt));
        }
        if let Some(id_tok) = refreshed.id_token {
            tokens.insert("id_token".to_string(), json!(id_tok));
        }
    }
    if let Some(root) = updated.as_object_mut() {
        root.insert(
            "last_refresh".to_string(),
            json!(chrono::Utc::now().to_rfc3339()),
        );
    }

    Ok((updated, true))
}

#[derive(Debug, Deserialize)]
struct UsageApiResponse {
    plan_type: Option<String>,
    rate_limit: Option<RateLimitDetails>,
}

#[derive(Debug, Deserialize)]
struct RateLimitDetails {
    primary_window: Option<UsageWindowRaw>,
    secondary_window: Option<UsageWindowRaw>,
}

#[derive(Debug, Deserialize)]
struct UsageWindowRaw {
    used_percent: f64,
    limit_window_seconds: i64,
    reset_at: i64,
}

pub async fn fetch_usage(client: &Client, auth_json: &Value) -> Result<(CodexUsage, Option<Value>), String> {
    let (active_auth, changed) = refresh_auth_if_needed(client, auth_json).await?;
    let parsed = parse_auth_json(&active_auth)?;

    let mut last_err = String::new();
    for url in USAGE_URLS {
        let res = client
            .get(url)
            .bearer_auth(&parsed.access_token)
            .header("ChatGPT-Account-Id", &parsed.account_id)
            .send()
            .await;

        match res {
            Ok(resp) if resp.status().is_success() => {
                let data: UsageApiResponse = resp
                    .json()
                    .await
                    .map_err(|e| format!("解析用量响应失败: {e}"))?;

                let mut windows = Vec::new();
                if let Some(rl) = data.rate_limit {
                    if let Some(w) = rl.primary_window {
                        windows.push(w);
                    }
                    if let Some(w) = rl.secondary_window {
                        windows.push(w);
                    }
                }

                let five_hour = pick_nearest_window(&windows, 18000);
                let one_week = pick_nearest_window(&windows, 604800);

                let usage = CodexUsage {
                    fetched_at: now_unix_seconds(),
                    plan_type: data.plan_type.or(parsed.plan_type),
                    five_hour,
                    one_week,
                };
                return Ok((usage, changed.then_some(active_auth)));
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                last_err = format!("HTTP {status}: {}", &body[..body.len().min(100)]);
            }
            Err(e) => {
                last_err = e.to_string();
            }
        }
    }

    Err(format!("获取 Codex 额度失败: {last_err}"))
}

fn pick_nearest_window(windows: &[UsageWindowRaw], target_secs: i64) -> Option<CodexQuotaWindow> {
    windows
        .iter()
        .min_by_key(|w| (w.limit_window_seconds - target_secs).abs())
        .filter(|w| (w.limit_window_seconds - target_secs).abs() < target_secs / 2)
        .map(|w| CodexQuotaWindow {
            used_percent: w.used_percent,
            window_seconds: w.limit_window_seconds,
            reset_at: w.reset_at,
        })
}

pub fn switch_account(store: &mut AccountStore, target_id: &str) -> Result<String, String> {
    let target_idx = store
        .codex_accounts
        .iter()
        .position(|a| a.id == target_id)
        .ok_or_else(|| "找不到目标 Codex 账号".to_string())?;

    process::stop_codex_desktop()?;

    // 保存当前运行实例可能已轮换的最新 auth.json
    if let Ok(Some(current_auth)) = read_current_auth() {
        if let Ok(parsed) = parse_auth_json(&current_auth) {
            if let Some(active) = store
                .codex_accounts
                .iter_mut()
                .find(|a| a.account_id == parsed.account_id)
            {
                active.auth_json = current_auth;
                active.updated_at = now_unix_seconds();
            }
        }
    }

    let target = &store.codex_accounts[target_idx];
    let serialized = serde_json::to_string_pretty(&target.auth_json)
        .map_err(|e| format!("序列化目标 auth.json 失败: {e}"))?;
    let auth_path = codex_auth_path()?;
    write_private_atomically(&auth_path, serialized.as_bytes())?;

    let display = target.display_name();
    store.active_codex_id = Some(target.id.clone());
    save_store(store)?;

    if let Some(app_path) = process::find_codex_app_path() {
        process::launch_app(&app_path)?;
    }

    Ok(display)
}

pub async fn oauth_login_and_save(store: &mut AccountStore) -> Result<String, String> {
    let listener = match TcpListener::bind(("127.0.0.1", OAUTH_PREFERRED_PORT)).await {
        Ok(l) => l,
        Err(_) => TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|e| format!("启动本地授权回调监听失败: {e}"))?,
    };
    let port = listener
        .local_addr()
        .map_err(|e| format!("获取本地端口失败: {e}"))?
        .port();
    let redirect_uri = format!("http://localhost:{port}/auth/callback");

    let state = Uuid::new_v4().simple().to_string();
    let code_verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));

    let mut auth_url = reqwest::Url::parse(&format!("{OAUTH_ISSUER}/oauth/authorize"))
        .map_err(|e| format!("构造授权链接失败: {e}"))?;
    auth_url
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", OAUTH_CLIENT_ID)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", OAUTH_SCOPE)
        .append_pair("state", &state)
        .append_pair("code_challenge", &code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("originator", OAUTH_ORIGINATOR);

    Command::new("open")
        .arg(auth_url.as_str())
        .status()
        .map_err(|e| format!("打开浏览器失败: {e}"))?;

    let code = wait_for_oauth_callback(listener, &state).await?;
    let auth_json = exchange_code_for_auth(&code, &redirect_uri, &code_verifier).await?;
    let parsed = parse_auth_json(&auth_json)?;

    let display = if let Some(existing) = store
        .codex_accounts
        .iter_mut()
        .find(|a| a.account_id == parsed.account_id)
    {
        existing.auth_json = auth_json;
        if parsed.email.is_some() {
            existing.email = parsed.email;
        }
        if parsed.plan_type.is_some() {
            existing.plan_type = parsed.plan_type;
        }
        existing.usage_error = None;
        existing.updated_at = now_unix_seconds();
        existing.display_name()
    } else {
        let acc = CodexAccount {
            id: Uuid::new_v4().to_string(),
            email: parsed.email,
            account_id: parsed.account_id,
            plan_type: parsed.plan_type,
            auth_json,
            usage: None,
            usage_error: None,
            updated_at: now_unix_seconds(),
        };
        let name = acc.display_name();
        store.codex_accounts.push(acc);
        name
    };

    save_store(store)?;
    Ok(display)
}

async fn wait_for_oauth_callback(
    listener: TcpListener,
    expected_state: &str,
) -> Result<String, String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(OAUTH_TIMEOUT_SECS);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("等待浏览器授权超时（3分钟），请重试".to_string());
        }

        let (mut stream, _) = tokio::time::timeout(remaining, listener.accept())
            .await
            .map_err(|_| "等待浏览器授权超时（3分钟），请重试".to_string())?
            .map_err(|e| format!("接收回调连接失败: {e}"))?;

        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).await.unwrap_or(0);
        if n == 0 {
            continue;
        }
        let req_text = String::from_utf8_lossy(&buf[..n]);
        let first_line = req_text.lines().next().unwrap_or("");
        let path_part = first_line.split_whitespace().nth(1).unwrap_or("");

        if !path_part.starts_with("/auth/callback") {
            let _ = stream
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
            continue;
        }

        let parsed_url = reqwest::Url::parse(&format!("http://localhost{path_part}"))
            .map_err(|e| format!("解析回调 URL 失败: {e}"))?;
        let params: HashMap<String, String> = parsed_url
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        if let Some(err) = params.get("error") {
            let desc = params.get("error_description").unwrap_or(err);
            let html = format!("<html><meta charset='utf-8'><body style='font-family:system-ui;background:#16171a;color:#f3f4f6;display:flex;align-items:center;justify-content:center;height:100vh'><h3>授权失败: {desc}</h3></body></html>");
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            return Err(format!("授权被拒绝: {desc}"));
        }

        if params.get("state").map(String::as_str) != Some(expected_state) {
            return Err("回调 state 校验失败".to_string());
        }

        let Some(code) = params.get("code").cloned() else {
            return Err("回调缺少 code 参数".to_string());
        };

        let html = "<html><meta charset='utf-8'><body style='font-family:system-ui;background:#16171a;color:#86efac;display:flex;align-items:center;justify-content:center;height:100vh'><h2>授权成功，可关闭此页面并返回 AI Switcher</h2></body></html>";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
            html.len()
        );
        let _ = stream.write_all(resp.as_bytes()).await;
        return Ok(code);
    }
}

async fn exchange_code_for_auth(
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<Value, String> {
    let client = create_http_client()?;
    let resp = client
        .post(OAUTH_TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", OAUTH_CLIENT_ID),
            ("code_verifier", code_verifier),
        ])
        .send()
        .await
        .map_err(|e| format!("换取登录令牌失败: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("换取令牌失败 HTTP {status}: {}", &body[..body.len().min(120)]));
    }

    let token_resp: RefreshTokenResponse = resp
        .json()
        .await
        .map_err(|e| format!("解析令牌响应失败: {e}"))?;

    let mut tokens = serde_json::Map::new();
    tokens.insert("access_token".to_string(), json!(token_resp.access_token));
    if let Some(rt) = token_resp.refresh_token {
        tokens.insert("refresh_token".to_string(), json!(rt));
    }
    if let Some(id_tok) = token_resp.id_token {
        tokens.insert("id_token".to_string(), json!(id_tok));
    }

    let mut auth_json = json!({
        "OPENAI_API_KEY": Value::Null,
        "auth_mode": "chatgpt",
        "last_refresh": chrono::Utc::now().to_rfc3339(),
        "tokens": Value::Object(tokens),
    });

    if let Ok(parsed) = parse_auth_json(&auth_json) {
        if let Some(t) = auth_json.get_mut("tokens").and_then(Value::as_object_mut) {
            t.insert("account_id".to_string(), json!(parsed.account_id));
        }
    }

    Ok(auth_json)
}

#[cfg(test)]
mod tests {
    use super::parse_auth_json;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use serde_json::json;

    fn fake_jwt(payload: serde_json::Value) -> String {
        let h = URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let p = URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("{h}.{p}.sig")
    }

    #[test]
    fn parses_codex_auth_json_claims() {
        let access = fake_jwt(json!({
            "exp": 1999999999,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc-123",
                "chatgpt_plan_type": "plus"
            }
        }));
        let id_tok = fake_jwt(json!({ "email": "user@example.com" }));
        let auth = json!({
            "tokens": {
                "access_token": access,
                "id_token": id_tok,
                "refresh_token": "rt-xyz",
                "account_id": "acc-123"
            }
        });

        let parsed = parse_auth_json(&auth).expect("parse auth");
        assert_eq!(parsed.account_id, "acc-123");
        assert_eq!(parsed.email.as_deref(), Some("user@example.com"));
        assert_eq!(parsed.plan_type.as_deref(), Some("plus"));
        assert_eq!(parsed.refresh_token.as_deref(), Some("rt-xyz"));
    }
}
