//! Video hardware detection and scheduling.
//! Every NVIDIA GPU is its own encode device (NVENC, selected with -gpu N); AMD uses AMF, Intel uses
//! Quick Sync; any other GPU with a Windows hardware encoder works through Media Foundation.
//! Video jobs are spread over all devices (least busy first).

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use super::ffmpeg::{self, CREATE_NO_WINDOW};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub vendor: String,
    pub driver: String,
    pub vram_mb: u32,
    pub compute_cap: String,
    /// Human name of the generation, e.g. "Blackwell (RTX 50)"
    pub generation: String,
    pub features: Vec<String>,
}

/// One hardware encoder the app can send video jobs to.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EncDevice {
    /// "nv0", "nv1", "amf", "qsv", "mf"
    pub id: String,
    pub name: String,
    /// "nvenc", "amf", "qsv", "mf"
    pub kind: String,
    pub vendor: String,
    /// NVENC: CUDA device index
    pub index: u32,
    pub av1: bool,
    pub hevc: bool,
    pub h264: bool,
    pub ten_bit: bool,
    pub uhq: bool,
    pub split: bool,
    pub cuda: bool,
}

/// Aggregate view (any device can do it), kept for the interface.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Encoders {
    pub av1_nvenc: bool,
    pub hevc_nvenc: bool,
    pub h264_nvenc: bool,
    pub nvenc_10bit: bool,
    pub nvenc_uhq: bool,
    #[serde(default)]
    pub nvenc_split: bool,
    pub cuda_decode: bool,
    pub av1_qsv: bool,
    pub hevc_qsv: bool,
    pub h264_qsv: bool,
    pub av1_amf: bool,
    pub hevc_amf: bool,
    pub h264_amf: bool,
    #[serde(default)]
    pub av1_mf: bool,
    #[serde(default)]
    pub hevc_mf: bool,
    #[serde(default)]
    pub h264_mf: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    pub gpus: Vec<GpuInfo>,
    pub devices: Vec<EncDevice>,
    pub encoders: Encoders,
    pub ffmpeg_ok: bool,
    pub cpu_threads: usize,
    /// Codec id recommended for compression ("av1", "hevc", "h264")
    pub recommended_codec: String,
    pub recommended_reason: String,
}

fn generation_for(cc: &str, name: &str) -> (String, Vec<String>) {
    let major: u32 = cc.split('.').next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let minor: u32 = cc.split('.').nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
    let n = name.to_lowercase();
    let mut f = vec![];
    let gen = if major >= 12 || n.contains("rtx 50") || n.contains("rtx pro") {
        f.extend(["NVENC AV1 (UHQ)", "NVENC HEVC 10-бит / 4:2:2", "NVENC H.264", "NVDEC AV1/HEVC/H.264/VP9", "Несколько NVENC-блоков"]);
        "Blackwell (RTX 50)"
    } else if major == 8 && minor >= 9 || n.contains("rtx 40") {
        f.extend(["NVENC AV1", "NVENC HEVC 10-бит", "NVENC H.264", "NVDEC AV1/HEVC/H.264/VP9"]);
        "Ada Lovelace (RTX 40)"
    } else if major == 8 || n.contains("rtx 30") {
        f.extend(["NVENC HEVC 10-бит", "NVENC H.264", "NVDEC AV1/HEVC/H.264/VP9"]);
        "Ampere (RTX 30)"
    } else if major == 7 && minor >= 5 || n.contains("rtx 20") || n.contains("gtx 16") {
        f.extend(["NVENC HEVC 10-бит", "NVENC H.264", "NVDEC HEVC/H.264/VP9"]);
        "Turing (RTX 20 / GTX 16)"
    } else if major > 0 {
        f.extend(["NVENC H.264", "NVENC HEVC"]);
        "NVIDIA (старшее поколение)"
    } else {
        ""
    };
    (gen.to_string(), f.into_iter().map(String::from).collect())
}

fn nvidia_gpus() -> Vec<GpuInfo> {
    let out = Command::new("nvidia-smi")
        .args(["--query-gpu=name,compute_cap,driver_version,memory.total", "--format=csv,noheader,nounits"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let Ok(out) = out else { return vec![] };
    if !out.status.success() {
        return vec![];
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split(',').map(|s| s.trim()).collect();
            if p.len() < 4 {
                return None;
            }
            let (generation, features) = generation_for(p[1], p[0]);
            Some(GpuInfo {
                name: p[0].to_string(),
                vendor: "NVIDIA".into(),
                compute_cap: p[1].to_string(),
                driver: p[2].to_string(),
                vram_mb: p[3].parse().unwrap_or(0),
                generation,
                features,
            })
        })
        .collect()
}

/// All display adapters from WMI (AMD, Intel, Qualcomm, Moore Threads, …), minus virtual ones.
fn other_gpus() -> Vec<GpuInfo> {
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_VideoController | ForEach-Object { \"$($_.Name)|$($_.AdapterCompatibility)|$($_.DriverVersion)|$($_.AdapterRAM)\" }",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let Ok(out) = out else { return vec![] };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.trim().split('|').collect();
            let name = p.first()?.trim().to_string();
            let maker = p.get(1).map(|s| s.trim().to_string()).unwrap_or_default();
            let low = format!("{} {}", name, maker).to_lowercase();
            if name.is_empty() || low.contains("nvidia") {
                return None;
            }
            for skip in ["virtual", "basic display", "remote", "meta ", "parsec", "citrix", "vmware", "hyper-v", "idd", "spacedesk", "displaylink"] {
                if low.contains(skip) {
                    return None;
                }
            }
            let vendor = if low.contains("intel") {
                "Intel"
            } else if low.contains("amd") || low.contains("radeon") || low.contains("advanced micro") {
                "AMD"
            } else if low.contains("qualcomm") || low.contains("adreno") {
                "Qualcomm"
            } else {
                maker.as_str()
            };
            let ram: u64 = p.get(3).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            Some(GpuInfo {
                name,
                vendor: vendor.to_string(),
                driver: p.get(2).map(|s| s.trim().to_string()).unwrap_or_default(),
                vram_mb: (ram / 1024 / 1024) as u32,
                ..Default::default()
            })
        })
        .collect()
}

fn test_enc(codec: &str, extra: &[&str]) -> bool {
    test_enc_size(codec, "640x360", extra)
}

fn test_enc_size(codec: &str, size: &str, extra: &[&str]) -> bool {
    let src = format!("color=c=gray:s={size}:r=30");
    let mut args = vec!["-f", "lavfi", "-i", src.as_str(), "-frames:v", "8", "-c:v", codec];
    args.extend_from_slice(extra);
    args.extend_from_slice(&["-f", "null", "-"]);
    ffmpeg::quick_ok(&args)
}

/// NVENC device list in CUDA order (the order -gpu N uses): "GPU #0 - < name > has Compute SM 12.0".
fn nvenc_list() -> Vec<(u32, String)> {
    let out = ffmpeg::command()
        .args(["-hide_banner", "-f", "lavfi", "-i", "nullsrc=s=256x256", "-frames:v", "1", "-c:v", "h264_nvenc", "-gpu", "list", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    let Ok(out) = out else { return vec![] };
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    let re = regex::Regex::new(r"GPU #(\d+) - < (.+?) >").unwrap();
    re.captures_iter(&text).filter_map(|c| Some((c[1].parse().ok()?, c[2].to_string()))).collect()
}

fn detect_nvidia(has_nv: bool) -> Vec<EncDevice> {
    if !has_nv {
        return vec![];
    }
    let mut list = nvenc_list();
    if list.is_empty() {
        list.push((0, "NVIDIA".into()));
    }
    std::thread::scope(|s| {
        let handles: Vec<_> = list
            .iter()
            .map(|(i, name)| {
                s.spawn(move || {
                    let g = i.to_string();
                    let gpu = ["-gpu", g.as_str()];
                    let mut d = EncDevice { id: format!("nv{i}"), name: name.clone(), kind: "nvenc".into(), vendor: "NVIDIA".into(), index: *i, ..Default::default() };
                    d.h264 = test_enc("h264_nvenc", &gpu);
                    d.hevc = test_enc("hevc_nvenc", &gpu);
                    d.av1 = test_enc("av1_nvenc", &gpu);
                    d.ten_bit = d.hevc && test_enc("hevc_nvenc", &["-gpu", g.as_str(), "-pix_fmt", "p010le", "-profile:v", "main10"]);
                    d.uhq = d.hevc && test_enc("hevc_nvenc", &["-gpu", g.as_str(), "-tune", "uhq", "-preset", "p7"]);
                    d.split = d.hevc && test_enc_size("hevc_nvenc", "1920x1080", &["-gpu", g.as_str(), "-preset", "p7", "-split_encode_mode", "2"]);
                    let dev = format!("cuda=cu:{i}");
                    d.cuda = ffmpeg::quick_ok(&[
                        "-init_hw_device", dev.as_str(), "-f", "lavfi", "-i", "color=c=gray:s=320x240", "-frames:v", "2",
                        "-vf", "hwupload_cuda,scale_cuda=160:120", "-f", "null", "-",
                    ]);
                    d
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).filter(|d| d.h264 || d.hevc).collect()
    })
}

fn detect() -> Capabilities {
    let ffmpeg_ok = ffmpeg::ffmpeg_available();
    let mut gpus = nvidia_gpus();
    let has_nv = !gpus.is_empty();
    gpus.extend(other_gpus());
    let has_intel = gpus.iter().any(|g| g.vendor == "Intel");
    let has_amd = gpus.iter().any(|g| g.vendor == "AMD");
    let mut devices: Vec<EncDevice> = vec![];

    if ffmpeg_ok {
        let (nv, amf, qsv, mf) = std::thread::scope(|s| {
            let nv = s.spawn(|| detect_nvidia(has_nv));
            let amf = s.spawn(|| {
                if !has_amd {
                    return None;
                }
                let name = gpus.iter().find(|g| g.vendor == "AMD").map(|g| g.name.clone()).unwrap_or_else(|| "AMD".into());
                let mut d = EncDevice { id: "amf".into(), name, kind: "amf".into(), vendor: "AMD".into(), ..Default::default() };
                d.h264 = test_enc("h264_amf", &[]);
                d.hevc = test_enc("hevc_amf", &[]);
                d.av1 = test_enc("av1_amf", &[]);
                d.ten_bit = d.hevc && test_enc("hevc_amf", &["-pix_fmt", "p010le", "-profile:v", "main10"]);
                (d.h264 || d.hevc || d.av1).then_some(d)
            });
            let qsv = s.spawn(|| {
                if !has_intel {
                    return None;
                }
                let name = gpus.iter().find(|g| g.vendor == "Intel").map(|g| g.name.clone()).unwrap_or_else(|| "Intel".into());
                let mut d = EncDevice { id: "qsv".into(), name, kind: "qsv".into(), vendor: "Intel".into(), ..Default::default() };
                d.h264 = test_enc("h264_qsv", &["-pix_fmt", "nv12"]);
                d.hevc = test_enc("hevc_qsv", &["-pix_fmt", "nv12"]);
                d.av1 = test_enc("av1_qsv", &["-pix_fmt", "nv12"]);
                d.ten_bit = d.hevc && test_enc("hevc_qsv", &["-pix_fmt", "p010le", "-profile:v", "main10"]);
                (d.h264 || d.hevc || d.av1).then_some(d)
            });
            // Media Foundation: the Windows hardware encoder of whatever GPU is installed.
            let mf = s.spawn(|| {
                let o = ["-pix_fmt", "nv12", "-hw_encoding", "1"];
                let mut d = EncDevice { id: "mf".into(), name: "Media Foundation".into(), kind: "mf".into(), vendor: "Windows".into(), ..Default::default() };
                d.h264 = test_enc("h264_mf", &o);
                d.hevc = test_enc("hevc_mf", &o);
                d.av1 = test_enc("av1_mf", &o);
                (d.h264 || d.hevc || d.av1).then_some(d)
            });
            (nv.join().unwrap_or_default(), amf.join().ok().flatten(), qsv.join().ok().flatten(), mf.join().ok().flatten())
        });
        devices.extend(nv);
        devices.extend(amf);
        devices.extend(qsv);
        // Media Foundation is used on its own only when no vendor encoder works
        // (other vendors, old drivers); otherwise it stays a fallback step.
        if let Some(mut m) = mf {
            let other: Vec<&GpuInfo> = gpus.iter().filter(|g| g.vendor != "NVIDIA").collect();
            if let Some(g) = other.first() {
                if devices.is_empty() {
                    m.name = format!("{} (Media Foundation)", g.name);
                }
            }
            devices.push(m);
        }
    }

    let vendor_devs: Vec<&EncDevice> = devices.iter().filter(|d| d.kind != "mf").collect();
    let pool: Vec<&EncDevice> = if vendor_devs.is_empty() { devices.iter().collect() } else { vendor_devs };
    let mut e = Encoders::default();
    for d in &devices {
        match d.kind.as_str() {
            "nvenc" => {
                e.av1_nvenc |= d.av1;
                e.hevc_nvenc |= d.hevc;
                e.h264_nvenc |= d.h264;
                e.nvenc_10bit |= d.ten_bit;
                e.nvenc_uhq |= d.uhq;
                e.nvenc_split |= d.split;
                e.cuda_decode |= d.cuda;
            }
            "amf" => {
                e.av1_amf |= d.av1;
                e.hevc_amf |= d.hevc;
                e.h264_amf |= d.h264;
            }
            "qsv" => {
                e.av1_qsv |= d.av1;
                e.hevc_qsv |= d.hevc;
                e.h264_qsv |= d.h264;
            }
            _ => {
                e.av1_mf |= d.av1;
                e.hevc_mf |= d.hevc;
                e.h264_mf |= d.h264;
            }
        }
    }
    let names = |f: &dyn Fn(&EncDevice) -> bool| pool.iter().filter(|d| f(d)).map(|d| d.name.replace("NVIDIA GeForce ", "")).collect::<Vec<_>>().join(", ");
    let (rc, reason) = if pool.iter().any(|d| d.av1) {
        ("av1", format!("AV1 на видеокарте ({}): на 30-50 % меньше H.264 и примерно на 20 % меньше HEVC при том же качестве", names(&|d| d.av1)))
    } else if pool.iter().any(|d| d.hevc) {
        ("hevc", format!("HEVC (H.265) на видеокарте ({}): вдвое меньше H.264 при том же качестве. AV1-кодирование эта видеокарта не поддерживает", names(&|d| d.hevc)))
    } else if pool.iter().any(|d| d.h264) {
        ("h264", format!("H.264 на видеокарте ({}): быстро; для максимального сжатия выберите AV1 на процессоре", names(&|d| d.h264)))
    } else {
        ("av1", "Видеокарта с аппаратным кодированием не найдена: AV1 (SVT-AV1) на процессоре - медленнее, но с лучшим сжатием".to_string())
    };

    Capabilities {
        gpus,
        devices,
        encoders: e,
        ffmpeg_ok,
        cpu_threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
        recommended_codec: rc.into(),
        recommended_reason: reason,
    }
}

static CAPS: OnceLock<Capabilities> = OnceLock::new();

pub fn caps() -> &'static Capabilities {
    CAPS.get_or_init(detect)
}

/// Devices that take video jobs: vendor encoders, or Media Foundation when there are none.
pub fn job_devices() -> Vec<&'static EncDevice> {
    let all = &caps().devices;
    let vendor: Vec<&EncDevice> = all.iter().filter(|d| d.kind != "mf").collect();
    if vendor.is_empty() {
        all.iter().collect()
    } else {
        vendor
    }
}

pub fn mf_device() -> Option<&'static EncDevice> {
    caps().devices.iter().find(|d| d.kind == "mf")
}

// ── Scheduling ──────────────────────────────────────────────────────────

static ACTIVE: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

/// A video job holding one device; released when dropped.
pub struct Lease {
    pub device: &'static EncDevice,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut a = ACTIVE.lock().unwrap();
        if let Some(m) = a.as_mut() {
            if let Some(n) = m.get_mut(&self.device.id) {
                *n = n.saturating_sub(1);
            }
        }
    }
}

/// Picks the least busy device that supports the codec (`preferred` = a device id from settings, or "auto").
pub fn acquire(supports: impl Fn(&EncDevice) -> bool, preferred: &str) -> Option<Lease> {
    let devs = job_devices();
    let cands: Vec<&'static EncDevice> = if preferred != "auto" && !preferred.is_empty() {
        devs.iter().copied().filter(|d| d.id == preferred && supports(d)).collect()
    } else {
        devs.iter().copied().filter(|d| supports(d)).collect()
    };
    let mut a = ACTIVE.lock().unwrap();
    let m = a.get_or_insert_with(HashMap::new);
    let pick = cands.into_iter().min_by_key(|d| (*m.get(&d.id).unwrap_or(&0), d.index))?;
    *m.entry(pick.id.clone()).or_insert(0) += 1;
    Some(Lease { device: pick })
}
