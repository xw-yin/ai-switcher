use std::time::Duration;

use reqwest::Client;
use semver::Version;
use serde::Deserialize;

const RELEASES: &str = "https://github.com/xw-yin/ai-switcher/releases";
const LATEST: &str = "https://api.github.com/repos/xw-yin/ai-switcher/releases/latest";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Clone, Deserialize)]
struct Asset {
    name: String,
    state: String,
    #[cfg(target_os = "windows")]
    digest: Option<String>,
}

#[derive(Clone)]
pub struct AvailableUpdate {
    pub tag: String,
    asset: Option<Asset>,
}

impl AvailableUpdate {
    pub fn action_label(&self) -> &'static str {
        #[cfg(target_os = "windows")]
        if self.installer_digest().is_some() {
            return "安装更新";
        }
        if self.asset.is_some() {
            "下载安装包"
        } else {
            "查看发布页"
        }
    }

    pub async fn open(self) -> Result<&'static str, String> {
        let Some(asset) = &self.asset else {
            crate::process::open_url(&format!("{RELEASES}/tag/{}", self.tag))?;
            return Ok("已打开发布页");
        };
        let url = format!("{RELEASES}/download/{}/{}", self.tag, asset.name);
        #[cfg(target_os = "windows")]
        if let Some(digest) = self.installer_digest() {
            launch_installer(&url, &asset.name, digest).await?;
            return Ok("已启动安装程序，请按提示完成更新");
        }
        crate::process::open_url(&url)?;
        Ok("已在浏览器中打开下载，请完成安装")
    }

    #[cfg(target_os = "windows")]
    fn installer_digest(&self) -> Option<&str> {
        let asset = self.asset.as_ref()?;
        if !asset.name.ends_with("-setup.exe") {
            return None;
        }
        let digest = asset.digest.as_deref()?.strip_prefix("sha256:")?;
        (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then_some(digest)
    }
}

fn client(timeout: Duration) -> Result<Client, String> {
    Client::builder()
        .user_agent(concat!("ai-switcher/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())
}

pub async fn check() -> Result<Option<AvailableUpdate>, String> {
    let response = client(Duration::from_secs(15))?
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err("GitHub 上没有可访问的正式版本".into());
    }
    let release: Release = response
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let installed =
        std::env::current_exe().is_ok_and(|exe| exe.with_file_name("Uninstall.exe").is_file());
    select_update(
        release,
        CURRENT_VERSION,
        asset_suffixes(std::env::consts::OS, std::env::consts::ARCH, installed),
    )
}

fn asset_suffixes(os: &str, arch: &str, installed: bool) -> &'static [&'static str] {
    match (os, arch) {
        ("macos", "aarch64") => &["macos-universal.dmg", "macos-arm64.dmg"],
        ("macos", "x86_64") => &["macos-universal.dmg", "macos-x86_64.dmg"],
        ("windows", "x86_64") if installed => &["windows-x86_64-setup.exe"],
        ("windows", "x86_64") => &["windows-x86_64.zip"],
        ("linux", "x86_64") => &["linux-x86_64.tar.gz"],
        _ => &[],
    }
}

fn select_update(
    release: Release,
    current: &str,
    suffixes: &[&str],
) -> Result<Option<AvailableUpdate>, String> {
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let version = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )
    .map_err(|e| format!("无法识别发布版本号：{e}"))?;
    let current = Version::parse(current).map_err(|e| format!("无法识别当前版本号：{e}"))?;
    if !version.pre.is_empty() || !version.cmp_precedence(&current).is_gt() {
        return Ok(None);
    }
    let asset = suffixes.iter().find_map(|suffix| {
        let name = format!("AI-Switcher-v{version}-{suffix}");
        release
            .assets
            .iter()
            .find(|asset| asset.name == name && asset.state == "uploaded")
    });
    Ok(Some(AvailableUpdate {
        tag: release.tag_name,
        asset: asset.cloned(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, assets: &[&str]) -> Release {
        Release {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
            assets: assets
                .iter()
                .map(|&name| Asset {
                    name: name.into(),
                    state: "uploaded".into(),
                    #[cfg(target_os = "windows")]
                    digest: None,
                })
                .collect(),
        }
    }

    #[test]
    fn offers_only_newer_stable_versions() {
        let select = |tag| select_update(release(tag, &[]), "0.1.9", &[]).unwrap();
        assert!(select("v0.1.10").is_some());
        assert!(select("v0.1.9").is_none());
        assert!(select("v0.1.8").is_none());
        assert!(select("v0.2.0-beta.1").is_none());
    }

    #[test]
    fn distinguishes_windows_installation_and_portable_packages() {
        let setup = "AI-Switcher-v0.1.10-windows-x86_64-setup.exe";
        let zip = "AI-Switcher-v0.1.10-windows-x86_64.zip";
        for (installed, expected) in [(true, setup), (false, zip)] {
            let update = select_update(
                release("v0.1.10", &[zip, setup]),
                "0.1.9",
                asset_suffixes("windows", "x86_64", installed),
            )
            .unwrap()
            .unwrap();
            assert_eq!(update.asset.unwrap().name, expected);
        }
    }

    #[test]
    fn falls_back_to_release_page_when_platform_package_is_missing() {
        let update = select_update(
            release("v0.1.10", &["AI-Switcher-v0.1.10-macos-x86_64.dmg"]),
            "0.1.9",
            asset_suffixes("macos", "aarch64", false),
        )
        .unwrap()
        .unwrap();
        assert_eq!(update.action_label(), "查看发布页");
    }
}

#[cfg(target_os = "windows")]
async fn launch_installer(url: &str, name: &str, digest: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::{fs, io::Write, process::Command};

    const MAX_INSTALLER_BYTES: usize = 256 * 1024 * 1024;
    let mut response = client(Duration::from_secs(300))?
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.to_string())?;
    let dir = std::env::temp_dir().join(format!("ai-switcher-update-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&dir).map_err(|e| e.to_string())?;
    let installer = dir.join(name);
    let result = async {
        let mut file = fs::File::create_new(&installer).map_err(|e| e.to_string())?;
        let mut hash = Sha256::new();
        let mut size = 0;
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            size += chunk.len();
            if size > MAX_INSTALLER_BYTES {
                return Err("安装包大小超出限制".into());
            }
            hash.update(&chunk);
            file.write_all(&chunk).map_err(|e| e.to_string())?;
        }
        drop(file);
        if !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(digest) {
            return Err("安装包 SHA-256 校验失败".into());
        }
        let mut child = Command::new(&installer)
            .current_dir(&dir)
            .spawn()
            .map_err(|e| format!("无法启动安装程序：{e}"))?;
        let cleanup_dir = dir.clone();
        std::thread::spawn(move || {
            if child.wait().is_ok() {
                let _ = fs::remove_dir_all(cleanup_dir);
            }
        });
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_dir_all(dir);
    }
    result
}
