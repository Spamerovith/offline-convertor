//! Image compression and conversion: mozjpeg, imagequant + oxipng, WebP/AVIF/JXL via ffmpeg.

use std::io::Cursor;
use std::path::Path;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbImage, RgbaImage};
use serde::Deserialize;

use super::ffmpeg;
use super::paths;
use super::{output, Ctx, JobResult, CANCELLED};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ImageOpts {
    /// "compress" or "convert"
    pub mode: String,
    /// "same", "jpg", "png", "webp", "avif", "jxl", "gif", "bmp", "tiff", "ico", "tga"
    pub format: String,
    pub quality: f64,
    /// Longest side limit in pixels, 0 = keep
    pub max_side: u32,
    /// Scale in percent, 0 or 100 = keep
    pub scale: u32,
    pub keep_metadata: bool,
    /// Background for transparent images saved as JPEG/BMP
    pub background: String,
    /// Gentle denoise + contrast adaptive sharpening before encoding
    pub enhance: bool,
}

impl Default for ImageOpts {
    fn default() -> Self {
        Self {
            mode: "compress".into(),
            format: "same".into(),
            quality: 85.0,
            max_side: 0,
            scale: 0,
            keep_metadata: true,
            background: "#ffffff".into(),
            enhance: false,
        }
    }
}

/// «Улучшение» for photos: light spatial denoise, then contrast adaptive sharpening (AMD CAS in ffmpeg).
/// Both are mild on purpose: no halos, no oversharpened look.
pub const IMAGE_ENHANCE: &str = "hqdn3d=1.2:1.0:0:0,cas=0.35";

// ── Perceptual JPEG quality search (SSIMULACRA2) ────────────────────────────────

/// Maps the 0..100 slider to a SSIMULACRA2 target. 85 (default) → 81: no visible difference at normal
/// viewing distance; 95 → 90: visually lossless even at 1:1.
fn ssim2_target(q: f64) -> f64 {
    if q >= 85.0 { 81.0 + (q - 85.0) * 0.9 } else { 81.0 - (85.0 - q) * 0.6 }
}

fn to_ssim_rgb(img: &RgbImage) -> ssimulacra2::Rgb {
    let d: Vec<[f32; 3]> = img.pixels().map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]).collect();
    ssimulacra2::Rgb::new(d, img.width() as usize, img.height() as usize, ssimulacra2::TransferCharacteristic::SRGB, ssimulacra2::ColorPrimaries::BT709)
        .expect("valid size")
}

/// Four 512 px tiles (aligned to JPEG blocks) from different parts of the picture:
/// the score of this mosaic follows the full-image score within 1-2 points at 1/7 of the time.
fn sample_mosaic(img: &RgbImage) -> RgbImage {
    let (w, h) = img.dimensions();
    let t = 512u32;
    if w <= t * 2 || h <= t * 2 {
        return img.clone();
    }
    let mut out = RgbImage::new(t * 2, t * 2);
    for (k, (fx, fy)) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)].iter().enumerate() {
        let x = ((((w as f64 * fx) as u32).saturating_sub(t / 2)).min(w - t) / 16) * 16;
        let y = ((((h as f64 * fy) as u32).saturating_sub(t / 2)).min(h - t) / 16) * 16;
        let tile = image::imageops::crop_imm(img, x, y, t, t).to_image();
        image::imageops::replace(&mut out, &tile, ((k as u32 % 2) * t) as i64, ((k as u32 / 2) * t) as i64);
    }
    out
}

/// Smallest JPEG (quality and chroma mode) whose SSIMULACRA2 score reaches the target.
/// Returns (quality, full-resolution chroma), or None when the target can't be reached within `max_q`
/// (then the caller keeps the plain slider quality).
fn search_jpeg(img: &DynamicImage, target: f64, max_q: f32, base_q: f32, bg: [u8; 3]) -> Option<(f32, bool)> {
    let rgb = flatten(img, bg);
    let m = sample_mosaic(&rgb);
    let reference = to_ssim_rgb(&m);
    let mimg = DynamicImage::ImageRgb8(m.clone());
    let score = |q: f32, full: bool| -> Option<(f64, usize)> {
        let data = encode_jpeg_ex(&mimg, q, None, None, bg, Some(full)).ok()?;
        let dec = image::load_from_memory_with_format(&data, ImageFormat::Jpeg).ok()?.to_rgb8();
        let s = ssimulacra2::compute_frame_ssimulacra2(reference.clone(), to_ssim_rgb(&dec)).ok()?;
        Some((s, data.len()))
    };
    // Never bigger than the plain encode at the slider quality.
    let base_len = encode_jpeg_ex(&mimg, base_q, None, None, bg, None).ok()?.len();
    let mut best: Option<(f32, bool, usize)> = None;
    for full in [false, true] {
        // Binary search on quality 30..max_q (score grows with quality).
        let (mut lo, mut hi) = (30.0f32, max_q);
        let mut found: Option<(f32, usize)> = None;
        if let Some((s, n)) = score(hi, full) {
            if s < target {
                // This chroma mode can't reach the target within the limit
            } else {
                found = Some((hi, n));
                while hi - lo > 2.0 {
                    let mid = ((lo + hi) / 2.0).round();
                    match score(mid, full) {
                        Some((s, n)) if s >= target => {
                            hi = mid;
                            found = Some((mid, n));
                        }
                        Some(_) => lo = mid,
                        None => break,
                    }
                }
            }
        }
        if let Some((q, n)) = found {
            if n <= base_len && best.is_none_or(|b| n < b.2) {
                best = Some((q, full, n));
            }
        }
    }
    best.map(|(q, f, _)| (q, f))
}

pub struct Loaded {
    pub img: DynamicImage,
    pub icc: Option<Vec<u8>>,
    pub exif: Option<Vec<u8>>,
    pub orientation: Option<image::metadata::Orientation>,
    #[allow(dead_code)]
    pub format: Option<ImageFormat>,
}

/// Decodes with the native decoders, falling back to ffmpeg (HEIC, AVIF, JXL, PSD, …).
pub fn load(path: &Path) -> Result<Loaded, String> {
    let native = (|| -> Result<Loaded, String> {
        let reader = ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
        let format = reader.format();
        if matches!(format, Some(ImageFormat::Avif) | None) {
            return Err("fallback".into());
        }
        let mut reader = reader;
        reader.no_limits();
        let mut dec = reader.into_decoder().map_err(|e| e.to_string())?;
        let icc = dec.icc_profile().ok().flatten();
        let exif = dec.exif_metadata().ok().flatten();
        let orientation = dec.orientation().ok();
        let img = DynamicImage::from_decoder(dec).map_err(|e| e.to_string())?;
        Ok(Loaded { img, icc, exif, orientation, format })
    })();
    match native {
        Ok(l) => Ok(l),
        Err(_) => {
            let rgba = ffmpeg::decode_image_rgba(path)?;
            Ok(Loaded { img: DynamicImage::ImageRgba8(rgba), icc: None, exif: None, orientation: None, format: None })
        }
    }
}

fn parse_hex(c: &str) -> [u8; 3] {
    let c = c.trim_start_matches('#');
    if c.len() == 6 {
        let v = u32::from_str_radix(c, 16).unwrap_or(0xffffff);
        [(v >> 16) as u8, (v >> 8) as u8, v as u8]
    } else {
        [255, 255, 255]
    }
}

fn flatten(img: &DynamicImage, bg: [u8; 3]) -> RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut out = RgbImage::new(w, h);
    for (o, p) in out.pixels_mut().zip(rgba.pixels()) {
        let a = p[3] as u32;
        for i in 0..3 {
            o[i] = ((p[i] as u32 * a + bg[i] as u32 * (255 - a) + 127) / 255) as u8;
        }
    }
    out
}

fn has_real_alpha(img: &DynamicImage) -> bool {
    if !img.color().has_alpha() {
        return false;
    }
    img.to_rgba8().pixels().any(|p| p[3] != 255)
}

/// Estimates libjpeg quality of an existing JPEG from its luminance quantisation table.
pub fn estimate_jpeg_quality(data: &[u8]) -> Option<f64> {
    const STD: [u32; 64] = [
        16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77,
        24, 35, 55, 64, 81, 104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
    ];
    let mut i = 2;
    while i + 4 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = data[i + 1];
        let len = ((data[i + 2] as usize) << 8) | data[i + 3] as usize;
        if m == 0xDB {
            let seg = data.get(i + 4..i + 2 + len)?;
            let pq = seg[0] >> 4;
            let tq = seg[0] & 0x0f;
            if tq == 0 {
                let mut sum = 0u32;
                for k in 0..64 {
                    let v = if pq == 0 { *seg.get(1 + k)? as u32 } else { ((*seg.get(1 + 2 * k)? as u32) << 8) | *seg.get(2 + 2 * k)? as u32 };
                    sum += v;
                }
                // The table is stored in zig-zag order; a sum-based scale estimate is order-independent.
                let std_sum: u32 = STD.iter().sum();
                let scale = sum as f64 * 100.0 / std_sum as f64;
                let q = if scale <= 100.0 { (200.0 - scale) / 2.0 } else { 5000.0 / scale };
                return Some(q.clamp(1.0, 100.0));
            }
        }
        if m == 0xDA {
            break;
        }
        i += 2 + len;
    }
    None
}

/// `full_chroma`: None = 4:4:4 from quality 80 up, Some(x) = forced
fn encode_jpeg_ex(img: &DynamicImage, q: f32, icc: Option<&[u8]>, exif: Option<&[u8]>, bg: [u8; 3], full_chroma: Option<bool>) -> Result<Vec<u8>, String> {
    let gray = matches!(img.color(), image::ColorType::L8 | image::ColorType::L16);
    let (w, h) = (img.width() as usize, img.height() as usize);
    let (cs, buf): (mozjpeg::ColorSpace, Vec<u8>) =
        if gray { (mozjpeg::ColorSpace::JCS_GRAYSCALE, img.to_luma8().into_raw()) } else { (mozjpeg::ColorSpace::JCS_RGB, flatten(img, bg).into_raw()) };
    let icc = icc.map(|v| v.to_vec());
    let exif = exif.map(|e| {
        if e.starts_with(b"Exif\0\0") {
            e.to_vec()
        } else {
            let mut v = b"Exif\0\0".to_vec();
            v.extend_from_slice(e);
            v
        }
    });
    std::panic::catch_unwind(move || -> Result<Vec<u8>, String> {
        let mut c = mozjpeg::Compress::new(cs);
        c.set_size(w, h);
        c.set_quality(q);
        c.set_progressive_mode();
        c.set_optimize_scans(true);
        c.set_optimize_coding(true);
        if !gray {
            // Full-resolution colour at high quality: no colour bleeding on saturated edges.
            if full_chroma.unwrap_or(q >= 80.0) {
                c.set_chroma_sampling_pixel_sizes((1, 1), (1, 1));
            } else {
                c.set_chroma_sampling_pixel_sizes((2, 2), (2, 2));
            }
        }
        let mut s = c.start_compress(Vec::with_capacity(w * h / 4)).map_err(|e| e.to_string())?;
        if let Some(e) = &exif {
            if e.len() < 65000 {
                s.write_marker(mozjpeg::Marker::APP(1), e);
            }
        }
        if let Some(p) = &icc {
            s.write_icc_profile(p);
        }
        s.write_scanlines(&buf).map_err(|e| e.to_string())?;
        s.finish().map_err(|e| e.to_string())
    })
    .map_err(|_| "Сбой JPEG-кодировщика".to_string())?
}

fn encode_png_indexed(w: u32, h: u32, palette: &[imagequant::RGBA], idx: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let pal: Vec<u8> = palette.iter().flat_map(|c| [c.r, c.g, c.b]).collect();
        enc.set_palette(pal);
        if palette.iter().any(|c| c.a != 255) {
            let trns: Vec<u8> = palette.iter().map(|c| c.a).collect();
            enc.set_trns(trns);
        }
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(idx).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

fn oxi(data: &[u8], strip: bool) -> Vec<u8> {
    let mut o = oxipng::Options::from_preset(3);
    if strip {
        o.strip = oxipng::StripChunks::Safe;
    }
    oxipng::optimize_from_memory(data, &o).unwrap_or_else(|_| data.to_vec())
}

/// Lossy PNG: perceptual palette quantisation with dithering, then lossless optimisation.
fn png_lossy(rgba: &RgbaImage, q: f64, strip: bool) -> Option<Vec<u8>> {
    let (w, h) = rgba.dimensions();
    let target = (55.0 + q * 0.45).clamp(1.0, 100.0) as u8;
    // If the palette can't reach this, the image stays lossless instead of getting visible dithering.
    let min = target.saturating_sub(if q >= 80.0 { 15 } else { 30 });
    let mut liq = imagequant::new();
    liq.set_speed(3).ok()?;
    liq.set_quality(min, target).ok()?;
    let px: Vec<imagequant::RGBA> = rgba.pixels().map(|p| imagequant::RGBA::new(p[0], p[1], p[2], p[3])).collect();
    let mut img = liq.new_image(px, w as usize, h as usize, 0.0).ok()?;
    let mut res = liq.quantize(&mut img).ok()?;
    res.set_dithering_level(1.0).ok()?;
    let (pal, idx) = res.remapped(&mut img).ok()?;
    let raw = encode_png_indexed(w, h, &pal, &idx).ok()?;
    Some(oxi(&raw, strip))
}

fn encode_png_lossless(img: &DynamicImage, strip: bool) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(oxi(&buf, strip))
}

fn encode_via_ffmpeg(img: &DynamicImage, fmt: &str, q: f64) -> Result<Vec<u8>, String> {
    let alpha = has_real_alpha(img);
    let rgba = img.to_rgba8();
    match fmt {
        "webp" => {
            let mut args: Vec<String> = vec!["-c:v".into(), "libwebp".into()];
            if q >= 100.0 {
                args.extend(["-lossless", "1", "-compression_level", "6"].map(String::from));
            } else {
                let wq = (40.0 + q * 0.55).clamp(1.0, 100.0);
                args.extend(["-quality".into(), format!("{wq:.0}"), "-compression_level".into(), "6".into(), "-preset".into(), "picture".into()]);
            }
            args.extend(["-pix_fmt".into(), if alpha { "yuva420p".into() } else { "yuv420p".into() }]);
            ffmpeg::encode_rgba(&rgba, &args, "webp")
        }
        "avif" => {
            let crf = (18.0 + (100.0 - q) * 0.5).clamp(0.0, 63.0);
            let args: Vec<String> = vec![
                "-c:v".into(), "libaom-av1".into(), "-still-picture".into(), "1".into(), "-crf".into(), format!("{crf:.0}"),
                "-cpu-used".into(), "4".into(), "-row-mt".into(), "1".into(), "-pix_fmt".into(),
                if q >= 95.0 { "yuv444p".into() } else { "yuv420p".into() }, "-frames:v".into(), "1".into(),
            ];
            if alpha {
                return Err("AVIF с прозрачностью не поддерживается, выберите WebP или PNG".into());
            }
            ffmpeg::encode_rgba(&rgba, &args, "avif")
        }
        "jxl" => {
            let d = ((100.0 - q) / 100.0 * 6.0).max(0.0);
            let args: Vec<String> = vec![
                "-c:v".into(), "libjxl".into(), "-distance".into(), format!("{d:.2}"), "-effort".into(), "7".into(), "-frames:v".into(), "1".into(),
                "-pix_fmt".into(), if alpha { "rgba".into() } else { "rgb24".into() },
            ];
            ffmpeg::encode_rgba(&rgba, &args, "image2pipe")
        }
        _ => Err("Неизвестный формат".into()),
    }
}

fn encode_simple(img: &DynamicImage, fmt: ImageFormat, bg: [u8; 3]) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let img = match fmt {
        ImageFormat::Bmp | ImageFormat::Gif if !has_real_alpha(img) => DynamicImage::ImageRgb8(img.to_rgb8()),
        ImageFormat::Bmp => DynamicImage::ImageRgb8(flatten(img, bg)),
        ImageFormat::Ico => {
            let i = if img.width() > 256 || img.height() > 256 { img.resize(256, 256, image::imageops::FilterType::Lanczos3) } else { img.clone() };
            DynamicImage::ImageRgba8(i.to_rgba8())
        }
        ImageFormat::Gif => DynamicImage::ImageRgba8(img.to_rgba8()),
        _ => img.clone(),
    };
    img.write_to(&mut Cursor::new(&mut buf), fmt).map_err(|e| e.to_string())?;
    Ok(buf)
}

fn fmt_from_ext(e: &str) -> &str {
    match e {
        "jpg" | "jpeg" | "jpe" | "jfif" => "jpg",
        "png" => "png",
        "webp" => "webp",
        "avif" => "avif",
        "jxl" => "jxl",
        "gif" => "gif",
        "bmp" => "bmp",
        "tif" | "tiff" => "tiff",
        "ico" => "ico",
        "tga" => "tga",
        _ => "",
    }
}

pub fn run(src: &Path, o: &ImageOpts, ctx: &Ctx) -> JobResult {
    ctx.stage(0.05, "Чтение");
    let src_bytes = std::fs::read(src).map_err(|e| format!("Не удалось прочитать файл: {e}"))?;
    let src_size = src_bytes.len() as u64;
    let src_ext = paths::ext_of(src);
    let src_fmt = fmt_from_ext(&src_ext).to_string();
    let mut fmt = o.format.to_lowercase();
    if fmt == "same" || fmt.is_empty() {
        fmt = if src_fmt.is_empty() || (o.mode == "compress" && matches!(src_fmt.as_str(), "bmp" | "tiff" | "tga")) {
            // HEIC, PSD, BMP, TIFF… → a compressible format
            "jpg".into()
        } else {
            src_fmt.clone()
        };
    }
    let compress = o.mode == "compress";
    let q = o.quality.clamp(1.0, 100.0);
    let strip = !o.keep_metadata;
    let bg = parse_hex(&o.background);

    let loaded = load(src)?;
    if ctx.cancelled() {
        return Err(CANCELLED.into());
    }
    ctx.stage(0.3, "Обработка");
    let mut img = loaded.img;
    let keep_exif_raw = fmt == "jpg" && !strip && loaded.exif.is_some();
    if !keep_exif_raw {
        if let Some(or) = loaded.orientation {
            img.apply_orientation(or);
        }
    }
    let resized = {
        let (w, h) = (img.width(), img.height());
        let mut nw = w;
        let mut nh = h;
        if o.scale > 0 && o.scale < 100 {
            nw = (w as u64 * o.scale as u64 / 100).max(1) as u32;
            nh = (h as u64 * o.scale as u64 / 100).max(1) as u32;
        }
        if o.max_side > 0 && nw.max(nh) > o.max_side {
            let k = o.max_side as f64 / nw.max(nh) as f64;
            nw = ((nw as f64 * k).round() as u32).max(1);
            nh = ((nh as f64 * k).round() as u32).max(1);
        }
        if (nw, nh) != (w, h) {
            img = img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3);
            true
        } else {
            false
        }
    };

    let mut notes: Vec<String> = vec![];
    if o.enhance {
        ctx.stage(0.4, "Улучшение");
        let rgba = img.to_rgba8();
        match ffmpeg::filter_rgba(&rgba, IMAGE_ENHANCE) {
            Ok(f) => {
                img = if has_real_alpha(&img) { DynamicImage::ImageRgba8(f) } else { DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(f).to_rgb8()) };
                notes.push("улучшено".into());
            }
            Err(_) => notes.push("улучшение не применено".into()),
        }
    }
    let modified = resized || o.enhance;

    ctx.stage(0.5, "Кодирование");
    let mut note: Option<String> = None;
    let data: Vec<u8> = match fmt.as_str() {
        "jpg" => {
            let mut jq = q as f32;
            let mut full_chroma: Option<bool> = None;
            let mut max_q = 95.0f32;
            if compress && src_fmt == "jpg" && !modified {
                // Never re-encode above the source quality: that only adds bytes.
                if let Some(sq) = estimate_jpeg_quality(&src_bytes) {
                    jq = jq.min((sq as f32 - 1.0).max(40.0));
                    max_q = max_q.min((sq as f32 - 1.0).max(40.0));
                }
            }
            if q < 100.0 && img.width() >= 16 && img.height() >= 16 {
                // Every photo gets exactly as much quality as it needs to look like the source.
                ctx.stage(0.55, "Подбор качества");
                // Upper bound: a little above the slider value, so a file never grows over the fixed-quality result.
                let hi = max_q.min(q as f32 + 3.0).max(30.0);
                if let Some((sq, full)) = search_jpeg(&img, ssim2_target(q), hi, jq, bg) {
                    jq = sq;
                    full_chroma = Some(full);
                }
            }
            let icc = if strip { None } else { loaded.icc.as_deref() };
            let exif = if keep_exif_raw { loaded.exif.as_deref() } else { None };
            note = Some(format!("JPEG · качество {}", jq.round()));
            encode_jpeg_ex(&img, jq, icc, exif, bg, full_chroma)?
        }
        "png" => {
            if q >= 100.0 {
                note = Some("PNG без потерь".into());
                if src_fmt == "png" && !modified {
                    oxi(&src_bytes, strip)
                } else {
                    encode_png_lossless(&img, strip)?
                }
            } else {
                let rgba = img.to_rgba8();
                match png_lossy(&rgba, q, strip) {
                    Some(d) => {
                        note = Some("PNG · палитра".into());
                        d
                    }
                    None => {
                        note = Some("PNG без потерь".into());
                        if src_fmt == "png" && !modified {
                            oxi(&src_bytes, strip)
                        } else {
                            encode_png_lossless(&img, strip)?
                        }
                    }
                }
            }
        }
        "webp" | "avif" | "jxl" => {
            note = Some(fmt.to_uppercase());
            encode_via_ffmpeg(&img, &fmt, q)?
        }
        "gif" => encode_simple(&img, ImageFormat::Gif, bg)?,
        "bmp" => encode_simple(&img, ImageFormat::Bmp, bg)?,
        "tiff" => encode_simple(&img, ImageFormat::Tiff, bg)?,
        "ico" => encode_simple(&img, ImageFormat::Ico, bg)?,
        "tga" => encode_simple(&img, ImageFormat::Tga, bg)?,
        other => return Err(format!("Формат {other} не поддерживается")),
    };
    if ctx.cancelled() {
        return Err(CANCELLED.into());
    }
    let out_ext = if fmt == src_fmt && !src_ext.is_empty() { src_ext.clone() } else { fmt.clone() };
    let suffix = if compress { "_compressed" } else { "" };
    let dst = paths::reserve(src, ctx.out_dir.as_deref(), suffix, &out_ext)?;
    let final_data: &[u8] = if compress && fmt == src_fmt && !modified && data.len() as u64 >= src_size {
        note = Some("Сильнее сжать нельзя, сохранена копия".into());
        &src_bytes
    } else {
        &data
    };
    if !notes.is_empty() {
        let extra = notes.join(" · ");
        note = Some(match note {
            Some(n) => format!("{n} · {extra}"),
            None => extra,
        });
    }
    if let Err(e) = std::fs::write(&dst, final_data) {
        paths::discard(&dst);
        return Err(format!("Не удалось записать файл: {e}"));
    }
    ctx.set(1.0);
    Ok(output(&dst, note))
}
