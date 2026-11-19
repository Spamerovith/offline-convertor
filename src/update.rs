//! Updates from the public GitHub releases. This is the only network access of the app:
//! one request to api.github.com on start, and the download when the user agrees.
//! User data (%APPDATA%\OfflineConvertor) is never touched: only the exe is replaced.

use std::io::{Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

const LATEST: &str = "https://api.github.com/repos/Spamerovith/offline-convertor/releases/latest";
const ASSET: &str = "OfflineConvertor.exe";
const AGENT: &str = "OfflineConvertor-updater";

#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub newer: bool,
    pub url: String,
    pub size: u64,
    pub notes: String,
    pub page: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

fn parse_ver(v: &str) -> (u32, u32, u32) {
    let mut it = v.trim().trim_start_matches(['v', 'V']).split(['.', '-']).map(|x| x.parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

pub fn check() -> Result<UpdateInfo, String> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let mut resp = ureq::get(LATEST)
        .header("User-Agent", AGENT)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    let r: Release = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    let asset = r.assets.iter().find(|a| a.name.eq_ignore_ascii_case(ASSET));
    let latest = r.tag_name.trim_start_matches(['v', 'V']).to_string();
    Ok(UpdateInfo {
        newer: asset.is_some() && parse_ver(&latest) > parse_ver(&current),
        url: asset.map(|a| a.browser_download_url.clone()).unwrap_or_default(),
        size: asset.map(|a| a.size).unwrap_or(0),
        notes: r.body,
        page: r.html_url,
        current,
        latest,
    })
}

fn exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| e.to_string())
}

fn old_path(exe: &std::path::Path) -> PathBuf {
    let mut s = exe.as_os_str().to_owned();
    s.push(".old");
    PathBuf::from(s)
}

/// Removes the previous exe left by an update (it can't be deleted while it runs).
/// Right after an update the previous process may still be closing, so retry for a while.
pub fn cleanup_old() {
    if let Ok(e) = exe() {
        let mut n = e.as_os_str().to_owned();
        n.push(".new");
        let _ = std::fs::remove_file(PathBuf::from(n));
        let old = old_path(&e);
        if old.exists() {
            std::thread::spawn(move || {
                for _ in 0..30 {
                    if std::fs::remove_file(&old).is_ok() || !old.exists() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            });
        }
    }
}

#[derive(Clone, Serialize)]
struct Progress {
    done: u64,
    total: u64,
}

/// Downloads the new exe next to the current one, swaps them and starts the new version.
pub fn install(app: &AppHandle, url: &str) -> Result<(), String> {
    if !url.starts_with("https://github.com/Spamerovith/offline-convertor/") {
        return Err("Неверный адрес обновления".into());
    }
    let exe = exe()?;
    let mut new_path = exe.as_os_str().to_owned();
    new_path.push(".new");
    let new_path = PathBuf::from(new_path);

    let mut resp = ureq::get(url).header("User-Agent", AGENT).call().map_err(|e| e.to_string())?;
    let total: u64 = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut reader = resp.body_mut().with_config().limit(u64::MAX).reader();
    let mut f = std::fs::File::create(&new_path).map_err(|e| format!("Нет доступа к папке программы: {e}"))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    let mut last = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if done - last >= 1 << 20 || done == total {
            last = done;
            let _ = app.emit("update", Progress { done, total });
        }
    }
    drop(f);
    // A truncated download must never replace a working exe.
    let ok_size = total == 0 || done == total;
    let head = std::fs::read(&new_path).ok().map(|d| d.starts_with(b"MZ")).unwrap_or(false);
    if !ok_size || !head || done < 1 << 20 {
        let _ = std::fs::remove_file(&new_path);
        return Err("Файл обновления скачан не полностью".into());
    }
    let old = old_path(&exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("Не удалось заменить программу: {e}"))?;
    if let Err(e) = std::fs::rename(&new_path, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("Не удалось заменить программу: {e}"));
    }
    std::process::Command::new(&exe).spawn().map_err(|e| e.to_string())?;
    Ok(())
}
