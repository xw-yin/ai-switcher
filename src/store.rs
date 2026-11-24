use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::Value;
use uuid::Uuid;

use crate::codex;
use crate::models::{now_unix_seconds, AccountStore, AntigravityAccount, CodexAccount};

const APP_DIR_NAME: &str = "com.xwyin.ai-switcher";
const STORE_FILE_NAME: &str = "accounts.json";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ImportSummary {
    pub codex_added: usize,
    pub codex_updated: usize,
    pub antigravity_added: usize,
    pub antigravity_updated: usize,
}

impl ImportSummary {
    pub fn format_message(&self) -> String {
        let total = self.codex_added
            + self.codex_updated
            + self.antigravity_added
            + self.antigravity_updated;
        if total == 0 {
            return "未发现可导入的账号".to_string();
        }
        format!(
            "导入完成: Codex +{} / 更新 {}，Antigravity +{} / 更新 {}",
            self.codex_added,
            self.codex_updated,
            self.antigravity_added,
            self.antigravity_updated
        )
    }
}

pub fn store_path() -> Result<PathBuf, String> {
    let data_dir = dirs::data_dir().ok_or_else(|| "无法读取 Application Support 目录".to_string())?;
    Ok(data_dir.join(APP_DIR_NAME).join(STORE_FILE_NAME))
}

pub fn load_store() -> Result<AccountStore, String> {
    let path = store_path()?;
    if !path.is_file() {
        return Ok(AccountStore::default());
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("读取存储失败: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("解析存储失败: {e}"))
}

pub fn save_store(store: &AccountStore) -> Result<(), String> {
    let path = store_path()?;
    export_store_to_path(store, &path)
}

pub fn export_store_to_path(store: &AccountStore, path: &Path) -> Result<(), String> {
    let serialized =
        serde_json::to_string_pretty(store).map_err(|e| format!("序列化存储失败: {e}"))?;
    write_private_atomically(path, serialized.as_bytes())
}

pub fn write_private_atomically(path: &Path, content: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("无效文件路径: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;

    let tmp_path = parent.join(format!(".tmp-{}", Uuid::new_v4().simple()));
    {
        let mut file =
            fs::File::create(&tmp_path).map_err(|e| format!("创建临时文件失败: {e}"))?;
        file.write_all(content)
            .map_err(|e| format!("写入文件失败: {e}"))?;
        file.sync_all().map_err(|e| format!("刷盘失败: {e}"))?;
    }
    let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600));
    fs::rename(&tmp_path, path).map_err(|e| format!("替换文件失败: {e}"))?;
    Ok(())
}

fn upsert_codex_account(store: &mut AccountStore, incoming: CodexAccount, summary: &mut ImportSummary) {
    if let Some(existing) = store
        .codex_accounts
        .iter_mut()
        .find(|a| a.account_id == incoming.account_id)
    {
        existing.auth_json = incoming.auth_json;
        if incoming.email.is_some() {
            existing.email = incoming.email;
        }
        if incoming.plan_type.is_some() {
            existing.plan_type = incoming.plan_type;
        }
        if incoming.usage.is_some() {
            existing.usage = incoming.usage;
        }
        existing.updated_at = now_unix_seconds();
        summary.codex_updated += 1;
    } else {
        store.codex_accounts.push(CodexAccount {
            id: Uuid::new_v4().to_string(),
            updated_at: now_unix_seconds(),
            ..incoming
        });
        summary.codex_added += 1;
    }
}

fn upsert_antigravity_account(
    store: &mut AccountStore,
    incoming: AntigravityAccount,
    summary: &mut ImportSummary,
) {
    if let Some(existing) = store
        .antigravity_accounts
        .iter_mut()
        .find(|a| a.email.eq_ignore_ascii_case(&incoming.email))
    {
        if incoming.keychain_secret.is_some() {
            existing.keychain_secret = incoming.keychain_secret;
        }
        if incoming.scoped_keychain_secret.is_some() {
            existing.scoped_keychain_account = incoming.scoped_keychain_account;
            existing.scoped_keychain_secret = incoming.scoped_keychain_secret;
        }
        if incoming.vscdb_oauth_token.is_some() {
            existing.vscdb_oauth_token = incoming.vscdb_oauth_token;
        }
        if incoming.plan_name.is_some() {
            existing.plan_name = incoming.plan_name;
        }
        if incoming.usage.is_some() {
            existing.usage = incoming.usage;
        }
        existing.updated_at = now_unix_seconds();
        summary.antigravity_updated += 1;
    } else {
        store.antigravity_accounts.push(AntigravityAccount {
            id: Uuid::new_v4().to_string(),
            updated_at: now_unix_seconds(),
            ..incoming
        });
        summary.antigravity_added += 1;
    }
}

fn normalize_codex_item_to_auth_json(item: &Value) -> Option<Value> {
    if item.get("tokens").is_some() {
        return Some(item.clone());
    }
    if let Some(auth_json) = item.get("authJson") {
        return Some(auth_json.clone());
    }
    let access_token = item.get("access_token").and_then(Value::as_str)?;
    let mut tokens = serde_json::Map::new();
    tokens.insert(
        "access_token".to_string(),
        Value::String(access_token.to_string()),
    );
    if let Some(id_token) = item.get("id_token").and_then(Value::as_str) {
        tokens.insert("id_token".to_string(), Value::String(id_token.to_string()));
    }
    if let Some(refresh_token) = item.get("refresh_token").and_then(Value::as_str) {
        tokens.insert(
            "refresh_token".to_string(),
            Value::String(refresh_token.to_string()),
        );
    }
    if let Some(account_id) = item.get("account_id").and_then(Value::as_str) {
        tokens.insert(
            "account_id".to_string(),
            Value::String(account_id.to_string()),
        );
    }

    let mut root = serde_json::Map::new();
    root.insert("OPENAI_API_KEY".to_string(), Value::Null);
    root.insert("auth_mode".to_string(), Value::String("chatgpt".to_string()));
    if let Some(last_refresh) = item.get("last_refresh").and_then(Value::as_str) {
        root.insert(
            "last_refresh".to_string(),
            Value::String(last_refresh.to_string()),
        );
    }
    root.insert("tokens".to_string(), Value::Object(tokens));
    Some(Value::Object(root))
}

fn try_import_codex_item(
    store: &mut AccountStore,
    item: &Value,
    summary: &mut ImportSummary,
) -> bool {
    let Some(auth_json) = normalize_codex_item_to_auth_json(item) else {
        return false;
    };
    let Ok(parsed) = codex::parse_auth_json(&auth_json) else {
        return false;
    };
    let email = item
        .get("email")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(parsed.email);
    let plan_type = item
        .get("planType")
        .or_else(|| item.get("plan_type"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(parsed.plan_type);

    upsert_codex_account(
        store,
        CodexAccount {
            id: String::new(),
            email,
            account_id: parsed.account_id,
            plan_type,
            auth_json,
            usage: None,
            usage_error: None,
            updated_at: now_unix_seconds(),
        },
        summary,
    );
    true
}

pub fn import_accounts_from_value(
    store: &mut AccountStore,
    root: &Value,
) -> Result<ImportSummary, String> {
    let mut summary = ImportSummary::default();

    // 1. 顶层数组格式（如扁平 Codex 账号数组 [{ "access_token": ..., "refresh_token": ... }]）
    if let Some(arr) = root.as_array() {
        for item in arr {
            if item.get("keychain_secret").is_some() || item.get("vscdb_oauth_token").is_some() {
                if let Ok(acc) = serde_json::from_value::<AntigravityAccount>(item.clone()) {
                    upsert_antigravity_account(store, acc, &mut summary);
                    continue;
                }
            }
            try_import_codex_item(store, item, &mut summary);
        }
        return Ok(summary);
    }

    // 2. AiSwitcher 原生备份格式
    if root.get("codex_accounts").is_some() || root.get("antigravity_accounts").is_some() {
        if let Some(arr) = root.get("codex_accounts").and_then(Value::as_array) {
            for item in arr {
                if let Ok(acc) = serde_json::from_value::<CodexAccount>(item.clone()) {
                    upsert_codex_account(store, acc, &mut summary);
                }
            }
        }
        if let Some(arr) = root.get("antigravity_accounts").and_then(Value::as_array) {
            for item in arr {
                if let Ok(acc) = serde_json::from_value::<AntigravityAccount>(item.clone()) {
                    upsert_antigravity_account(store, acc, &mut summary);
                }
            }
        }
        return Ok(summary);
    }

    // 3. Codex Tools accounts.json / 导出格式
    if let Some(arr) = root.get("accounts").and_then(Value::as_array) {
        for item in arr {
            let source_kind = item
                .get("sourceKind")
                .and_then(Value::as_str)
                .unwrap_or("chatgpt");
            if source_kind != "chatgpt" {
                continue;
            }
            try_import_codex_item(store, item, &mut summary);
        }
        return Ok(summary);
    }

    // 4. 单个 Codex auth.json 或单个扁平 Codex 对象
    if try_import_codex_item(store, root, &mut summary) {
        return Ok(summary);
    }

    Err("无法识别的 JSON 格式（支持 AiSwitcher 备份、Codex 扁平数组、Codex Tools 及单账号 auth.json）".to_string())
}

pub fn import_accounts_from_path(
    store: &mut AccountStore,
    path: &Path,
) -> Result<ImportSummary, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("读取文件失败: {e}"))?;
    let root: Value = serde_json::from_str(&raw).map_err(|e| format!("JSON 格式无效: {e}"))?;
    import_accounts_from_value(store, &root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use serde_json::json;

    #[test]
    fn roundtrips_native_export_and_import() {
        let mut src = AccountStore::default();
        src.antigravity_accounts.push(AntigravityAccount {
            id: "ag-1".to_string(),
            email: "demo@gmail.com".to_string(),
            plan_name: Some("Pro".to_string()),
            keychain_secret: Some("sec".to_string()),
            scoped_keychain_account: None,
            scoped_keychain_secret: None,
            vscdb_oauth_token: None,
            usage: None,
            usage_error: None,
            updated_at: 100,
        });
        let val = serde_json::to_value(&src).expect("serialize");

        let mut dst = AccountStore::default();
        let s1 = import_accounts_from_value(&mut dst, &val).expect("import first");
        assert_eq!(s1.antigravity_added, 1);
        assert_eq!(dst.antigravity_accounts.len(), 1);

        let s2 = import_accounts_from_value(&mut dst, &val).expect("import duplicate");
        assert_eq!(s2.antigravity_added, 0);
        assert_eq!(s2.antigravity_updated, 1);
        assert_eq!(dst.antigravity_accounts.len(), 1);
    }

    #[test]
    fn imports_single_codex_auth_json() {
        let h = URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let p = URL_SAFE_NO_PAD.encode(
            json!({
                "exp": 1999999999,
                "https://api.openai.com/auth": { "chatgpt_account_id": "acc-9" }
            })
            .to_string(),
        );
        let auth = json!({
            "tokens": {
                "access_token": format!("{h}.{p}.sig"),
                "account_id": "acc-9"
            }
        });

        let mut store = AccountStore::default();
        let summary = import_accounts_from_value(&mut store, &auth).expect("import single auth");
        assert_eq!(summary.codex_added, 1);
        assert_eq!(store.codex_accounts.len(), 1);
    }

    #[test]
    fn imports_flat_codex_array_format() {
        let h = URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let p = URL_SAFE_NO_PAD.encode(
            json!({
                "exp": 1999999999,
                "https://api.openai.com/auth": {
                    "chatgpt_account_id": "595977d3-b0af-4207-bf69-b57be48ce595",
                    "chatgpt_plan_type": "plus"
                }
            })
            .to_string(),
        );
        let flat_array = json!([
            {
                "id_token": format!("{h}.{p}.sig"),
                "access_token": format!("{h}.{p}.sig"),
                "refresh_token": "rt.1.test",
                "account_id": "595977d3-b0af-4207-bf69-b57be48ce595",
                "last_refresh": "2026-11-16T01:06:02.000Z",
                "email": "demo@example.com",
                "type": "codex",
                "expired": "2026-11-26T01:04:40.000Z"
            }
        ]);

        let mut store = AccountStore::default();
        let summary = import_accounts_from_value(&mut store, &flat_array).expect("import flat array");
        assert_eq!(summary.codex_added, 1);
        let acc = &store.codex_accounts[0];
        assert_eq!(acc.email.as_deref(), Some("demo@example.com"));
        assert_eq!(acc.plan_type.as_deref(), Some("plus"));
        assert_eq!(acc.auth_json["auth_mode"], "chatgpt");
        assert_eq!(acc.auth_json["tokens"]["refresh_token"], "rt.1.test");
    }
}
