use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use sysinfo::{Pid, System, Uid};

const CODEX_APP_NAMES: [&str; 3] = ["ChatGPT.app", "Codex.app", "Codex Desktop.app"];
const CODEX_CLI_RELATIVE_PATHS: [&str; 3] = [
    "Contents/Resources/codex",
    "Contents/Resources/codex-cli/bin/codex",
    "Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
];

const ANTIGRAVITY_APP_NAMES: [&str; 2] = ["Antigravity.app", "Antigravity IDE.app"];

fn is_executable_file(path: &Path) -> bool {
    path.metadata()
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111 != 0))
        .unwrap_or(false)
}

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

pub fn is_antigravity_app_bundle(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
        return false;
    };
    ANTIGRAVITY_APP_NAMES
        .iter()
        .any(|target| name.eq_ignore_ascii_case(target))
}

pub fn find_codex_app_path() -> Option<PathBuf> {
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

pub fn find_antigravity_app_path() -> Option<PathBuf> {
    let home = dirs::home_dir();
    for name in ANTIGRAVITY_APP_NAMES {
        let sys = Path::new("/Applications").join(name);
        if is_antigravity_app_bundle(&sys) {
            return Some(sys);
        }
        if let Some(ref h) = home {
            let user = h.join("Applications").join(name);
            if is_antigravity_app_bundle(&user) {
                return Some(user);
            }
        }
    }
    None
}

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

fn running_app_process_ids(
    system: &System,
    user_id: &Uid,
    matcher: impl Fn(&Path) -> bool,
) -> HashSet<Pid> {
    let same_user: Vec<(Pid, &sysinfo::Process)> = system
        .processes()
        .iter()
        .filter(|(_, p)| p.user_id() == Some(user_id))
        .map(|(pid, p)| (*pid, p))
        .collect();

    let roots: HashSet<Pid> = same_user
        .iter()
        .filter_map(|(pid, proc)| {
            proc.exe()
                .and_then(|exe| app_bundle_for_main_executable(exe, &matcher).map(|_| *pid))
        })
        .collect();

    let parents: Vec<(Pid, Option<Pid>)> = same_user
        .into_iter()
        .map(|(pid, p)| (pid, p.parent()))
        .collect();

    collect_descendants(roots, &parents)
}

fn stop_app_processes(label: &str, matcher: impl Fn(&Path) -> bool) -> Result<(), String> {
    let mut system = System::new_all();
    let current_pid = sysinfo::get_current_pid().map_err(|e| e.to_string())?;
    let user_id = system
        .process(current_pid)
        .and_then(|p| p.user_id())
        .cloned()
        .ok_or_else(|| format!("无法识别当前用户，未结束 {label}"))?;

    let mut remaining = running_app_process_ids(&system, &user_id, &matcher);
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
        remaining.extend(running_app_process_ids(&system, &user_id, &matcher));
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
    stop_app_processes("ChatGPT/Codex", is_codex_app_bundle)
}

pub fn stop_antigravity_desktop() -> Result<(), String> {
    stop_app_processes("Antigravity", is_antigravity_app_bundle)
}

pub fn launch_app(path: &Path) -> Result<(), String> {
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
