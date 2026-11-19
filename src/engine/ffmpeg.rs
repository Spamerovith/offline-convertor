//! Locating and running the bundled ffmpeg: progress parsing, cancellation, media probing.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use regex::Regex;
use serde::Serialize;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Path to ffmpeg.exe: `<exe dir>/engine/ffmpeg.exe`, then next to the exe, then the engine packed
/// inside a single-file build (unpacked once to %LOCALAPPDATA%), then PATH.
pub fn ffmpeg_path() -> &'static PathBuf {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                for c in [dir.join("engine").join("ffmpeg.exe"), dir.join("ffmpeg.exe")] {
                    if c.is_file() {
                        return c;
                    }
                }
            }
        }
        #[cfg(embedded_engine)]
        if let Some(p) = unpack_embedded() {
            return p;
        }
        PathBuf::from("ffmpeg.exe")
    })
}

#[cfg(embedded_engine)]
fn unpack_embedded() -> Option<PathBuf> {
    static PACKED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ffmpeg.zst"));
    let size: u64 = env!("OC_ENGINE_SIZE").parse().ok()?;
    let dir = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("OfflineConvertor").join("engine");
    let path = dir.join(format!("ffmpeg-{}.exe", env!("OC_ENGINE_ID")));
    if std::fs::metadata(&path).map(|m| m.len() == size).unwrap_or(false) {
        return Some(path);
    }
    std::fs::create_dir_all(&dir).ok()?;
    // Older engine versions are no longer needed.
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.path() != path {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let tmp = dir.join(format!("unpack-{}.tmp", std::process::id()));
    let ok = (|| -> std::io::Result<()> {
        let mut dec = zstd::stream::Decoder::new(PACKED)?;
        let mut f = std::fs::File::create(&tmp)?;
        std::io::copy(&mut dec, &mut f)?;
        f.sync_all()?;
        Ok(())
    })();
    if ok.is_err() || std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0) != size {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    if std::fs::rename(&tmp, &path).is_err() {
        // Another instance unpacked it at the same moment.
        let _ = std::fs::remove_file(&tmp);
    }
    path.is_file().then_some(path)
}

pub fn ffmpeg_available() -> bool {
    Command::new(ffmpeg_path())
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn command() -> Command {
    let mut c = Command::new(ffmpeg_path());
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct VideoStream {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub bitrate_kbps: Option<u32>,
    pub ten_bit: bool,
    pub hdr: bool,
    pub pix_fmt: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AudioStream {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub bitrate_kbps: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MediaInfo {
    pub duration: f64,
    pub bitrate_kbps: Option<u32>,
    pub format: String,
    pub video: Option<VideoStream>,
    pub audio: Vec<AudioStream>,
    pub has_subtitles: bool,
    pub has_cover: bool,
}

fn re(p: &str) -> Regex {
    Regex::new(p).unwrap()
}

/// Parses `ffmpeg -i` stderr into a MediaInfo (no ffprobe needed).
pub fn probe(path: &Path) -> Result<MediaInfo, String> {
    let out = command()
        .arg("-hide_banner")
        .arg("-i")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Не удалось запустить движок ffmpeg: {e}"))?;
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    parse_probe(&text)
}

pub fn parse_probe(text: &str) -> Result<MediaInfo, String> {
    static RE: OnceLock<[Regex; 7]> = OnceLock::new();
    let [r_dur, r_br, r_res, r_fps, r_kbps, r_hz, r_input] = RE.get_or_init(|| {
        [
            re(r"Duration:\s*(\d+):(\d+):(\d+(?:\.\d+)?)"),
            re(r"Duration:.*bitrate:\s*(\d+)\s*kb/s"),
            re(r",\s*(\d{2,5})x(\d{2,5})[\s,\[]"),
            re(r"([\d.]+)\s*fps"),
            re(r"(\d+)\s*kb/s"),
            re(r"(\d+)\s*Hz"),
            re(r"Input #0,\s*([^,]+)"),
        ]
    });
    if !text.contains("Input #0") {
        let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("неизвестный формат");
        return Err(format!("Файл не распознан: {}", last.trim()));
    }
    let mut info = MediaInfo::default();
    if let Some(c) = r_input.captures(text) {
        info.format = c[1].trim().to_string();
    }
    if let Some(c) = r_dur.captures(text) {
        let h: f64 = c[1].parse().unwrap_or(0.0);
        let m: f64 = c[2].parse().unwrap_or(0.0);
        let s: f64 = c[3].parse().unwrap_or(0.0);
        info.duration = h * 3600.0 + m * 60.0 + s;
    }
    if let Some(c) = r_br.captures(text) {
        info.bitrate_kbps = c[1].parse().ok();
    }
    for line in text.lines() {
        let l = line.trim();
        if !l.starts_with("Stream #") {
            continue;
        }
        if let Some(idx) = l.find("Video: ") {
            if l.contains("attached pic") || l.contains("(attached_pic)") {
                info.has_cover = true;
                continue;
            }
            if info.video.is_some() {
                continue;
            }
            let rest = &l[idx + 7..];
            let codec = rest.split(|c: char| c == ' ' || c == ',').next().unwrap_or("").to_string();
            let mut v = VideoStream { codec, ..Default::default() };
            if let Some(c) = r_res.captures(&format!("{rest} ")) {
                v.width = c[1].parse().unwrap_or(0);
                v.height = c[2].parse().unwrap_or(0);
            }
            if let Some(c) = r_fps.captures(rest) {
                v.fps = c[1].parse().unwrap_or(0.0);
            }
            if let Some(c) = r_kbps.captures(rest) {
                v.bitrate_kbps = c[1].parse().ok();
            }
            // pixel format is the first token of the second comma group
            let mut parts = rest.split(", ");
            parts.next();
            if let Some(pf) = parts.next() {
                v.pix_fmt = pf.split(|c: char| c == '(' || c == ' ').next().unwrap_or("").to_string();
            }
            let pf = v.pix_fmt.as_str();
            v.ten_bit = pf.contains("10") || pf.contains("12") || pf.contains("p016");
            v.hdr = l.contains("smpte2084") || l.contains("arib-std-b67");
            info.video = Some(v);
        } else if let Some(idx) = l.find("Audio: ") {
            let rest = &l[idx + 7..];
            let codec = rest.split(|c: char| c == ' ' || c == ',').next().unwrap_or("").to_string();
            let mut a = AudioStream { codec, ..Default::default() };
            if let Some(c) = r_hz.captures(rest) {
                a.sample_rate = c[1].parse().unwrap_or(0);
            }
            a.channels = if rest.contains("mono") {
                1
            } else if rest.contains("stereo") {
                2
            } else if rest.contains("5.1") {
                6
            } else if rest.contains("7.1") {
                8
            } else {
                2
            };
            if let Some(c) = r_kbps.captures(rest) {
                a.bitrate_kbps = c[1].parse().ok();
            }
            info.audio.push(a);
        } else if l.contains("Subtitle: ") {
            info.has_subtitles = true;
        }
    }
    Ok(info)
}

/// Progress callback: (fraction 0..1, speed multiplier).
pub type ProgressFn<'a> = &'a (dyn Fn(f64, f64) + Sync);

pub struct RunResult {
    pub ok: bool,
    pub cancelled: bool,
    pub error_tail: String,
}

/// Runs ffmpeg with progress reporting. `duration` is the expected output duration in seconds.
pub fn run(args: &[OsString], duration: f64, cancel: &AtomicBool, progress: ProgressFn) -> RunResult {
    let mut cmd = command();
    cmd.args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-progress", "pipe:1", "-nostats"]);
    cmd.args(args);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child: Child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return RunResult { ok: false, cancelled: false, error_tail: format!("Не удалось запустить ffmpeg: {e}") }
        }
    };
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let child = Arc::new(Mutex::new(child));
    let done = AtomicBool::new(false);
    let mut err_text = String::new();
    let status = std::thread::scope(|s| {
        let err_thread = s.spawn(move || {
            let mut t = String::new();
            let _ = stderr.read_to_string(&mut t);
            t
        });
        // Watchdog: kills ffmpeg as soon as the job is cancelled.
        let ch = child.clone();
        let done_ref = &done;
        s.spawn(move || {
            while !done_ref.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) {
                    let _ = ch.lock().unwrap().kill();
                    break;
                }
                std::thread::sleep(Duration::from_millis(80));
            }
        });
        let reader = BufReader::new(stdout);
        let mut speed = 0.0;
        for line in reader.lines().map_while(Result::ok) {
            if let Some(v) = line.strip_prefix("speed=") {
                speed = v.trim().trim_end_matches('x').parse().unwrap_or(speed);
            } else if let Some(v) = line.strip_prefix("out_time_us=") {
                if let Ok(us) = v.trim().parse::<f64>() {
                    if duration > 0.0 {
                        progress((us / 1e6 / duration).clamp(0.0, 0.999), speed);
                    }
                }
            } else if line.starts_with("progress=end") {
                progress(1.0, speed);
            }
        }
        // Poll so the watchdog can still take the lock and kill the process.
        let st = loop {
            match child.lock().unwrap().try_wait() {
                Ok(Some(s)) => break Ok(s),
                Ok(None) => {}
                Err(e) => break Err(e),
            }
            std::thread::sleep(Duration::from_millis(30));
        };
        done.store(true, Ordering::Relaxed);
        err_text = err_thread.join().unwrap_or_default();
        st
    });
    let cancelled = cancel.load(Ordering::Relaxed);
    let ok = !cancelled && status.map(|s| s.success()).unwrap_or(false);
    let tail = err_text;
    let tail: Vec<&str> = tail.lines().filter(|l| !l.trim().is_empty()).collect();
    let n = tail.len();
    RunResult { ok, cancelled, error_tail: tail[n.saturating_sub(6)..].join("\n") }
}

/// Runs ffmpeg once without progress, returning success (used for capability tests).
pub fn quick_ok(args: &[&str]) -> bool {
    let mut c = command();
    c.args(["-hide_banner", "-nostdin", "-loglevel", "error"]);
    c.args(args);
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    match c.spawn() {
        Ok(mut ch) => {
            let start = std::time::Instant::now();
            loop {
                match ch.try_wait() {
                    Ok(Some(s)) => return s.success(),
                    Ok(None) => {
                        if start.elapsed() > Duration::from_secs(20) {
                            let _ = ch.kill();
                            return false;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => return false,
                }
            }
        }
        Err(_) => false,
    }
}

/// Decodes any image ffmpeg understands (HEIC, AVIF, JXL, …) into RGBA8.
pub fn decode_image_rgba(path: &Path) -> Result<image::RgbaImage, String> {
    let out = command()
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "png", "-pix_fmt", "rgba", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() || out.stdout.is_empty() {
        return Err(format!("Не удалось прочитать изображение: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    image::load_from_memory_with_format(&out.stdout, image::ImageFormat::Png)
        .map(|i| i.to_rgba8())
        .map_err(|e| e.to_string())
}

/// Encodes raw RGBA through ffmpeg (WebP, AVIF, JXL) and returns the encoded bytes.
pub fn encode_rgba(img: &image::RgbaImage, codec_args: &[String], format: &str) -> Result<Vec<u8>, String> {
    use std::io::Write;
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let (w, h) = img.dimensions();
    // Some muxers (AVIF) need a seekable output, so encode into a private temp file.
    let tmp = std::env::temp_dir().join(format!(
        "oc-{}-{}.{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed),
        if format == "image2pipe" { "jxl" } else { format }
    ));
    let mut c = command();
    c.args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba", "-s"])
        .arg(format!("{w}x{h}"))
        .args(["-i", "-"])
        .args(codec_args)
        .args(["-f", if format == "image2pipe" { "image2" } else { format }])
        .arg(&tmp)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut ch = c.spawn().map_err(|e| e.to_string())?;
    let mut stdin = ch.stdin.take().unwrap();
    let raw = img.as_raw().clone();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&raw);
    });
    let out = ch.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    let data = std::fs::read(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    if !out.status.success() || data.is_empty() {
        return Err(format!("Ошибка кодирования: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(data)
}

pub fn os(v: impl Into<OsString>) -> OsString {
    v.into()
}
