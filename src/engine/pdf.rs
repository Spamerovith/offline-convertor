//! PDF compression: recompresses embedded photos (mozjpeg), caps their resolution,
//! compresses all streams and drops unused objects. Text and vector content stay untouched.

use std::path::Path;

use lopdf::{Document, Object, ObjectId, Stream};
use rayon::prelude::*;
use serde::Deserialize;

use super::paths;
use super::{output, Ctx, JobResult, CANCELLED};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PdfOpts {
    pub quality: f64,
    /// Longest side of embedded images in pixels; 0 keeps the resolution.
    pub max_side: u32,
    pub grayscale: bool,
}

impl Default for PdfOpts {
    fn default() -> Self {
        Self { quality: 85.0, max_side: 2339, grayscale: false }
    }
}

struct Candidate {
    id: ObjectId,
    stream: Stream,
}

fn int(s: &Stream, k: &[u8]) -> Option<i64> {
    s.dict.get(k).ok().and_then(|o| o.as_i64().ok())
}

/// Number of colour components for simple colour spaces (resolving one ICCBased level).
fn components(doc: &Document, cs: &Object) -> Option<u32> {
    let cs = match cs {
        Object::Reference(r) => doc.get_object(*r).ok()?,
        o => o,
    };
    match cs {
        Object::Name(n) => match n.as_slice() {
            b"DeviceRGB" | b"CalRGB" => Some(3),
            b"DeviceGray" | b"CalGray" => Some(1),
            _ => None,
        },
        Object::Array(a) => {
            let first = a.first()?.as_name().ok()?;
            match first {
                b"ICCBased" => {
                    let r = a.get(1)?.as_reference().ok()?;
                    let s = doc.get_object(r).ok()?.as_stream().ok()?;
                    let n = s.dict.get(b"N").ok()?.as_i64().ok()?;
                    if n == 1 || n == 3 {
                        Some(n as u32)
                    } else {
                        None
                    }
                }
                b"CalRGB" => Some(3),
                b"CalGray" => Some(1),
                _ => None,
            }
        }
        _ => None,
    }
}

fn few_colors(rgb: &[u8], comps: usize) -> bool {
    let px = rgb.len() / comps;
    let step = (px / 60_000).max(1);
    let mut set = std::collections::HashSet::new();
    let mut i = 0;
    while i < px {
        let o = i * comps;
        let key = if comps == 3 { (rgb[o] as u32) << 16 | (rgb[o + 1] as u32) << 8 | rgb[o + 2] as u32 } else { rgb[o] as u32 };
        set.insert(key);
        if set.len() > 512 {
            return false;
        }
        i += step;
    }
    true
}

fn recompress(c: &Candidate, comps: u32, o: &PdfOpts, q: f32) -> Option<Stream> {
    let s = &c.stream;
    let filters: Vec<Vec<u8>> = s.filters().ok()?.into_iter().map(|f| f.to_vec()).collect();
    let w = int(s, b"Width")? as u32;
    let h = int(s, b"Height")? as u32;
    if w < 48 || h < 48 {
        return None;
    }
    let has_mask = s.dict.has(b"SMask") || s.dict.has(b"Mask");
    let img: image::DynamicImage = if filters.len() == 1 && filters[0] == b"DCTDecode" {
        let d = image::load_from_memory_with_format(&s.content, image::ImageFormat::Jpeg).ok()?;
        if d.width() != w || d.height() != h {
            return None;
        }
        d
    } else if filters.iter().all(|f| f == b"FlateDecode") && !filters.is_empty() {
        if int(s, b"BitsPerComponent")? != 8 {
            return None;
        }
        let raw = s.decompressed_content_with_limit(1 << 31).ok()?;
        let need = (w * h * comps) as usize;
        if raw.len() < need {
            return None;
        }
        let raw = raw[..need].to_vec();
        if few_colors(&raw, comps as usize) {
            return None; // graphics / screenshots: JPEG would add artifacts
        }
        if comps == 3 {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(w, h, raw)?)
        } else {
            image::DynamicImage::ImageLuma8(image::GrayImage::from_raw(w, h, raw)?)
        }
    } else {
        return None;
    };
    let mut img = img;
    if o.grayscale && comps == 3 {
        img = image::DynamicImage::ImageLuma8(img.to_luma8());
    }
    if o.max_side > 0 && !has_mask && w.max(h) > o.max_side {
        let k = o.max_side as f64 / w.max(h) as f64;
        let nw = ((w as f64 * k).round() as u32).max(1);
        let nh = ((h as f64 * k).round() as u32).max(1);
        img = img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3);
    }
    let gray = matches!(img.color(), image::ColorType::L8);
    let (cs, buf) = if gray {
        (mozjpeg::ColorSpace::JCS_GRAYSCALE, img.to_luma8().into_raw())
    } else {
        (mozjpeg::ColorSpace::JCS_RGB, img.to_rgb8().into_raw())
    };
    let (nw, nh) = (img.width() as usize, img.height() as usize);
    let jpeg = std::panic::catch_unwind(move || -> Option<Vec<u8>> {
        let mut c = mozjpeg::Compress::new(cs);
        c.set_size(nw, nh);
        c.set_quality(q);
        c.set_progressive_mode();
        c.set_optimize_scans(true);
        c.set_optimize_coding(true);
        if !gray {
            c.set_chroma_sampling_pixel_sizes((2, 2), (2, 2));
        }
        let mut st = c.start_compress(Vec::new()).ok()?;
        st.write_scanlines(&buf).ok()?;
        st.finish().ok()
    })
    .ok()??;
    if jpeg.len() as f64 > s.content.len() as f64 * 0.92 {
        return None;
    }
    let mut dict = s.dict.clone();
    dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
    dict.remove(b"DecodeParms");
    dict.set("Width", Object::Integer(nw as i64));
    dict.set("Height", Object::Integer(nh as i64));
    dict.set("BitsPerComponent", Object::Integer(8));
    if gray && comps == 3 || o.grayscale {
        dict.set("ColorSpace", Object::Name(if gray { b"DeviceGray".to_vec() } else { b"DeviceRGB".to_vec() }));
    }
    dict.set("Length", Object::Integer(jpeg.len() as i64));
    let mut ns = Stream::new(dict, jpeg);
    ns.allows_compression = false;
    Some(ns)
}

pub fn run(src: &Path, o: &PdfOpts, ctx: &Ctx) -> JobResult {
    ctx.stage(0.02, "Чтение PDF");
    let src_size = super::file_size(src);
    let mut doc = Document::load(src).map_err(|e| format!("Не удалось открыть PDF: {e}"))?;
    if doc.is_encrypted() && doc.decrypt("").is_err() {
        return Err("PDF защищён паролем".into());
    }
    let q = (30.0 + o.quality.clamp(0.0, 100.0) * 0.6) as f32;

    let mut cands: Vec<(Candidate, u32)> = vec![];
    for (id, obj) in doc.objects.iter() {
        let Object::Stream(s) = obj else { continue };
        let is_img = s.dict.get(b"Subtype").ok().and_then(|v| v.as_name().ok()) == Some(b"Image".as_slice());
        if !is_img || s.dict.has(b"Decode") {
            continue;
        }
        if s.dict.get(b"ImageMask").ok().and_then(|v| v.as_bool().ok()) == Some(true) {
            continue;
        }
        let Some(cs) = s.dict.get(b"ColorSpace").ok() else { continue };
        let Some(n) = components(&doc, cs) else { continue };
        cands.push((Candidate { id: *id, stream: s.clone() }, n));
    }
    let total = cands.len().max(1);
    let done = std::sync::atomic::AtomicUsize::new(0);
    ctx.stage(0.05, &format!("Изображений: {}", cands.len()));
    let results: Vec<(ObjectId, Stream)> = cands
        .par_iter()
        .filter_map(|(c, n)| {
            if ctx.cancelled() {
                return None;
            }
            let r = recompress(c, *n, o, q).map(|s| (c.id, s));
            let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            ctx.set(0.05 + 0.8 * d as f64 / total as f64);
            r
        })
        .collect();
    if ctx.cancelled() {
        return Err(CANCELLED.into());
    }
    let replaced = results.len();
    for (id, s) in results {
        doc.objects.insert(id, Object::Stream(s));
    }
    ctx.stage(0.88, "Сжатие потоков");
    doc.delete_zero_length_streams();
    doc.prune_objects();
    doc.compress();
    let mut buf: Vec<u8> = Vec::new();
    doc.save_modern(&mut buf).map_err(|e| format!("Не удалось сохранить PDF: {e}"))?;
    let dst = paths::reserve(src, ctx.out_dir.as_deref(), "_compressed", "pdf")?;
    let mut note = Some(format!("Пережато изображений: {replaced}"));
    let data: Vec<u8> = if buf.len() as u64 >= src_size {
        note = Some("Сильнее сжать нельзя, сохранена копия".into());
        std::fs::read(src).map_err(|e| e.to_string())?
    } else {
        buf
    };
    if let Err(e) = std::fs::write(&dst, data) {
        paths::discard(&dst);
        return Err(e.to_string());
    }
    ctx.set(1.0);
    Ok(output(&dst, note))
}
