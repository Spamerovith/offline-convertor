//! Video compression and conversion on NVENC/NVDEC with automatic CPU fallback.

use std::ffi::OsString;
use std::path::Path;

use serde::Deserialize;

use super::ffmpeg::{self, os, MediaInfo};
use super::gpu::{self, Capabilities, EncDevice};
use super::paths;
use super::{output, Ctx, JobResult, CANCELLED};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct VideoOpts {
    /// "compress" or "convert"
    pub mode: String,
    /// Output container: "same", "mp4", "mkv", "webm", "mov", "avi", "flv", "wmv", "mpeg", "3gp", "ts", "m4v", "ogv", "gif"
    pub format: String,
    /// "auto", "av1", "hevc", "h264", "vp9", "copy"
    pub codec: String,
    /// 0..100: 0 = best compression, 100 = best quality
    pub quality: f64,
    pub target_mb: Option<f64>,
    pub bitrate_kbps: Option<u32>,
    /// Target height; 0 keeps the original
    pub height: u32,
    /// 0 keeps the original
    pub fps: f64,
    /// "auto", "copy", "aac", "opus", "mp3", "none"
    pub audio: String,
    pub audio_kbps: u32,
    pub trim_start: Option<f64>,
    pub trim_end: Option<f64>,
    /// "auto", "gpu", "cpu"
    pub hw: String,
    /// "quality" (UHQ where supported), "balanced", "fast"
    pub speed: String,
    pub gif_fps: f64,
    pub gif_width: u32,
}

impl Default for VideoOpts {
    fn default() -> Self {
        Self {
            mode: "compress".into(),
            format: "same".into(),
            codec: "auto".into(),
            quality: 85.0,
            target_mb: None,
            bitrate_kbps: None,
            height: 0,
            fps: 0.0,
            audio: "auto".into(),
            audio_kbps: 0,
            trim_start: None,
            trim_end: None,
            hw: "auto".into(),
            speed: "balanced".into(),
            gif_fps: 15.0,
            gif_width: 480,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Family {
    Av1,
    Hevc,
    H264,
    Vp9,
    Mpeg4,
    Wmv2,
    Mpeg2,
    Theora,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hw {
    Nvenc,
    Qsv,
    Amf,
    /// Media Foundation: the Windows hardware encoder of any GPU
    Mf,
    Cpu,
}

/// Fallback ladder, tried in order until one works:
/// FullGpu = NVDEC, CUDA filters, NVENC (frames never leave the GPU);
/// HwDecode = hardware decode (CUDA or D3D11 for any vendor) + the device's encoder;
/// MfEnc = Media Foundation hardware encoder; Cpu = software only.
#[derive(Clone, Copy, PartialEq, PartialOrd, Debug)]
enum Level {
    FullGpu,
    HwDecode,
    MfEnc,
    Cpu,
}

fn supports(d: &EncDevice, fam: Family) -> bool {
    match fam {
        Family::Av1 => d.av1,
        Family::Hevc => d.hevc,
        Family::H264 => d.h264,
        _ => false,
    }
}

fn hw_of(level: Level, dev: Option<&EncDevice>, fam: Option<Family>) -> Hw {
    let Some(f) = fam else { return Hw::Cpu };
    match level {
        Level::Cpu => Hw::Cpu,
        Level::MfEnc => match gpu::mf_device() {
            Some(m) if supports(m, f) => Hw::Mf,
            _ => Hw::Cpu,
        },
        _ => match dev.map(|d| d.kind.as_str()) {
            Some("nvenc") => Hw::Nvenc,
            Some("amf") => Hw::Amf,
            Some("qsv") => Hw::Qsv,
            Some("mf") => Hw::Mf,
            _ => Hw::Cpu,
        },
    }
}

fn container_for(src: &Path, o: &VideoOpts) -> String {
    let f = o.format.to_lowercase();
    if f != "same" && !f.is_empty() {
        return f;
    }
    let e = paths::ext_of(src);
    match e.as_str() {
        "mp4" | "mkv" | "mov" | "webm" | "m4v" => e,
        _ => "mp4".into(),
    }
}

fn family_for(container: &str, o: &VideoOpts, caps: &Capabilities) -> Option<Family> {
    let c = o.codec.as_str();
    let fam = match c {
        "av1" => Family::Av1,
        "hevc" => Family::Hevc,
        "h264" => Family::H264,
        "vp9" => Family::Vp9,
        "copy" => return None,
        _ => {
            // auto
            match container {
                "webm" => {
                    if gpu::job_devices().iter().any(|d| d.av1) || o.mode == "compress" {
                        Family::Av1
                    } else {
                        Family::Vp9
                    }
                }
                "avi" => Family::Mpeg4,
                "wmv" => Family::Wmv2,
                "mpeg" | "mpg" => Family::Mpeg2,
                "ogv" => Family::Theora,
                "flv" | "3gp" | "ts" => Family::H264,
                _ if o.mode == "compress" => match caps.recommended_codec.as_str() {
                    "av1" => Family::Av1,
                    "hevc" => Family::Hevc,
                    _ => Family::H264,
                },
                // conversion keeps maximum compatibility
                "mkv" => Family::Hevc,
                _ => Family::H264,
            }
        }
    };
    // Containers that can't carry the chosen codec.
    let fam = match (container, fam) {
        ("webm", Family::Hevc | Family::H264) => Family::Av1,
        ("mov" | "m4v" | "flv" | "3gp" | "ts", Family::Av1) => Family::Hevc,
        ("flv" | "3gp", Family::Hevc) => Family::H264,
        (_, f) => f,
    };
    Some(fam)
}

/// Maps the 0..100 quality slider to a constant-quality value for each encoder family.
fn cq_for(fam: Family, q: f64, hw: Hw) -> f64 {
    let inv = (100.0 - q.clamp(0.0, 100.0)) / 100.0; // 0 = best quality
    match fam {
        // Calibrated with VMAF on grainy 1080p: the default (85 %) lands at VMAF ≈ 94+.
        Family::Av1 => {
            if hw == Hw::Cpu {
                20.0 + inv * 40.0 // SVT-AV1 CRF 20..60 (85 → 26)
            } else {
                20.0 + inv * 40.0 // NVENC AV1 CQ 20..60 (85 → 26)
            }
        }
        Family::Hevc => 16.0 + inv * 32.0, // 85 → ~20.8
        Family::H264 => 15.0 + inv * 32.0, // 85 → ~19.8
        Family::Vp9 => 18.0 + inv * 40.0,
        _ => 0.0,
    }
}

/// NVENC speed profile. Measured on 1440p60 AV1 (RTX 50 series):
/// "max" UHQ 1.5× realtime; "balanced" HQ on both NVENC engines 3.3× at the same size
/// (VMAF −1 point); "fast" 4×.
struct Speed {
    preset: &'static str,
    uhq: bool,
    split: bool,
    lookahead: &'static str,
    /// Added to CQ so HQ files land near the UHQ size.
    cq_offset: f64,
}

fn speed_preset(o: &VideoOpts) -> Speed {
    match o.speed.as_str() {
        "fast" => Speed { preset: "p5", uhq: false, split: true, lookahead: "20", cq_offset: 2.0 },
        "quality" | "max" => Speed { preset: "p7", uhq: true, split: false, lookahead: "32", cq_offset: 0.0 },
        _ => Speed { preset: "p7", uhq: false, split: true, lookahead: "32", cq_offset: 2.0 },
    }
}

struct Plan {
    args: Vec<OsString>,
    out_duration: f64,
}

#[allow(clippy::too_many_arguments)]
fn build(
    src: &Path,
    dst: &Path,
    info: &MediaInfo,
    o: &VideoOpts,
    dev: Option<&EncDevice>,
    container: &str,
    fam: Option<Family>,
    level: Level,
    video_kbps: Option<u32>,
    audio_kbps_budget: Option<u32>,
) -> Plan {
    let mut a: Vec<OsString> = vec![];
    let v = info.video.clone().unwrap_or_default();
    let hw = hw_of(level, dev, fam);
    let gif = container == "gif";
    let cuda = dev.is_some_and(|d| d.cuda);
    let gpu_index = dev.map(|d| d.index).unwrap_or(0).to_string();
    let full_gpu = level == Level::FullGpu && hw == Hw::Nvenc && cuda && !gif;

    if let Some(s) = o.trim_start.filter(|s| *s > 0.0) {
        a.extend([os("-ss"), os(format!("{s:.3}"))]);
    }
    if !gif {
        match (hw, level) {
            (Hw::Nvenc, Level::FullGpu) if full_gpu => a.extend([
                os("-hwaccel"), os("cuda"), os("-hwaccel_device"), os(&gpu_index), os("-hwaccel_output_format"), os("cuda"), os("-extra_hw_frames"), os("8"),
            ]),
            (Hw::Nvenc, _) if cuda => a.extend([os("-hwaccel"), os("cuda"), os("-hwaccel_device"), os(&gpu_index)]),
            // Any vendor: DXVA/D3D11 hardware decoding (falls back to software by itself).
            (Hw::Amf | Hw::Qsv | Hw::Mf, _) => a.extend(["-hwaccel", "d3d11va"].map(os)),
            _ => {}
        }
    }
    a.push(os("-i"));
    a.push(src.as_os_str().to_owned());

    let start = o.trim_start.unwrap_or(0.0).max(0.0);
    let mut out_duration = (info.duration - start).max(0.0);
    if let Some(e) = o.trim_end.filter(|e| *e > start) {
        let t = e - start;
        a.extend([os("-t"), os(format!("{t:.3}"))]);
        out_duration = out_duration.min(t);
    }

    // ── GIF: palette-optimised, CPU only ─────────────────────────────────────
    if gif {
        let fps = if o.gif_fps > 0.0 { o.gif_fps } else { 15.0 };
        let w = if o.gif_width > 0 && o.gif_width < v.width { o.gif_width as i64 } else { -1 };
        a.extend([
            os("-vf"),
            os(format!(
                "fps={fps},scale={w}:-1:flags=lanczos,split[s0][s1];[s0]palettegen=stats_mode=diff:max_colors=256[p];[s1][p]paletteuse=dither=sierra2_4a:diff_mode=rectangle"
            )),
            os("-loop"),
            os("0"),
            os("-an"),
        ]);
        a.push(dst.as_os_str().to_owned());
        return Plan { args: a, out_duration };
    }

    a.extend(["-map", "0:v:0", "-map", "0:a?", "-map_metadata", "0", "-map_chapters", "0"].map(os));
    if container == "mkv" {
        a.extend(["-map", "0:s?", "-c:s", "copy", "-map", "0:t?"].map(os));
    }

    // ── Video ───────────────────────────────────────────────────────────────
    match fam {
        None => a.extend(["-c:v", "copy"].map(os)),
        Some(fam) => {
            let ten_bit = matches!(fam, Family::Av1 | Family::Hevc)
                && match hw {
                    Hw::Cpu => true,
                    Hw::Mf => false,
                    _ => dev.is_some_and(|d| d.ten_bit),
                };
            let target_h = if o.height > 0 && o.height < v.height { o.height } else { 0 };
            let mut filters: Vec<String> = vec![];
            if o.fps > 0.0 && (v.fps == 0.0 || o.fps < v.fps - 0.01) {
                filters.push(format!("fps={}", o.fps));
            }
            if full_gpu {
                let fmt = if ten_bit { "p010le" } else { "nv12" };
                if target_h > 0 {
                    filters.push(format!("scale_cuda=w=-2:h={target_h}:interp_algo=lanczos:format={fmt}"));
                } else {
                    filters.push(format!("scale_cuda=format={fmt}"));
                }
            } else {
                if target_h > 0 {
                    filters.push(format!("scale=-2:{target_h}:flags=lanczos"));
                }
                let fmt = match (fam, hw, ten_bit) {
                    (_, Hw::Nvenc | Hw::Qsv | Hw::Amf, true) => "p010le",
                    (_, Hw::Nvenc | Hw::Qsv | Hw::Amf | Hw::Mf, _) => "nv12",
                    (Family::Av1 | Family::Hevc, Hw::Cpu, true) => "yuv420p10le",
                    _ => "yuv420p",
                };
                filters.push(format!("format={fmt}"));
            }
            if !filters.is_empty() {
                a.extend([os("-vf"), os(filters.join(","))]);
            }

            let mut cq = cq_for(fam, o.quality, hw);
            let sp = speed_preset(o);
            match (fam, hw) {
                (Family::Av1 | Family::Hevc | Family::H264, Hw::Nvenc) => {
                    let enc = match fam {
                        Family::Av1 => "av1_nvenc",
                        Family::Hevc => "hevc_nvenc",
                        _ => "h264_nvenc",
                    };
                    let d = dev.cloned().unwrap_or_default();
                    let uhq = sp.uhq && d.uhq && fam != Family::H264;
                    if !uhq && fam != Family::H264 {
                        cq = (cq + sp.cq_offset).min(63.0);
                    }
                    a.extend([os("-c:v"), os(enc), os("-gpu"), os(&gpu_index), os("-preset"), os(sp.preset), os("-tune"), os(if uhq { "uhq" } else { "hq" })]);
                    // Quarter-resolution first pass: same quality as full-resolution, ~2× faster.
                    a.extend([os("-multipass"), os("qres"), os("-rc-lookahead"), os(sp.lookahead)]);
                    a.extend(["-spatial-aq", "1", "-temporal-aq", "1"].map(os));
                    if sp.split && !uhq && fam != Family::H264 && d.split {
                        // Both NVENC engines work on one video.
                        a.extend(["-split_encode_mode", "2"].map(os));
                    }
                    a.extend([os("-bf"), os(if fam == Family::H264 { "3" } else { "4" }), os("-b_ref_mode"), os("middle")]);
                    if fam == Family::Hevc && ten_bit {
                        a.extend(["-profile:v", "main10"].map(os));
                    }
                    if fam == Family::H264 {
                        a.extend(["-profile:v", "high"].map(os));
                    }
                    match video_kbps {
                        Some(k) => a.extend([
                            os("-rc"), os("vbr"), os("-b:v"), os(format!("{k}k")),
                            os("-maxrate"), os(format!("{}k", k * 3 / 2)), os("-bufsize"), os(format!("{}k", k * 2)),
                        ]),
                        // Without an explicit ceiling NVENC silently caps CQ mode at ~11 Mbit/s,
                        // which smears grain and fine detail on complex scenes.
                        None => a.extend([
                            os("-rc"), os("vbr"), os("-cq"), os(format!("{cq:.1}")), os("-b:v"), os("0"),
                            os("-maxrate"), os("400M"), os("-bufsize"), os("800M"),
                        ]),
                    }
                }
                (Family::Av1 | Family::Hevc | Family::H264, Hw::Amf) => {
                    // AMD Radeon (VCN): constant QP with pre-analysis and adaptive quantisation.
                    let base = match fam {
                        Family::Av1 => "av1",
                        Family::Hevc => "hevc",
                        _ => "h264",
                    };
                    a.extend([os("-c:v"), os(format!("{base}_amf")), os("-quality"), os("quality"), os("-preanalysis"), os("1")]);
                    if fam != Family::Av1 {
                        a.extend(["-vbaq", "1"].map(os));
                    }
                    if fam == Family::Hevc && ten_bit {
                        a.extend(["-profile:v", "main10"].map(os));
                    }
                    match video_kbps {
                        Some(k) => a.extend([
                            os("-rc"), os("vbr_peak"), os("-b:v"), os(format!("{k}k")), os("-maxrate"), os(format!("{}k", k * 3 / 2)),
                        ]),
                        None => {
                            // AV1 QP runs 0..255 (≈ 4 per H.26x step)
                            let (qi, qp, qb) = if fam == Family::Av1 {
                                let q = (cq * 4.0).clamp(1.0, 255.0);
                                (q - 8.0, q, q + 8.0)
                            } else {
                                (cq - 2.0, cq, cq + 2.0)
                            };
                            a.extend([os("-rc"), os("cqp"), os("-qp_i"), os(format!("{qi:.0}")), os("-qp_p"), os(format!("{qp:.0}"))]);
                            if fam != Family::Hevc {
                                a.extend([os("-qp_b"), os(format!("{qb:.0}"))]);
                            }
                        }
                    }
                }
                (Family::Av1 | Family::Hevc | Family::H264, Hw::Qsv) => {
                    // Intel Arc / Iris Xe / UHD: ICQ (intelligent constant quality) at the slowest preset.
                    let base = match fam {
                        Family::Av1 => "av1",
                        Family::Hevc => "hevc",
                        _ => "h264",
                    };
                    a.extend([os("-c:v"), os(format!("{base}_qsv")), os("-preset"), os("veryslow")]);
                    if fam != Family::Av1 {
                        a.extend(["-extbrc", "1", "-look_ahead_depth", "40"].map(os));
                    }
                    if fam == Family::Hevc && ten_bit {
                        a.extend(["-profile:v", "main10"].map(os));
                    }
                    match video_kbps {
                        Some(k) => a.extend([
                            os("-b:v"), os(format!("{k}k")), os("-maxrate"), os(format!("{}k", k * 3 / 2)), os("-bufsize"), os(format!("{}k", k * 2)),
                        ]),
                        None => a.extend([os("-global_quality"), os(format!("{:.0}", cq.clamp(1.0, 51.0)))]),
                    }
                }
                (Family::Av1 | Family::Hevc | Family::H264, Hw::Mf) => {
                    // Media Foundation: works with the hardware encoder of any GPU vendor.
                    let base = match fam {
                        Family::Av1 => "av1",
                        Family::Hevc => "hevc",
                        _ => "h264",
                    };
                    a.extend([os("-c:v"), os(format!("{base}_mf")), os("-hw_encoding"), os("1")]);
                    match video_kbps {
                        Some(k) => a.extend([os("-rate_control"), os("u_vbr"), os("-b:v"), os(format!("{k}k"))]),
                        None => {
                            // Calibrated by VMAF: quality 70 ≈ VMAF 95-96 (the 85 % default)
                            let q = o.quality.clamp(0.0, 100.0);
                            let mq = if q >= 85.0 { 70.0 + (q - 85.0) * 1.33 } else { 70.0 - (85.0 - q) * 0.7 };
                            a.extend([os("-rate_control"), os("quality"), os("-quality"), os(format!("{:.0}", mq.clamp(1.0, 100.0)))]);
                        }
                    }
                }
                (Family::Av1, Hw::Cpu) => {
                    a.extend(["-c:v", "libsvtav1", "-preset", "6", "-svtav1-params", "tune=0:enable-overlays=1:scd=1"].map(os));
                    match video_kbps {
                        Some(k) => a.extend([os("-b:v"), os(format!("{k}k"))]),
                        None => a.extend([os("-crf"), os(format!("{:.0}", cq))]),
                    }
                }
                (Family::Hevc, Hw::Cpu) => {
                    a.extend(["-c:v", "libx265", "-preset", "medium", "-x265-params", "log-level=error:aq-mode=3"].map(os));
                    match video_kbps {
                        Some(k) => a.extend([os("-b:v"), os(format!("{k}k"))]),
                        None => a.extend([os("-crf"), os(format!("{cq:.1}"))]),
                    }
                }
                (Family::H264, Hw::Cpu) => {
                    a.extend(["-c:v", "libx264", "-preset", "slow"].map(os));
                    if container == "3gp" {
                        a.extend(["-profile:v", "baseline"].map(os));
                    }
                    match video_kbps {
                        Some(k) => a.extend([os("-b:v"), os(format!("{k}k"))]),
                        None => a.extend([os("-crf"), os(format!("{cq:.1}"))]),
                    }
                }
                (Family::Vp9, _) => {
                    a.extend(["-c:v", "libvpx-vp9", "-row-mt", "1", "-deadline", "good", "-cpu-used", "2"].map(os));
                    match video_kbps {
                        Some(k) => a.extend([os("-b:v"), os(format!("{k}k"))]),
                        None => a.extend([os("-crf"), os(format!("{:.0}", cq)), os("-b:v"), os("0")]),
                    }
                }
                (Family::Mpeg4, _) => {
                    let q = 2.0 + (100.0 - o.quality) / 100.0 * 10.0;
                    a.extend([os("-c:v"), os("libxvid"), os("-q:v"), os(format!("{q:.0}")), os("-tag:v"), os("XVID")]);
                }
                (Family::Wmv2, _) => {
                    let k = video_kbps.unwrap_or_else(|| ((v.width.max(640) * v.height.max(360)) as f64 * 0.004 * (0.5 + o.quality / 100.0)) as u32);
                    a.extend([os("-c:v"), os("wmv2"), os("-b:v"), os(format!("{k}k"))]);
                }
                (Family::Mpeg2, _) => {
                    let q = 2.0 + (100.0 - o.quality) / 100.0 * 10.0;
                    a.extend([os("-c:v"), os("mpeg2video"), os("-q:v"), os(format!("{q:.0}"))]);
                }
                (Family::Theora, _) => {
                    let q = 3.0 + o.quality / 100.0 * 7.0;
                    a.extend([os("-c:v"), os("libtheora"), os("-q:v"), os(format!("{q:.0}"))]);
                }
            }
            if fam == Family::Hevc && matches!(container, "mp4" | "mov" | "m4v") {
                a.extend(["-tag:v", "hvc1"].map(os));
            }
        }
    }

    // ── Audio ───────────────────────────────────────────────────────────────
    let src_a = info.audio.first();
    let default_codec = match container {
        "webm" | "mkv" => "libopus",
        "ogv" => "libvorbis",
        "avi" => "libmp3lame",
        "wmv" => "wmav2",
        "mpeg" | "mpg" => "mp2",
        _ => "aac",
    };
    let allowed_copy: &[&str] = match container {
        "mp4" | "m4v" | "mov" => &["aac", "mp3", "ac3", "eac3", "alac", "opus"],
        "mkv" => &["aac", "mp3", "ac3", "eac3", "opus", "flac", "vorbis", "dts", "truehd", "alac"],
        "webm" => &["opus", "vorbis"],
        "flv" | "3gp" | "ts" => &["aac", "mp3"],
        "avi" => &["mp3", "ac3"],
        _ => &[],
    };
    let budget = audio_kbps_budget;
    let can_copy = src_a.is_some_and(|s| {
        allowed_copy.contains(&s.codec.as_str())
            && budget.is_none_or(|b| s.bitrate_kbps.is_some_and(|sb| sb <= b + 8))
            && s.bitrate_kbps.is_none_or(|b| b <= 320)
    });
    let choice = o.audio.as_str();
    if choice == "none" || info.audio.is_empty() {
        a.push(os("-an"));
    } else if (choice == "auto" && can_copy && fam.is_some()) || choice == "copy" || (fam.is_none() && choice == "auto") {
        a.extend(["-c:a", "copy"].map(os));
    } else {
        let codec = match choice {
            "aac" => "aac",
            "opus" => "libopus",
            "mp3" => "libmp3lame",
            _ => default_codec,
        };
        let codec = if container == "webm" && codec != "libvorbis" { "libopus" } else { codec };
        let ch = src_a.map(|s| s.channels).unwrap_or(2);
        let mut k = if o.audio_kbps > 0 {
            o.audio_kbps
        } else if let Some(b) = budget {
            b
        } else {
            match codec {
                "libopus" => 128,
                "mp2" => 224,
                _ => 160,
            }
        };
        if ch > 2 {
            k = k * 2; // multichannel needs more
        }
        a.extend([os("-c:a"), os(codec), os("-b:a"), os(format!("{k}k"))]);
        if codec == "libopus" && ch > 2 {
            a.extend(["-mapping_family", "1"].map(os));
        }
        if container == "3gp" {
            a.extend(["-ar", "44100", "-ac", "2"].map(os));
        }
    }
    if matches!(container, "mp4" | "mov" | "m4v") {
        a.extend(["-movflags", "+faststart"].map(os));
    }
    if container == "mpeg" || container == "mpg" {
        a.extend(["-f", "mpeg"].map(os));
    }
    a.push(dst.as_os_str().to_owned());
    Plan { args: a, out_duration }
}

fn ext_for_container(c: &str) -> &str {
    match c {
        "mpeg" => "mpg",
        other => other,
    }
}

pub fn run(src: &Path, opts: &VideoOpts, ctx: &Ctx) -> JobResult {
    let caps = gpu::caps();
    if !caps.ffmpeg_ok {
        return Err("Движок ffmpeg не найден рядом с программой (папка engine)".into());
    }
    ctx.stage(0.0, "Анализ файла");
    let info = ffmpeg::probe(src)?;
    if info.video.is_none() {
        return Err("В файле нет видеодорожки".into());
    }
    let container = container_for(src, opts);
    let fam = family_for(&container, opts, caps);
    let suffix = if opts.mode == "compress" { "_compressed" } else { "" };
    let dst = paths::reserve(src, ctx.out_dir.as_deref(), suffix, ext_for_container(&container))?;
    let src_size = super::file_size(src);

    let start = opts.trim_start.unwrap_or(0.0).max(0.0);
    let mut dur = (info.duration - start).max(0.0);
    if let Some(e) = opts.trim_end.filter(|e| *e > start) {
        dur = dur.min(e - start);
    }

    // Bitrate planning for "target size" / "bitrate" modes.
    let mut video_kbps: Option<u32> = opts.bitrate_kbps.filter(|k| *k > 0);
    let mut audio_budget: Option<u32> = None;
    let mut target_bytes: Option<u64> = None;
    let mut note: Option<String> = None;
    if let Some(mb) = opts.target_mb.filter(|m| *m > 0.0) {
        let tb = (mb * 1024.0 * 1024.0) as u64;
        let full_length = (dur - info.duration).abs() < 0.5;
        if tb >= src_size && full_length && opts.height == 0 {
            note = Some("Исходник уже меньше заданного размера - применён режим качества".into());
        } else if dur > 0.0 {
            let total_kbps = tb as f64 * 8.0 / 1000.0 / dur * 0.975;
            let n_audio = if opts.audio == "none" { 0 } else { info.audio.len() as u32 };
            let ak: u32 = if n_audio == 0 {
                0
            } else if total_kbps < 400.0 {
                48
            } else if total_kbps < 1200.0 {
                96
            } else {
                128
            };
            audio_budget = if ak > 0 { Some(ak) } else { None };
            let vk = (total_kbps - (ak * n_audio) as f64).max(50.0);
            video_kbps = Some(vk as u32);
            target_bytes = Some(tb);
        }
    }

    let duration_for_progress = if dur > 0.0 { dur } else { 1.0 };
    let stage_label = std::sync::Mutex::new(String::new());
    let progress = |f: f64, speed: f64| {
        let l = stage_label.lock().unwrap().clone();
        let s = if speed > 0.0 { format!("{l} · {speed:.1}×") } else { l };
        (ctx.progress)(f, Some(s));
    };

    // Pick a video device: the least busy one that can encode this codec (several GPUs share the work).
    let lease = match fam {
        Some(f) if opts.hw != "cpu" && container != "gif" => gpu::acquire(|d| supports(d, f), &crate::settings::get().gpu_device),
        _ => None,
    };
    let dev: Option<&EncDevice> = lease.as_ref().map(|l| l.device);
    // NVDEC handles 4:2:0 only (4:2:2/4:4:4 and exotic formats decode on the CPU).
    let pf = info.video.as_ref().map(|v| v.pix_fmt.as_str()).unwrap_or("");
    let nvdec_ok = pf.is_empty() || pf.starts_with("yuv420p") || pf.starts_with("yuvj420p") || pf.starts_with("nv12") || pf.starts_with("p010");
    let mf_can = fam.is_some_and(|f| gpu::mf_device().is_some_and(|m| supports(m, f))) && opts.hw != "cpu" && container != "gif";
    let mut levels: Vec<Level> = match dev.map(|d| d.kind.as_str()) {
        Some("nvenc") if nvdec_ok && dev.is_some_and(|d| d.cuda) => vec![Level::FullGpu, Level::HwDecode],
        Some("nvenc") | Some("amf") | Some("qsv") => vec![Level::HwDecode],
        _ => vec![],
    };
    if mf_can && dev.is_none_or(|d| d.kind != "mf") {
        levels.push(Level::MfEnc);
    }
    if dev.is_some_and(|d| d.kind == "mf") {
        levels.push(Level::MfEnc);
    }
    levels.push(Level::Cpu);
    let multi = gpu::job_devices().len() > 1;
    let mut attempt_kbps = video_kbps;
    let mut corrected = false;
    let mut quality_retry = false;
    let mut q_override = opts.quality;
    let mut last_err = String::new();
    let mut li = 0usize;
    while li < levels.len() {
        let level = levels[li];
        if ctx.cancelled() {
            paths::discard(&dst);
            return Err(CANCELLED.into());
        }
        let hw = hw_of(level, dev, fam);
        let short = |d: &EncDevice| d.name.replace("NVIDIA ", "").replace("GeForce ", "");
        let label = match hw {
            Hw::Cpu => "Кодирование на процессоре".to_string(),
            Hw::Mf => "Кодирование на видеокарте (Media Foundation)".to_string(),
            _ if multi => format!("Кодирование на {}", dev.map(short).unwrap_or_default()),
            _ => "Кодирование на видеокарте".to_string(),
        };
        let label = if corrected { "Уточнение размера".to_string() } else if quality_retry { "Повторное сжатие".to_string() } else { label };
        *stage_label.lock().unwrap() = label.clone();
        ctx.stage(0.0, &label);
        let mut o2 = opts.clone();
        o2.quality = q_override;
        let plan = build(src, &dst, &info, &o2, dev, &container, fam, level, attempt_kbps, audio_budget);
        let r = ffmpeg::run(&plan.args, duration_for_progress.min(plan.out_duration.max(0.1)), ctx.cancel, &progress);
        if r.cancelled {
            paths::discard(&dst);
            return Err(CANCELLED.into());
        }
        if !r.ok {
            last_err = r.error_tail;
            li += 1;
            continue;
        }
        let out_size = super::file_size(&dst);
        // Target size correction: one more pass with a corrected bitrate.
        if let (Some(tb), Some(k)) = (target_bytes, attempt_kbps) {
            if out_size > tb + tb / 33 && !corrected {
                corrected = true;
                let factor = tb as f64 / out_size as f64 * 0.97;
                attempt_kbps = Some(((k as f64) * factor).max(40.0) as u32);
                ctx.stage(0.0, "Уточнение размера");
                continue;
            }
        }
        // Quality mode produced a bigger file than the source: try once more, harder.
        if opts.mode == "compress" && target_bytes.is_none() && video_kbps.is_none() && fam.is_some() && out_size >= src_size * 97 / 100 {
            if !quality_retry && q_override > 40.0 {
                quality_retry = true;
                q_override -= 15.0;
                ctx.stage(0.0, "Исходник уже хорошо сжат - пробую сильнее");
                continue;
            }
            if out_size >= src_size {
                // Never hand back a bigger file: keep an exact copy instead.
                let _ = std::fs::copy(src, &dst);
                note = Some("Видео уже сжато оптимально - сохранена копия без потерь".into());
            }
        }
        let used = match hw {
            Hw::Cpu => "CPU".to_string(),
            Hw::Nvenc if multi => format!("NVENC {}", dev.map(short).unwrap_or_default()),
            Hw::Nvenc => "NVENC".into(),
            Hw::Qsv => "Intel QSV".into(),
            Hw::Amf => "AMD AMF".into(),
            Hw::Mf => "Media Foundation".into(),
        };
        let codec_name = match fam {
            Some(Family::Av1) => "AV1",
            Some(Family::Hevc) => "HEVC",
            Some(Family::H264) => "H.264",
            Some(Family::Vp9) => "VP9",
            Some(Family::Mpeg4) => "MPEG-4",
            Some(Family::Wmv2) => "WMV",
            Some(Family::Mpeg2) => "MPEG-2",
            Some(Family::Theora) => "Theora",
            None => "без перекодирования",
        };
        let base = if container == "gif" { "GIF".to_string() } else { format!("{codec_name} · {used}") };
        let n = match note {
            Some(n) => format!("{base} · {n}"),
            None => base,
        };
        return Ok(output(&dst, Some(n)));
    }
    paths::discard(&dst);
    Err(if last_err.is_empty() { "Не удалось перекодировать видео".into() } else { last_err })
}
