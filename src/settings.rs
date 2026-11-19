//! Persistent user settings in %APPDATA%\OfflineConvertor\settings.json (never next to the exe).

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// "" (not chosen yet - ask on first start), "ru", "en"
    pub lang: String,
    /// Update offer the user dismissed ("1.3.0"), so it isn't shown again on every start
    pub skip_version: String,
    pub theme: String,
    /// "manual" (keep results until «Загрузить на диск»), "source" (next to the source), "folder" (out_dir)
    pub out_mode: String,
    /// Output folder for out_mode = "folder"
    pub out_dir: String,
    pub video_parallel: u32,
    /// "auto" (spread video over all GPUs) or a device id ("nv0", "nv1", "amf", "qsv", "mf")
    pub gpu_device: String,
    /// 0 = all CPU threads
    pub cpu_parallel: u32,
    pub open_when_done: bool,
    /// Remembered per-tool options (JSON blobs from the UI)
    pub tool_options: serde_json::Map<String, serde_json::Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lang: String::new(),
            skip_version: String::new(),
            theme: "graphite".into(),
            out_mode: String::new(),
            out_dir: String::new(),
            video_parallel: 2,
            gpu_device: "auto".into(),
            cpu_parallel: 0,
            open_when_done: false,
            tool_options: Default::default(),
        }
    }
}

fn file() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("OfflineConvertor").join("settings.json")
}

fn cell() -> &'static Mutex<Settings> {
    static S: OnceLock<Mutex<Settings>> = OnceLock::new();
    S.get_or_init(|| {
        // Tolerate a UTF-8 BOM (files edited by hand in Notepad or PowerShell).
        let s = std::fs::read(file())
            .ok()
            .and_then(|d| {
                let d = d.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&d);
                serde_json::from_slice(d).ok()
            })
            .unwrap_or_default();
        Mutex::new(s)
    })
}

pub fn get() -> Settings {
    cell().lock().unwrap().clone()
}

pub fn set(s: Settings) {
    let p = file();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(data) = serde_json::to_vec_pretty(&s) {
        let _ = std::fs::write(&p, data);
    }
    *cell().lock().unwrap() = s;
}
