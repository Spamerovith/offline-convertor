//! Unlimited parallel job queue: a GPU lane for video and a CPU lane for everything else.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::engine::{self, Ctx, JobOutput, CANCELLED};
use crate::settings;

#[derive(Debug, Clone, Deserialize)]
pub struct JobSpec {
    pub id: String,
    /// "video", "audio", "image", "pdf", "ocr", "auto"
    pub tool: String,
    pub path: String,
    #[serde(default)]
    pub options: serde_json::Value,
    #[serde(default)]
    pub out_dir: Option<String>,
    /// Keep the result in a temporary folder until the user saves it.
    #[serde(default)]
    pub temp: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobEvent {
    pub id: String,
    /// "running", "done", "error", "cancelled"
    pub state: String,
    pub progress: f64,
    pub stage: Option<String>,
    pub output: Option<JobOutput>,
    pub error: Option<String>,
}

struct Lane {
    q: Mutex<VecDeque<JobSpec>>,
    cv: Condvar,
    limit: AtomicU32,
}

struct Queue {
    gpu: Lane,
    cpu: Lane,
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    app: AppHandle,
}

static QUEUE: OnceLock<Arc<Queue>> = OnceLock::new();

pub const VIDEO_EXT: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "webm", "flv", "wmv", "m4v", "mpg", "mpeg", "ts", "mts", "m2ts", "3gp", "3g2", "ogv", "vob", "mxf", "f4v", "asf", "rm", "rmvb", "divx", "dv",
];
pub const AUDIO_EXT: &[&str] = &[
    "mp3", "wav", "flac", "aac", "m4a", "ogg", "oga", "opus", "wma", "aiff", "aif", "alac", "ac3", "eac3", "dts", "amr", "ape", "wv", "mka", "mp2", "caf", "tta", "weba",
];
pub const IMAGE_EXT: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "webp", "avif", "heic", "heif", "jxl", "gif", "bmp", "tif", "tiff", "ico", "tga", "psd", "qoi", "pnm", "ppm", "pgm", "hdr", "dds", "jp2", "exr",
];

pub fn kind_of(p: &Path) -> &'static str {
    let e = engine::paths::ext_of(p);
    if VIDEO_EXT.contains(&e.as_str()) {
        "video"
    } else if AUDIO_EXT.contains(&e.as_str()) {
        "audio"
    } else if IMAGE_EXT.contains(&e.as_str()) {
        "image"
    } else if e == "pdf" {
        "pdf"
    } else {
        "other"
    }
}

fn lane_for(spec: &JobSpec) -> bool {
    // true = GPU lane
    match spec.tool.as_str() {
        "video" => true,
        "auto" => kind_of(Path::new(&spec.path)) == "video",
        _ => false,
    }
}

pub fn init(app: AppHandle) {
    let s = settings::get();
    let cpu_threads = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
    let q = Arc::new(Queue {
        gpu: Lane { q: Mutex::new(VecDeque::new()), cv: Condvar::new(), limit: AtomicU32::new(s.video_parallel.clamp(1, 8)) },
        cpu: Lane {
            q: Mutex::new(VecDeque::new()),
            cv: Condvar::new(),
            limit: AtomicU32::new(if s.cpu_parallel == 0 { cpu_threads } else { s.cpu_parallel }.clamp(1, 64)),
        },
        cancels: Mutex::new(HashMap::new()),
        app,
    });
    let _ = QUEUE.set(q.clone());
    for i in 0..32u32 {
        let q = q.clone();
        std::thread::Builder::new().name(format!("gpu-{i}")).spawn(move || worker(q, true, i)).ok();
    }
    // Once the video devices are known, allow N videos per GPU.
    std::thread::spawn(move || {
        let _ = engine::gpu::caps();
        let s = settings::get();
        apply_limits(s.video_parallel, s.cpu_parallel);
    });
    for i in 0..cpu_threads.clamp(1, 64) {
        let q = q.clone();
        std::thread::Builder::new().name(format!("cpu-{i}")).spawn(move || worker(q, false, i)).ok();
    }
}

pub fn apply_limits(video: u32, cpu: u32) {
    if let Some(q) = QUEUE.get() {
        let cpu_threads = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
        // Videos run per GPU: with "all GPUs" every device gets its own share.
        let s = settings::get();
        let devices = if s.gpu_device == "auto" || s.gpu_device.is_empty() { engine::gpu::job_devices().len().max(1) as u32 } else { 1 };
        q.gpu.limit.store((video.clamp(1, 8) * devices).min(32), Ordering::Relaxed);
        q.cpu.limit.store(if cpu == 0 { cpu_threads } else { cpu }.clamp(1, 64), Ordering::Relaxed);
        q.gpu.cv.notify_all();
        q.cpu.cv.notify_all();
    }
}

pub fn enqueue(specs: Vec<JobSpec>) {
    let Some(q) = QUEUE.get() else { return };
    let mut cancels = q.cancels.lock().unwrap();
    let (mut g, mut c) = (vec![], vec![]);
    for s in specs {
        cancels.insert(s.id.clone(), Arc::new(AtomicBool::new(false)));
        if lane_for(&s) {
            g.push(s)
        } else {
            c.push(s)
        }
    }
    drop(cancels);
    if !g.is_empty() {
        q.gpu.q.lock().unwrap().extend(g);
        q.gpu.cv.notify_all();
    }
    if !c.is_empty() {
        q.cpu.q.lock().unwrap().extend(c);
        q.cpu.cv.notify_all();
    }
}

pub fn cancel(id: &str) {
    let Some(q) = QUEUE.get() else { return };
    if let Some(f) = q.cancels.lock().unwrap().get(id) {
        f.store(true, Ordering::Relaxed);
    }
    // Remove it from the waiting lists too.
    for lane in [&q.gpu, &q.cpu] {
        let mut l = lane.q.lock().unwrap();
        let before = l.len();
        l.retain(|s| s.id != id);
        if l.len() != before {
            emit(&q.app, JobEvent { id: id.into(), state: "cancelled".into(), progress: 0.0, stage: None, output: None, error: None });
        }
    }
}

pub fn cancel_all() {
    let Some(q) = QUEUE.get() else { return };
    let ids: Vec<String> = {
        let mut ids: Vec<String> = q.gpu.q.lock().unwrap().iter().map(|s| s.id.clone()).collect();
        ids.extend(q.cpu.q.lock().unwrap().iter().map(|s| s.id.clone()));
        ids
    };
    for f in q.cancels.lock().unwrap().values() {
        f.store(true, Ordering::Relaxed);
    }
    q.gpu.q.lock().unwrap().clear();
    q.cpu.q.lock().unwrap().clear();
    for id in ids {
        emit(&q.app, JobEvent { id, state: "cancelled".into(), progress: 0.0, stage: None, output: None, error: None });
    }
}

fn emit(app: &AppHandle, e: JobEvent) {
    let _ = app.emit("job", e);
}

fn worker(q: Arc<Queue>, gpu: bool, index: u32) {
    engine::ocr::init_thread();
    let lane = if gpu { &q.gpu } else { &q.cpu };
    loop {
        let spec = {
            let mut l = lane.q.lock().unwrap();
            loop {
                if index < lane.limit.load(Ordering::Relaxed) {
                    if let Some(s) = l.pop_front() {
                        break s;
                    }
                }
                l = lane.cv.wait_timeout(l, Duration::from_millis(500)).unwrap().0;
            }
        };
        run_job(&q, spec);
    }
}

fn run_job(q: &Queue, spec: JobSpec) {
    let cancel = q.cancels.lock().unwrap().get(&spec.id).cloned().unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    let app = q.app.clone();
    let id = spec.id.clone();
    let last = Mutex::new((Instant::now() - Duration::from_secs(1), String::new()));
    let progress = |f: f64, stage: Option<String>| {
        let mut l = last.lock().unwrap();
        let stage_changed = stage.as_ref().is_some_and(|s| *s != l.1);
        if !stage_changed && l.0.elapsed() < Duration::from_millis(120) && f < 1.0 {
            return;
        }
        l.0 = Instant::now();
        if let Some(s) = &stage {
            l.1 = s.clone();
        }
        emit(&app, JobEvent { id: id.clone(), state: "running".into(), progress: f, stage, output: None, error: None });
    };
    progress(0.0, Some("Запуск".into()));
    let src = PathBuf::from(&spec.path);
    let out_dir = if spec.temp {
        Some(engine::paths::temp_dir_for(&src))
    } else {
        spec.out_dir.clone().filter(|s| !s.trim().is_empty()).map(PathBuf::from)
    };
    let ctx = Ctx { cancel: &cancel, progress: &progress, out_dir };
    let opts = spec.options.clone();

    let tool = if spec.tool == "auto" {
        match kind_of(&src) {
            "video" => "video",
            "audio" => "audio",
            "image" => "image",
            "pdf" => "pdf",
            _ => "other",
        }
    } else {
        spec.tool.as_str()
    };
    // Batch mode passes per-kind option blocks: {"video": {...}, "image": {...}, ...}
    let pick = |k: &str| -> serde_json::Value {
        if spec.tool == "auto" {
            opts.get(k).cloned().unwrap_or(serde_json::json!({}))
        } else {
            opts.clone()
        }
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> engine::JobResult {
        if !src.is_file() {
            return Err("Файл не найден".into());
        }
        match tool {
            "video" => {
                let o: engine::video::VideoOpts = serde_json::from_value(pick("video")).map_err(|e| e.to_string())?;
                engine::video::run(&src, &o, &ctx)
            }
            "audio" => {
                let mut o: engine::audio::AudioOpts = serde_json::from_value(pick("audio")).map_err(|e| e.to_string())?;
                if spec.tool == "auto" {
                    o.mode = "compress".into();
                }
                engine::audio::run(&src, &o, &ctx)
            }
            "image" => {
                let o: engine::image_ops::ImageOpts = serde_json::from_value(pick("image")).map_err(|e| e.to_string())?;
                engine::image_ops::run(&src, &o, &ctx)
            }
            "pdf" => {
                let o: engine::pdf::PdfOpts = serde_json::from_value(pick("pdf")).map_err(|e| e.to_string())?;
                engine::pdf::run(&src, &o, &ctx)
            }
            "ocr" => {
                let o: engine::ocr::OcrOpts = serde_json::from_value(pick("ocr")).map_err(|e| e.to_string())?;
                engine::ocr::run(&src, &o, &ctx)
            }
            _ => Err("Этот тип файла не поддерживается".into()),
        }
    }))
    .unwrap_or_else(|_| Err("Внутренняя ошибка при обработке файла".into()));

    let ev = match result {
        Ok(out) => JobEvent { id: spec.id.clone(), state: "done".into(), progress: 1.0, stage: None, output: Some(out), error: None },
        Err(e) if e == CANCELLED || cancel.load(Ordering::Relaxed) => {
            JobEvent { id: spec.id.clone(), state: "cancelled".into(), progress: 0.0, stage: None, output: None, error: None }
        }
        Err(e) => JobEvent { id: spec.id.clone(), state: "error".into(), progress: 0.0, stage: None, output: None, error: Some(e) },
    };
    emit(&q.app, ev);
    q.cancels.lock().unwrap().remove(&spec.id);
}
