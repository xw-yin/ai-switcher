use std::collections::HashSet;
#[cfg(target_os = "macos")]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use sysinfo::{Pid, System, Uid};

#[cfg(target_os = "macos")]
const CODEX_APP_NAMES: [&str; 3] = ["ChatGPT.app", "Codex.app", "Codex Desktop.app"];
#[cfg(target_os = "macos")]
const CODEX_CLI_RELATIVE_PATHS: [&str; 3] = [
    "Contents/Resources/codex",
    "Contents/Resources/codex-cli/bin/codex",
    "Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
];
#[cfg(target_os = "macos")]
const ANTIGRAVITY_APP_NAME: &str = "Antigravity.app";
#[cfg(target_os = "macos")]
const ANTIGRAVITY_IDE_APP_NAME: &str = "Antigravity IDE.app";

#[cfg(target_os = "macos")]
fn is_executable_file(path: &Path) -> bool {
    path.metadata()
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111 != 0))
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
pub fn is_codex_app_bundle(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
        return false;
    };
    if name.eq_ignore_ascii_case("Codex.app") || name.eq_ignore_ascii_case("Codex Desktop.app") {
        return true;
    }
    if !name.eq_ignore_ascii_case("ChatGPT.app") {
        return false;
    }
    CODEX_CLI_RELATIVE_PATHS
        .iter()
        .any(|rel| is_executable_file(&path.join(rel)))
}

#[cfg(target_os = "macos")]
pub fn is_antigravity_app_bundle(path: &Path) -> bool {
    path.is_dir()
        && path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(ANTIGRAVITY_APP_NAME))
}

#[cfg(target_os = "macos")]
pub fn is_antigravity_ide_app_bundle(path: &Path) -> bool {
    path.is_dir()
        && path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(ANTIGRAVITY_IDE_APP_NAME))
}

pub fn find_codex_app_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir();
        for name in CODEX_APP_NAMES {
            let sys = Path::new("/Applications").join(name);
            if is_codex_app_bundle(&sys) {
                return Some(sys);
            }
            if let Some(ref h) = home {
                let user = h.join("Applications").join(name);
                if is_codex_app_bundle(&user) {
                    return Some(user);
                }
            }
        }
        None
    }

    #[cfg(target_os = "windows")]
    {
        let local = dirs::data_local_dir()?;
        let candidates = [
            local.join("Programs").join("OpenAI").join("Codex").join("Codex.exe"),
            local.join("Programs").join("ChatGPT").join("ChatGPT.exe"),
            local.join("OpenAI").join("Codex").join("Codex.exe"),
        ];
        candidates.into_iter().find(|p| p.is_file())
    }

    #[cfg(target_os = "linux")]
    {
        ["/usr/bin/codex", "/usr/local/bin/codex"]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
    }
}

pub fn find_antigravity_app_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let sys = Path::new("/Applications").join(ANTIGRAVITY_APP_NAME);
        if is_antigravity_app_bundle(&sys) {
            return Some(sys);
        }
        if let Some(h) = dirs::home_dir() {
            let user = h.join("Applications").join(ANTIGRAVITY_APP_NAME);
            if is_antigravity_app_bundle(&user) {
                return Some(user);
            }
        }
        None
    }

    #[cfg(target_os = "windows")]
    {
        let local = dirs::data_local_dir()?;
        let p = local.join("Programs").join("Antigravity").join("Antigravity.exe");
        p.is_file().then_some(p)
    }

    #[cfg(target_os = "linux")]
    {
        ["/usr/bin/antigravity", "/usr/local/bin/antigravity"]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
    }
}

pub fn find_antigravity_ide_app_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let sys = Path::new("/Applications").join(ANTIGRAVITY_IDE_APP_NAME);
        if is_antigravity_ide_app_bundle(&sys) {
            return Some(sys);
        }
        if let Some(h) = dirs::home_dir() {
            let user = h.join("Applications").join(ANTIGRAVITY_IDE_APP_NAME);
            if is_antigravity_ide_app_bundle(&user) {
                return Some(user);
            }
        }
        None
    }

    #[cfg(target_os = "windows")]
    {
        let local = dirs::data_local_dir()?;
        let p = local
            .join("Programs")
            .join("Antigravity IDE")
            .join("Antigravity IDE.exe");
        p.is_file().then_some(p)
    }

    #[cfg(target_os = "linux")]
    {
        ["/usr/bin/antigravity-ide", "/usr/local/bin/antigravity-ide"]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
    }
}

#[cfg(target_os = "macos")]
fn app_bundle_for_main_executable<'a>(
    executable: &'a Path,
    matcher: impl Fn(&Path) -> bool,
) -> Option<&'a Path> {
    let macos_dir = executable.parent()?;
    if !macos_dir
        .file_name()
        .and_then(|v| v.to_str())
        .is_some_and(|v| v.eq_ignore_ascii_case("MacOS"))
    {
        return None;
    }
    let contents_dir = macos_dir.parent()?;
    if !contents_dir
        .file_name()
        .and_then(|v| v.to_str())
        .is_some_and(|v| v.eq_ignore_ascii_case("Contents"))
    {
        return None;
    }
    let app_bundle = contents_dir.parent()?;
    if !matcher(app_bundle) {
        return None;
    }
    Some(app_bundle)
}

fn collect_descendants(
    mut roots: HashSet<Pid>,
    parents: &[(Pid, Option<Pid>)],
) -> HashSet<Pid> {
    loop {
        let before = roots.len();
        for (pid, parent) in parents {
            if parent.is_some_and(|p| roots.contains(&p)) {
                roots.insert(*pid);
            }
        }
        if roots.len() == before {
            return roots;
        }
    }
}

fn running_matching_process_ids(
    system: &System,
    user_id: Option<&Uid>,
    is_target_exe: impl Fn(&Path) -> bool,
) -> HashSet<Pid> {
    let same_user: Vec<(Pid, &sysinfo::Process)> = system
        .processes()
        .iter()
        .filter(|(_, p)| user_id.is_none() || p.user_id() == user_id)
        .map(|(pid, p)| (*pid, p))
        .collect();

    let roots: HashSet<Pid> = same_user
        .iter()
        .filter_map(|(pid, proc)| proc.exe().and_then(|exe| is_target_exe(exe).then_some(*pid)))
        .collect();

    let parents: Vec<(Pid, Option<Pid>)> = same_user
        .into_iter()
        .map(|(pid, p)| (pid, p.parent()))
        .collect();

    collect_descendants(roots, &parents)
}

fn stop_matching_processes(label: &str, is_target_exe: impl Fn(&Path) -> bool) -> Result<(), String> {
    let mut system = System::new_all();
    let current_pid = sysinfo::get_current_pid().map_err(|e| e.to_string())?;
    let user_id = system
        .process(current_pid)
        .and_then(|p| p.user_id())
        .cloned();

    let mut remaining = running_matching_process_ids(&system, user_id.as_ref(), &is_target_exe);
    remaining.remove(&current_pid);
    if remaining.is_empty() {
        return Ok(());
    }

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        for pid in &remaining {
            if let Some(p) = system.process(*pid) {
                let _ = p.kill();
            }
        }
        thread::sleep(Duration::from_millis(60));
        system.refresh_processes();
        remaining.retain(|pid| system.process(*pid).is_some());
        remaining.extend(running_matching_process_ids(
            &system,
            user_id.as_ref(),
            &is_target_exe,
        ));
        remaining.remove(&current_pid);
        if remaining.is_empty() {
            thread::sleep(Duration::from_millis(200));
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("结束 {label} 进程超时"));
        }
    }
}

pub fn stop_codex_desktop() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        stop_matching_processes("ChatGPT/Codex", |exe| {
            app_bundle_for_main_executable(exe, is_codex_app_bundle).is_some()
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        stop_matching_processes("ChatGPT/Codex", |exe| {
            exe.file_name()
                .and_then(|v| v.to_str())
                .is_some_and(|name| {
                    let lower = name.to_ascii_lowercase();
                    lower == "codex.exe"
                        || lower == "chatgpt.exe"
                        || lower == "codex"
                        || lower == "codex-desktop"
                })
        })
    }
}

pub fn stop_antigravity_desktop() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        stop_matching_processes("Antigravity", |exe| {
            app_bundle_for_main_executable(exe, is_antigravity_app_bundle).is_some()
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        stop_matching_processes("Antigravity", |exe| {
            exe.file_name()
                .and_then(|v| v.to_str())
                .is_some_and(|name| {
                    let lower = name.to_ascii_lowercase();
                    lower == "antigravity.exe" || lower == "antigravity"
                })
        })
    }
}

pub fn stop_antigravity_ide_desktop() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        stop_matching_processes("Antigravity IDE", |exe| {
            app_bundle_for_main_executable(exe, is_antigravity_ide_app_bundle).is_some()
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        stop_matching_processes("Antigravity IDE", |exe| {
            exe.file_name()
                .and_then(|v| v.to_str())
                .is_some_and(|name| {
                    let lower = name.to_ascii_lowercase();
                    lower == "antigravity ide.exe" || lower == "antigravity-ide"
                })
        })
    }
}

pub fn launch_app(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("open")
            .arg("-na")
            .arg(path)
            .status()
            .map_err(|e| format!("启动 {} 失败: {e}", path.display()))?;
        if !status.success() {
            return Err(format!("启动 {} 返回非零状态", path.display()));
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        Command::new(path)
            .spawn()
            .map_err(|e| format!("启动 {} 失败: {e}", path.display()))?;
        Ok(())
    }
}

pub fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(url)
            .status()
            .map_err(|e| format!("打开浏览器失败: {e}"))?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use std::ptr;

        #[link(name = "shell32")]
        extern "system" {
            fn ShellExecuteW(
                hwnd: *mut core::ffi::c_void,
                lp_operation: *const u16,
                lp_file: *const u16,
                lp_parameters: *const u16,
                lp_directory: *const u16,
                n_show_cmd: i32,
            ) -> isize;
        }

        const SW_SHOWNORMAL: i32 = 1;
        let op: Vec<u16> = OsStr::new("open").encode_wide().chain(Some(0)).collect();
        let target: Vec<u16> = OsStr::new(url).encode_wide().chain(Some(0)).collect();
        let ret = unsafe {
            ShellExecuteW(
                ptr::null_mut(),
                op.as_ptr(),
                target.as_ptr(),
                ptr::null(),
                ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        if ret <= 32 {
            return Err(format!("打开浏览器失败 (ShellExecuteW={ret})"));
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("打开浏览器失败: {e}"))?;
        Ok(())
    }
}
