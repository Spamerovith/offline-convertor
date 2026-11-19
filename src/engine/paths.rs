//! Output naming: never overwrite anything, reserve names atomically across parallel jobs.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

/// Reserves `<dir>/<stem><suffix>.<ext>` (adding " (1)", " (2)", … when taken) by creating an
/// empty file, so no other job and no existing file can collide with it.
pub fn reserve(src: &Path, out_dir: Option<&Path>, suffix: &str, ext: &str) -> Result<PathBuf, String> {
    let dir = match out_dir {
        Some(d) => d.to_path_buf(),
        None => src.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from(".")),
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("Не удалось создать папку вывода: {e}"))?;
    let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
    let ext = ext.trim_start_matches('.');
    let _g = LOCK.lock().unwrap();
    for i in 0..100_000u32 {
        let name = if i == 0 { format!("{stem}{suffix}.{ext}") } else { format!("{stem}{suffix} ({i}).{ext}") };
        let p = dir.join(name);
        if p == src {
            continue;
        }
        match OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(_) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Нет доступа к папке вывода: {e}")),
        }
    }
    Err("Не удалось подобрать имя файла".into())
}

pub fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Removes a reserved output after failure or cancellation.
pub fn discard(p: &Path) {
    let _ = std::fs::remove_file(p);
}

// ── Temporary results (saved to the user's folders only on request) ─────────

static TEMP_DIRS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("OfflineConvertor")
}

fn registry_file() -> PathBuf {
    app_data().join("temp_dirs.txt")
}

fn drive_of(p: &Path) -> Option<String> {
    match p.components().next() {
        Some(std::path::Component::Prefix(px)) => Some(px.as_os_str().to_string_lossy().to_uppercase()),
        _ => None,
    }
}

fn remember(dir: &Path) {
    let mut v = TEMP_DIRS.lock().unwrap();
    if !v.iter().any(|d| d == dir) {
        v.push(dir.to_path_buf());
        let list: Vec<String> = v.iter().map(|d| d.to_string_lossy().to_string()).collect();
        let _ = std::fs::create_dir_all(app_data());
        let _ = std::fs::write(registry_file(), list.join("\n"));
    }
}

/// Temporary result folder on the same drive as the source, so "save" is an instant rename.
pub fn temp_dir_for(src: &Path) -> PathBuf {
    let base = app_data().join("results");
    let src_drive = drive_of(src);
    if src_drive.is_none() || src_drive == drive_of(&base) {
        let _ = std::fs::create_dir_all(&base);
        remember(&base);
        return base;
    }
    let root = PathBuf::from(format!("{}\\", src_drive.unwrap()));
    let dir = root.join(".OfflineConvertor-temp");
    if std::fs::create_dir_all(&dir).is_ok() {
        // Hidden, so it doesn't clutter the drive while results wait to be saved.
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("attrib").arg("+h").arg(&dir).creation_flags(super::ffmpeg::CREATE_NO_WINDOW).status();
        remember(&dir);
        dir
    } else {
        let _ = std::fs::create_dir_all(&base);
        remember(&base);
        base
    }
}

pub fn is_temp(p: &Path) -> bool {
    let v = TEMP_DIRS.lock().unwrap();
    p.parent().is_some_and(|parent| v.iter().any(|d| d == parent))
}

/// Deletes every temporary result (this session's and leftovers from a previous crash).
pub fn cleanup_all() {
    let mut dirs: Vec<PathBuf> = TEMP_DIRS.lock().unwrap().clone();
    if let Ok(t) = std::fs::read_to_string(registry_file()) {
        dirs.extend(t.lines().filter(|l| !l.trim().is_empty()).map(PathBuf::from));
    }
    dirs.push(app_data().join("results"));
    for d in dirs {
        // Only our own folders.
        let name = d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name == "results" || name == ".OfflineConvertor-temp" {
            let _ = std::fs::remove_dir_all(&d);
        }
    }
    let _ = std::fs::remove_file(registry_file());
    TEMP_DIRS.lock().unwrap().clear();
}

/// Moves a temporary result to `dest` (rename when on the same drive, copy otherwise).
pub fn move_file(from: &Path, dest: &Path) -> Result<(), String> {
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("Нет доступа к папке: {e}"))?;
    }
    if std::fs::rename(from, dest).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, dest).map_err(|e| format!("Не удалось сохранить: {e}"))?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

/// Saves into a folder under the result's own name, never replacing anything.
pub fn move_into_folder(from: &Path, folder: &Path) -> Result<PathBuf, String> {
    let name = from.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
    // Strip the " (n)" that only existed to avoid clashes inside the temp folder.
    let (stem, ext) = match name.rfind('.') {
        Some(i) => (name[..i].to_string(), name[i + 1..].to_string()),
        None => (name.clone(), String::new()),
    };
    let stem = regex::Regex::new(r" \(\d+\)$").unwrap().replace(&stem, "").to_string();
    let fake_src = folder.join(format!("{stem}.__src__"));
    let dest = reserve(&fake_src, Some(folder), "", &ext)?;
    match move_file(from, &dest) {
        Ok(()) => Ok(dest),
        Err(e) => {
            discard(&dest);
            Err(e)
        }
    }
}
