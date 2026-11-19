pub mod audio;
pub mod ffmpeg;
pub mod gpu;
pub mod image_ops;
pub mod layout;
pub mod ocr;
pub mod paths;
pub mod pdf;
pub mod video;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

/// Per-job context handed to every engine function.
pub struct Ctx<'a> {
    pub cancel: &'a AtomicBool,
    /// (fraction 0..1, optional stage text)
    pub progress: &'a (dyn Fn(f64, Option<String>) + Sync),
    pub out_dir: Option<PathBuf>,
}

impl Ctx<'_> {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    pub fn stage(&self, f: f64, s: &str) {
        (self.progress)(f, Some(s.to_string()));
    }
    pub fn set(&self, f: f64) {
        (self.progress)(f, None);
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JobOutput {
    pub path: String,
    pub size: u64,
    pub note: Option<String>,
    /// Recognised text for OCR jobs (shown in the preview).
    pub text: Option<String>,
}

pub type JobResult = Result<JobOutput, String>;

pub const CANCELLED: &str = "__cancelled__";

pub fn file_size(p: &std::path::Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

pub fn output(path: &std::path::Path, note: Option<String>) -> JobOutput {
    JobOutput { path: path.to_string_lossy().to_string(), size: file_size(path), note, text: None }
}
