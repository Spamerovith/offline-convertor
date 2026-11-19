#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod jobs;
mod settings;
mod update;

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Serialize)]
struct SystemInfo {
    caps: engine::gpu::Capabilities,
    ocr_languages: Vec<engine::ocr::OcrLanguage>,
    version: String,
    ffmpeg_path: String,
}

#[derive(Serialize)]
struct FileEntry {
    path: String,
    name: String,
    size: u64,
    ext: String,
    kind: String,
}

#[tauri::command]
async fn system_info() -> SystemInfo {
    tauri::async_runtime::spawn_blocking(|| SystemInfo {
        caps: engine::gpu::caps().clone(),
        ocr_languages: engine::ocr::languages(),
        version: env!("CARGO_PKG_VERSION").into(),
        ffmpeg_path: engine::ffmpeg::ffmpeg_path().to_string_lossy().to_string(),
    })
    .await
    .unwrap()
}

fn walk(p: &Path, out: &mut Vec<FileEntry>) {
    if p.is_dir() {
        if let Ok(rd) = std::fs::read_dir(p) {
            let mut items: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
            items.sort();
            for i in items {
                walk(&i, out);
            }
        }
    } else if let Ok(m) = std::fs::metadata(p) {
        out.push(FileEntry {
            path: p.to_string_lossy().to_string(),
            name: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            size: m.len(),
            ext: engine::paths::ext_of(p),
            kind: jobs::kind_of(p).into(),
        });
    }
}

/// Expands dropped/selected paths (folders recursively) into a flat file list.
#[tauri::command]
async fn scan_paths(paths: Vec<String>) -> Vec<FileEntry> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = vec![];
        for p in paths {
            walk(Path::new(&p), &mut out);
        }
        out
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
async fn probe(path: String) -> Result<engine::ffmpeg::MediaInfo, String> {
    tauri::async_runtime::spawn_blocking(move || engine::ffmpeg::probe(Path::new(&path))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn enqueue(jobs: Vec<jobs::JobSpec>) {
    jobs::enqueue(jobs);
}

#[tauri::command]
fn cancel_job(id: String) {
    jobs::cancel(&id);
}

#[tauri::command]
fn cancel_all() {
    jobs::cancel_all();
}

#[tauri::command]
fn get_settings() -> settings::Settings {
    settings::get()
}

#[tauri::command]
fn save_settings(s: settings::Settings) {
    let (v, c) = (s.video_parallel, s.cpu_parallel);
    settings::set(s);
    jobs::apply_limits(v, c);
}

#[tauri::command]
fn reveal(path: String) {
    let _ = std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{path}\"")).spawn();
}

/// Saves a finished temporary result to the path chosen in the «Save as» dialog.
#[tauri::command]
async fn save_result(path: String, dest: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let from = PathBuf::from(&path);
        let to = PathBuf::from(&dest);
        if !engine::paths::is_temp(&from) || !from.is_file() {
            return Err("Временный файл не найден, обработайте файл ещё раз".to_string());
        }
        if from == to {
            return Ok(dest);
        }
        engine::paths::move_file(&from, &to)?;
        Ok(dest)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Saves several temporary results into one folder (unique names, nothing is replaced).
#[tauri::command]
async fn save_to_folder(paths: Vec<String>, folder: String) -> Vec<Result<String, String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let folder = PathBuf::from(folder);
        paths
            .into_iter()
            .map(|p| {
                let from = PathBuf::from(&p);
                if !engine::paths::is_temp(&from) || !from.is_file() {
                    return Err("Временный файл не найден".to_string());
                }
                engine::paths::move_into_folder(&from, &folder).map(|d| d.to_string_lossy().to_string())
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Deletes an unsaved temporary result.
#[tauri::command]
fn discard_result(path: String) {
    let p = PathBuf::from(path);
    if engine::paths::is_temp(&p) {
        engine::paths::discard(&p);
    }
}

/// Appends UI diagnostics to %APPDATA%\OfflineConvertor\ui.log (kept small).
#[tauri::command]
fn ui_log(msg: String) {
    use std::io::Write;
    let dir = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("OfflineConvertor");
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("ui.log");
    if std::fs::metadata(&p).map(|m| m.len() > 512 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&p);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "[{t}] {msg}");
    }
}

#[tauri::command]
async fn check_update() -> Result<update::UpdateInfo, String> {
    tauri::async_runtime::spawn_blocking(update::check).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn install_update(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let a = app.clone();
    tauri::async_runtime::spawn_blocking(move || update::install(&a, &url)).await.map_err(|e| e.to_string())??;
    // The new version is already starting: leave so it owns the window.
    jobs::cancel_all();
    engine::paths::cleanup_all();
    app.exit(0);
    Ok(())
}

#[tauri::command]
fn open_path(path: String) {
    let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
}

/// Command-line mode: `--cli <tool> <file> <options-json> <out-dir|-> <result.json>`.
fn cli(args: &[String]) -> ! {
    use std::sync::atomic::AtomicBool;
    let tool = args.get(2).cloned().unwrap_or_default();
    let src = PathBuf::from(args.get(3).cloned().unwrap_or_default());
    let opts: serde_json::Value = serde_json::from_str(args.get(4).map(|s| s.as_str()).unwrap_or("{}")).unwrap_or_default();
    let out_dir = args.get(5).filter(|s| s.as_str() != "-").map(PathBuf::from);
    let result_file = args.get(6).cloned().unwrap_or_else(|| "result.json".into());
    engine::ocr::init_thread();
    let cancel = AtomicBool::new(false);
    let progress = |_f: f64, _s: Option<String>| {};
    let ctx = engine::Ctx { cancel: &cancel, progress: &progress, out_dir };
    let t0 = std::time::Instant::now();
    let r: engine::JobResult = match tool.as_str() {
        "video" => serde_json::from_value(opts).map_err(|e| e.to_string()).and_then(|o| engine::video::run(&src, &o, &ctx)),
        "audio" => serde_json::from_value(opts).map_err(|e| e.to_string()).and_then(|o| engine::audio::run(&src, &o, &ctx)),
        "image" => serde_json::from_value(opts).map_err(|e| e.to_string()).and_then(|o| engine::image_ops::run(&src, &o, &ctx)),
        "pdf" => serde_json::from_value(opts).map_err(|e| e.to_string()).and_then(|o| engine::pdf::run(&src, &o, &ctx)),
        "ocr" => serde_json::from_value(opts).map_err(|e| e.to_string()).and_then(|o| engine::ocr::run(&src, &o, &ctx)),
        "caps" => {
            let v = serde_json::json!({ "caps": engine::gpu::caps(), "ocr": engine::ocr::languages() });
            let _ = std::fs::write(&result_file, serde_json::to_vec_pretty(&v).unwrap());
            std::process::exit(0);
        }
        _ => Err("unknown tool".into()),
    };
    let v = match r {
        Ok(o) => serde_json::json!({ "ok": true, "output": o, "seconds": t0.elapsed().as_secs_f64() }),
        Err(e) => serde_json::json!({ "ok": false, "error": e, "seconds": t0.elapsed().as_secs_f64() }),
    };
    let _ = std::fs::write(&result_file, serde_json::to_vec_pretty(&v).unwrap());
    std::process::exit(0);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--cli") {
        cli(&args);
    }
    // Start GPU/encoder detection right away so the first screen has it ready.
    std::thread::spawn(|| {
        let _ = engine::gpu::caps();
    });
    // Unsaved results from a previous run that ended abnormally.
    engine::paths::cleanup_all();
    update::cleanup_old();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            jobs::init(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            system_info,
            scan_paths,
            probe,
            enqueue,
            cancel_job,
            cancel_all,
            get_settings,
            save_settings,
            reveal,
            open_path,
            ui_log,
            save_result,
            save_to_folder,
            discard_result,
            check_update,
            install_update
        ])
        .build(tauri::generate_context!())
        .expect("error while building Offline Convertor")
        .run(|_app, event| {
            if let tauri::RunEvent::Exit = event {
                // Results that were never saved are removed with the app.
                jobs::cancel_all();
                std::thread::sleep(std::time::Duration::from_millis(300));
                engine::paths::cleanup_all();
            }
        });
}
