//! Audio conversion and compression (also extracts audio from video files).

use std::ffi::OsString;
use std::path::Path;

use serde::Deserialize;

use super::ffmpeg::{self, os};
use super::paths;
use super::{output, Ctx, JobResult, CANCELLED};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AudioOpts {
    /// "compress" or "convert"
    pub mode: String,
    /// "same", "mp3", "aac", "m4a", "opus", "ogg", "flac", "wav", "aiff", "alac", "wma", "ac3"
    pub format: String,
    pub quality: f64,
    /// None or 0 = automatic
    pub bitrate_kbps: Option<u32>,
    pub target_mb: Option<f64>,
    pub sample_rate: u32,
    pub channels: u32,
    pub normalize: bool,
    pub trim_start: Option<f64>,
    pub trim_end: Option<f64>,
    /// 16 or 24 for wav/aiff/flac
    pub bit_depth: u32,
}

impl Default for AudioOpts {
    fn default() -> Self {
        Self {
            mode: "convert".into(),
            format: "mp3".into(),
            quality: 85.0,
            bitrate_kbps: None,
            target_mb: None,
            sample_rate: 0,
            channels: 0,
            normalize: false,
            trim_start: None,
            trim_end: None,
            bit_depth: 16,
        }
    }
}

const LOSSY: &[&str] = &["mp3", "aac", "m4a", "opus", "ogg", "wma", "ac3", "oga", "mka", "weba"];

fn resolve_format(src: &Path, o: &AudioOpts) -> String {
    let f = o.format.to_lowercase();
    if f != "same" && !f.is_empty() {
        return f;
    }
    let e = paths::ext_of(src);
    if o.mode == "compress" {
        return match e.as_str() {
            "mp3" | "opus" | "ogg" | "wma" | "ac3" => e,
            "m4a" | "aac" | "mp4" => "m4a".into(),
            _ => "mp3".into(), // lossless or video sources → MP3
        };
    }
    if LOSSY.contains(&e.as_str()) || matches!(e.as_str(), "flac" | "wav" | "aiff") {
        e
    } else {
        "mp3".into()
    }
}

fn ext_for(fmt: &str) -> &str {
    match fmt {
        "alac" | "aac_m4a" => "m4a",
        "aiff" => "aiff",
        other => other,
    }
}

pub fn run(src: &Path, o: &AudioOpts, ctx: &Ctx) -> JobResult {
    ctx.stage(0.0, "Анализ файла");
    let info = ffmpeg::probe(src)?;
    if info.audio.is_empty() {
        return Err("В файле нет звуковой дорожки".into());
    }
    let fmt = resolve_format(src, o);
    let suffix = if o.mode == "compress" { "_compressed" } else { "" };
    let dst = paths::reserve(src, ctx.out_dir.as_deref(), suffix, ext_for(&fmt))?;
    let src_size = super::file_size(src);
    let start = o.trim_start.unwrap_or(0.0).max(0.0);
    let mut dur = (info.duration - start).max(0.0);
    if let Some(e) = o.trim_end.filter(|e| *e > start) {
        dur = dur.min(e - start);
    }

    let q = o.quality.clamp(0.0, 100.0);
    let mut kbps: Option<u32> = o.bitrate_kbps.filter(|k| *k > 0);
    if let Some(mb) = o.target_mb.filter(|m| *m > 0.0) {
        if dur > 0.0 {
            kbps = Some(((mb * 1024.0 * 1024.0 * 8.0 / 1000.0 / dur) * 0.98).clamp(8.0, 512.0) as u32);
        }
    }

    let mut a: Vec<OsString> = vec![];
    if start > 0.0 {
        a.extend([os("-ss"), os(format!("{start:.3}"))]);
    }
    a.push(os("-i"));
    a.push(src.as_os_str().to_owned());
    if let Some(e) = o.trim_end.filter(|e| *e > start) {
        a.extend([os("-t"), os(format!("{:.3}", e - start))]);
    }
    a.extend(["-map", "0:a:0"].map(os));
    let cover = info.has_cover && matches!(fmt.as_str(), "mp3" | "flac" | "m4a" | "alac" | "aac_m4a");
    if cover {
        a.extend(["-map", "0:v:0?", "-c:v", "copy", "-disposition:v:0", "attached_pic"].map(os));
    } else {
        a.push(os("-vn"));
    }
    a.extend(["-map_metadata", "0"].map(os));

    let compress = o.mode == "compress";
    match fmt.as_str() {
        "mp3" => {
            a.extend(["-c:a", "libmp3lame"].map(os));
            match kbps {
                Some(k) => a.extend([os("-b:a"), os(format!("{}k", k.clamp(8, 320)))]),
                None => {
                    let v = if compress { ((100.0 - q) / 10.0).round().clamp(0.0, 9.0) } else { 0.0 };
                    a.extend([os("-q:a"), os(format!("{v:.0}"))]);
                }
            }
            a.extend(["-id3v2_version", "3"].map(os));
        }
        "aac" | "m4a" => {
            let k = kbps.unwrap_or(if compress { (32.0 + q * 1.5) as u32 } else { 256 });
            a.extend([os("-c:a"), os("aac"), os("-b:a"), os(format!("{}k", k.clamp(16, 512)))]);
            if fmt == "aac" {
                a.extend(["-f", "adts"].map(os));
            } else {
                a.extend(["-movflags", "+faststart"].map(os));
            }
        }
        "opus" => {
            let k = kbps.unwrap_or(if compress { (24.0 + q * 1.04) as u32 } else { 160 });
            a.extend([os("-c:a"), os("libopus"), os("-b:a"), os(format!("{}k", k.clamp(6, 510))), os("-vbr"), os("on")]);
            a.extend(["-compression_level", "10"].map(os));
        }
        "ogg" => {
            a.extend(["-c:a", "libvorbis"].map(os));
            match kbps {
                Some(k) => a.extend([os("-b:a"), os(format!("{k}k"))]),
                None => a.extend([os("-q:a"), os(format!("{:.1}", if compress { q / 10.0 } else { 6.0 }))]),
            }
        }
        "flac" => {
            a.extend(["-c:a", "flac", "-compression_level", "8"].map(os));
            if o.bit_depth == 24 {
                a.extend(["-sample_fmt", "s32"].map(os));
            } else {
                a.extend(["-sample_fmt", "s16"].map(os));
            }
        }
        "wav" => a.extend([os("-c:a"), os(if o.bit_depth == 24 { "pcm_s24le" } else { "pcm_s16le" })]),
        "aiff" => a.extend([os("-c:a"), os(if o.bit_depth == 24 { "pcm_s24be" } else { "pcm_s16be" })]),
        "alac" => a.extend(["-c:a", "alac"].map(os)),
        "wma" => {
            let k = kbps.unwrap_or(if compress { (48.0 + q * 1.28) as u32 } else { 192 });
            a.extend([os("-c:a"), os("wmav2"), os("-b:a"), os(format!("{k}k"))]);
        }
        "ac3" => {
            let k = kbps.unwrap_or(if compress { (128.0 + q * 2.56) as u32 } else { 448 });
            a.extend([os("-c:a"), os("ac3"), os("-b:a"), os(format!("{}k", k.clamp(64, 640)))]);
        }
        other => return Err(format!("Формат {other} не поддерживается")),
    }
    if o.sample_rate > 0 {
        a.extend([os("-ar"), os(o.sample_rate.to_string())]);
    } else if fmt == "opus" {
        a.extend(["-ar", "48000"].map(os));
    }
    if o.channels > 0 {
        a.extend([os("-ac"), os(o.channels.to_string())]);
    }
    if o.normalize {
        a.extend(["-af", "loudnorm=I=-16:TP=-1.5:LRA=11"].map(os));
    }
    a.push(dst.as_os_str().to_owned());

    ctx.stage(0.0, "Кодирование");
    let progress = |f: f64, speed: f64| {
        (ctx.progress)(f, Some(if speed > 0.0 { format!("Кодирование · {speed:.0}×") } else { "Кодирование".into() }));
    };
    let r = ffmpeg::run(&a, if dur > 0.0 { dur } else { 1.0 }, ctx.cancel, &progress);
    if r.cancelled {
        paths::discard(&dst);
        return Err(CANCELLED.into());
    }
    if !r.ok {
        paths::discard(&dst);
        return Err(if r.error_tail.is_empty() { "Ошибка кодирования".into() } else { r.error_tail });
    }
    let mut note = None;
    if compress && super::file_size(&dst) >= src_size && paths::ext_of(&dst) == paths::ext_of(src) {
        let _ = std::fs::copy(src, &dst);
        note = Some("Аудио уже сжато оптимально - сохранена копия".into());
    }
    Ok(output(&dst, note))
}
