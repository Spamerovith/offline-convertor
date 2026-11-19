//! Offline text recognition.
//! Engine: built-in Windows OCR (Windows.Media.Ocr), run with the Russian and English models at once
//! and fused word by word; adaptive upscaling of small text; deskew; layout reconstruction
//! (columns, paragraphs, lists, headings). PDFs use the real text layer when it is valid
//! (positioned glyphs via pdf-extract) and OCR of the rendered page (Windows.Data.Pdf) otherwise.

use std::path::Path;

use image::{DynamicImage, GrayImage, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use windows::core::HSTRING;
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Storage::StorageFile;
use windows::Storage::Streams::{Buffer, InMemoryRandomAccessStream};

use super::layout::{self, Para, TLine};
use super::paths;
use super::{output, Ctx, JobResult, CANCELLED};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OcrOpts {
    /// "auto" (Russian + English together) or a tag like "ru", "en-US"
    pub lang: String,
    /// "txt" or "docx"
    pub output: String,
    /// Contrast stretch for scans and photos of documents
    pub preprocess: bool,
    /// Put the page picture into the Word file above its text
    pub include_images: bool,
    /// For PDF: "auto" (text layer per page when it's real text, else OCR), "ocr", "text"
    pub pdf_mode: String,
}

impl Default for OcrOpts {
    fn default() -> Self {
        Self { lang: "auto".into(), output: "txt".into(), preprocess: false, include_images: false, pdf_mode: "auto".into() }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OcrLanguage {
    pub tag: String,
    pub name: String,
}

pub fn init_thread() {
    unsafe {
        let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
    }
}

pub fn languages() -> Vec<OcrLanguage> {
    init_thread();
    let mut v = vec![];
    if let Ok(list) = OcrEngine::AvailableRecognizerLanguages() {
        for l in list {
            v.push(OcrLanguage {
                tag: l.LanguageTag().map(|s| s.to_string()).unwrap_or_default(),
                name: l.NativeName().map(|s| s.to_string()).unwrap_or_default(),
            });
        }
    }
    v
}

const NO_OCR: &str = "Распознавание текста недоступно: в Windows не установлен языковой пакет OCR. Параметры - Время и язык - Язык: добавьте русский или английский язык";

fn engine_for(tag: &str) -> Option<OcrEngine> {
    Language::CreateLanguage(&HSTRING::from(tag)).and_then(|l| OcrEngine::TryCreateFromLanguage(&l)).ok()
}

/// Primary engine first; in auto mode Russian and English run together.
fn engines(lang: &str) -> Result<Vec<OcrEngine>, String> {
    let langs = languages();
    let mut out = vec![];
    if lang == "auto" || lang.is_empty() {
        for prefix in ["ru", "en"] {
            if let Some(l) = langs.iter().find(|l| l.tag.to_lowercase().starts_with(prefix)) {
                if let Some(e) = engine_for(&l.tag) {
                    out.push(e);
                }
            }
        }
        if out.is_empty() {
            if let Ok(e) = OcrEngine::TryCreateFromUserProfileLanguages() {
                out.push(e);
            }
        }
    } else if let Some(e) = engine_for(lang) {
        out.push(e);
    }
    if out.is_empty() {
        Err(NO_OCR.into())
    } else {
        Ok(out)
    }
}

fn to_bitmap(img: &RgbaImage) -> windows::core::Result<SoftwareBitmap> {
    let (w, h) = img.dimensions();
    let mut bgra = img.as_raw().clone();
    for p in bgra.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let buf = CryptographicBuffer::CreateFromByteArray(&bgra)?;
    SoftwareBitmap::CreateCopyFromBuffer(&buf, BitmapPixelFormat::Bgra8, w as i32, h as i32)
}

fn from_bitmap(bmp: &SoftwareBitmap) -> windows::core::Result<RgbaImage> {
    let bmp = SoftwareBitmap::ConvertWithAlpha(bmp, BitmapPixelFormat::Bgra8, BitmapAlphaMode::Ignore)?;
    let w = bmp.PixelWidth()? as u32;
    let h = bmp.PixelHeight()? as u32;
    let buf = Buffer::Create(w * h * 4)?;
    bmp.CopyToBuffer(&buf)?;
    let mut arr = windows::core::Array::<u8>::new();
    CryptographicBuffer::CopyToByteArray(&buf, &mut arr)?;
    let mut v = arr.to_vec();
    for p in v.chunks_exact_mut(4) {
        p.swap(0, 2);
        p[3] = 255;
    }
    Ok(RgbaImage::from_raw(w, h, v).unwrap_or_else(|| RgbaImage::new(1, 1)))
}

// ── Image preparation ───────────────────────────────────────────────────

/// Percentile contrast stretch (scans, faded photos of documents).
fn stretch_contrast(img: &RgbaImage) -> RgbaImage {
    let g: GrayImage = DynamicImage::ImageRgba8(img.clone()).to_luma8();
    let mut hist = [0u64; 256];
    for p in g.pixels() {
        hist[p[0] as usize] += 1;
    }
    let total: u64 = hist.iter().sum();
    let pick = |rev: bool| {
        let mut acc = 0;
        let it: Box<dyn Iterator<Item = (usize, &u64)>> = if rev { Box::new(hist.iter().enumerate().rev()) } else { Box::new(hist.iter().enumerate()) };
        for (i, c) in it {
            acc += c;
            if acc * 200 >= total {
                return i;
            }
        }
        if rev {
            255
        } else {
            0
        }
    };
    let (lo, hi) = (pick(false), pick(true));
    if hi <= lo + 20 {
        return img.clone();
    }
    let span = (hi - lo) as f32;
    let mut out = img.clone();
    for p in out.pixels_mut() {
        for c in 0..3 {
            p[c] = (((p[c] as f32 - lo as f32) / span) * 255.0).clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Bilinear rotation around the centre, canvas expanded, filled with the page background.
fn rotate(img: &RgbaImage, deg: f64) -> RgbaImage {
    let (w, h) = (img.width() as f64, img.height() as f64);
    let (s, c) = deg.to_radians().sin_cos();
    let nw = (w * c.abs() + h * s.abs()).ceil() as u32;
    let nh = (w * s.abs() + h * c.abs()).ceil() as u32;
    let corners = [img.get_pixel(0, 0), img.get_pixel(img.width() - 1, 0), img.get_pixel(0, img.height() - 1), img.get_pixel(img.width() - 1, img.height() - 1)];
    let mut bg = [0u32; 3];
    for p in corners {
        for i in 0..3 {
            bg[i] += p[i] as u32;
        }
    }
    let bg = Rgba([(bg[0] / 4) as u8, (bg[1] / 4) as u8, (bg[2] / 4) as u8, 255]);
    let mut out = RgbaImage::from_pixel(nw, nh, bg);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (ncx, ncy) = (nw as f64 / 2.0, nh as f64 / 2.0);
    for y in 0..nh {
        for x in 0..nw {
            let dx = x as f64 - ncx;
            let dy = y as f64 - ncy;
            let sx = dx * c + dy * s + cx;
            let sy = -dx * s + dy * c + cy;
            if sx < 0.0 || sy < 0.0 || sx >= w - 1.0 || sy >= h - 1.0 {
                continue;
            }
            let (x0, y0) = (sx.floor() as u32, sy.floor() as u32);
            let (fx, fy) = (sx - x0 as f64, sy - y0 as f64);
            let p00 = img.get_pixel(x0, y0);
            let p10 = img.get_pixel(x0 + 1, y0);
            let p01 = img.get_pixel(x0, y0 + 1);
            let p11 = img.get_pixel(x0 + 1, y0 + 1);
            let mut px = [0u8; 4];
            for i in 0..4 {
                let v = p00[i] as f64 * (1.0 - fx) * (1.0 - fy) + p10[i] as f64 * fx * (1.0 - fy) + p01[i] as f64 * (1.0 - fx) * fy + p11[i] as f64 * fx * fy;
                px[i] = v.round() as u8;
            }
            out.put_pixel(x, y, Rgba(px));
        }
    }
    out
}

fn fit_max(img: RgbaImage, maxd: u32) -> RgbaImage {
    if img.width() <= maxd && img.height() <= maxd {
        return img;
    }
    DynamicImage::ImageRgba8(img).resize(maxd, maxd, image::imageops::FilterType::Lanczos3).to_rgba8()
}

// ── Recognition ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct OWord {
    text: String,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

#[derive(Debug, Clone)]
struct OLine {
    words: Vec<OWord>,
    text: String,
}

impl OLine {
    fn bbox(&self) -> (f32, f32, f32, f32) {
        let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for w in &self.words {
            b = (b.0.min(w.x0), b.1.min(w.y0), b.2.max(w.x1), b.3.max(w.y1));
        }
        b
    }
}

fn recognize_raw(engine: &OcrEngine, img: &RgbaImage) -> Result<(Vec<OLine>, Option<f64>), String> {
    let bmp = to_bitmap(img).map_err(|e| e.to_string())?;
    let res = engine.RecognizeAsync(&bmp).map_err(|e| e.to_string())?.join().map_err(|e| e.to_string())?;
    let angle = res.TextAngle().ok().and_then(|r| r.Value().ok());
    let mut lines = vec![];
    for line in res.Lines().map_err(|e| e.to_string())? {
        let mut words = vec![];
        if let Ok(ws) = line.Words() {
            for w in ws {
                let text = w.Text().map(|s| s.to_string()).unwrap_or_default();
                if let Ok(r) = w.BoundingRect() {
                    words.push(OWord { text, x0: r.X, y0: r.Y, x1: r.X + r.Width, y1: r.Y + r.Height });
                }
            }
        }
        if words.is_empty() {
            continue;
        }
        let text = line.Text().map(|s| s.to_string()).unwrap_or_default();
        lines.push(OLine { words, text });
    }
    Ok((lines, angle))
}

fn median_word_height(lines: &[OLine]) -> f32 {
    let mut v: Vec<f32> = lines.iter().flat_map(|l| l.words.iter().map(|w| w.y1 - w.y0)).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

// Russian letters that have no Latin look-alike.
const STRONG_CYR: &str = "бгдёжзийлпфцчшщъыьэюяБГДЁЖЗИЙЛПФЦЧШЩЪЫЬЭЮЯ";

fn is_cyr(c: char) -> bool {
    ('\u{0400}'..='\u{04FF}').contains(&c)
}

#[derive(Default)]
struct Script {
    cyr: usize,
    strong: usize,
    latin: usize,
    digits: usize,
}

fn script(s: &str) -> Script {
    let mut k = Script::default();
    for c in s.chars() {
        if is_cyr(c) {
            k.cyr += 1;
            if STRONG_CYR.contains(c) {
                k.strong += 1;
            }
        } else if c.is_ascii_alphabetic() {
            k.latin += 1;
        } else if c.is_ascii_digit() {
            k.digits += 1;
        }
    }
    k
}

/// Chooses between the Russian and the English reading of the same piece of text.
fn choose(ru: &str, en: &str, russian_context: bool) -> String {
    if en.is_empty() {
        return ru.to_string();
    }
    let r = script(ru);
    let e = script(en);
    if e.cyr > 0 {
        return ru.to_string();
    }
    // "песня.тр3" vs "neCHA.mp3": decide each dotted part on its own.
    for sep in ['.', '@', '/', '_'] {
        let rp: Vec<&str> = ru.split(sep).collect();
        let ep: Vec<&str> = en.split(sep).collect();
        if rp.len() > 1 && rp.len() == ep.len() {
            let n = rp.len();
            return rp
                .iter()
                .zip(&ep)
                .enumerate()
                .map(|(i, (a, b))| {
                    // File extension ("песня.mp3"): short, lower-case Latin in the English reading
                    let ext = sep == '.' && i == n - 1 && b.chars().count() <= 4 && !b.is_empty()
                        && b.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
                    if ext { b.to_string() } else { choose(a, b, russian_context) }
                })
                .collect::<Vec<_>>()
                .join(&sep.to_string());
        }
    }
    if r.cyr == 0 {
        // Latin, digits, punctuation: the English model reads them better (I/l, 0/O, %).
        return en.to_string();
    }
    if r.strong > 0 {
        // "З." read for "3." in a number
        let digit_like = ru.chars().filter(|c| is_cyr(*c)).all(|c| "ЗзбОо".contains(c));
        if digit_like && e.digits > 0 && e.latin == 0 {
            return en.to_string();
        }
        // "ассигасу", "омег", "ике": an English word inside English text
        if !russian_context && e.latin > 0 && plausible_en(en) {
            return en.to_string();
        }
        return ru.to_string();
    }
    // Only look-alike letters (а в е к м н о р с т у х): codes like "mp3", or Russian words like "кот".
    if r.digits > 0 && e.latin > 0 {
        return en.to_string();
    }
    // "CKaH B TeKCT" is the English model reading Russian "Скан в текст"
    if !plausible_en(en) {
        return ru.to_string();
    }
    if russian_context {
        ru.to_string()
    } else {
        en.to_string()
    }
}

/// English words are lower case, UPPER CASE or Capitalised; "CKaH" or "TeKCT" are not.
fn plausible_en(w: &str) -> bool {
    // Digits between letters ("a63au", "0ffline") mean the model is guessing.
    let core: Vec<char> = w.chars().filter(|c| c.is_alphanumeric()).collect();
    let has_letter = core.iter().any(|c| c.is_alphabetic());
    if has_letter {
        let first_l = core.iter().position(|c| c.is_alphabetic()).unwrap();
        let last_l = core.iter().rposition(|c| c.is_alphabetic()).unwrap();
        if core[first_l..=last_l].iter().any(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    let letters: Vec<char> = w.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() <= 1 {
        return letters.first().is_none_or(|c| c.is_lowercase() || *c == 'I' || *c == 'A');
    }
    let lower = letters.iter().all(|c| c.is_lowercase());
    let upper = letters.iter().all(|c| c.is_uppercase());
    let cap = letters[0].is_uppercase() && letters[1..].iter().all(|c| c.is_lowercase());
    lower || upper || cap
}

/// Word-level fusion of the Russian (primary) and English readings.
fn fuse(ru: &[OLine], en: &[OLine]) -> Vec<TLine> {
    let mut used_en = vec![false; en.len()];
    let mut out = vec![];
    for rl in ru {
        let (x0, y0, x1, y1) = rl.bbox();
        let rh = (y1 - y0).max(1.0);
        // English words on the same visual line.
        let mut ew: Vec<&OWord> = vec![];
        for (i, el) in en.iter().enumerate() {
            let (_, ey0, _, ey1) = el.bbox();
            let v = (y1.min(ey1) - y0.max(ey0)).max(0.0);
            if v >= 0.5 * rh.min((ey1 - ey0).max(1.0)) {
                for w in &el.words {
                    if w.x1 > x0 - rh && w.x0 < x1 + rh {
                        ew.push(w);
                        used_en[i] = true;
                    }
                }
            }
        }
        ew.sort_by(|a, b| a.x0.partial_cmp(&b.x0).unwrap());
        let strong = rl.words.iter().filter(|w| script(&w.text).strong > 0).count();
        // Mixed-script words ("tоmе") only happen when Latin text is read by the Russian model.
        let latin = rl.words.iter().filter(|w| script(&w.text).latin > 0).count();
        let russian_context = strong >= latin;

        // Clusters of Russian and English words connected by horizontal overlap.
        let mut text = String::new();
        let mut ri = 0;
        let mut ei = 0;
        let rw = &rl.words;
        while ri < rw.len() {
            let mut cx1 = rw[ri].x1;
            let mut rs = vec![rw[ri].text.clone()];
            let cx0 = rw[ri].x0;
            let first = ri;
            ri += 1;
            let mut es: Vec<String> = vec![];
            loop {
                let mut grew = false;
                while ei < ew.len() && ew[ei].x1 <= cx0 + 1.0 {
                    ei += 1; // English word before this cluster with no Russian counterpart
                }
                while ei < ew.len() && ew[ei].x0 < cx1 - 1.0 {
                    es.push(ew[ei].text.clone());
                    if ew[ei].x1 > cx1 {
                        cx1 = ew[ei].x1;
                        grew = true;
                    }
                    ei += 1;
                }
                while ri < rw.len() && rw[ri].x0 < cx1 - 1.0 {
                    rs.push(rw[ri].text.clone());
                    if rw[ri].x1 > cx1 {
                        cx1 = rw[ri].x1;
                        grew = true;
                    }
                    ri += 1;
                }
                if !grew {
                    break;
                }
            }
            // Language of the neighbourhood (two words each side): mixed lines switch mid-way.
            let lo = first.saturating_sub(2);
            let hi = (ri + 2).min(rw.len());
            let near: Vec<&OWord> = rw[lo..first].iter().chain(rw[ri..hi].iter()).collect();
            let n_strong = near.iter().filter(|w| script(&w.text).strong > 0).count();
            let n_latin = near.iter().filter(|w| script(&w.text).latin > 0).count();
            let ctx = if n_strong + n_latin == 0 { russian_context } else { n_strong >= n_latin };
            let piece = choose(&rs.join(" "), &es.join(" "), ctx);
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&piece);
        }
        out.push(TLine { text, x0, y0, x1, y1 });
    }
    // Lines only the English model found (clean Latin text the Russian model missed).
    for (i, el) in en.iter().enumerate() {
        if used_en[i] {
            continue;
        }
        let s = script(&el.text);
        let letters = el.text.chars().filter(|c| c.is_alphanumeric()).count();
        if s.latin >= 3 && letters * 10 >= el.text.chars().filter(|c| !c.is_whitespace()).count() * 8 {
            let (x0, y0, x1, y1) = el.bbox();
            out.push(TLine { text: el.text.clone(), x0, y0, x1, y1 });
        }
    }
    out
}

fn to_tlines(lines: &[OLine]) -> Vec<TLine> {
    lines
        .iter()
        .map(|l| {
            let (x0, y0, x1, y1) = l.bbox();
            TLine { text: l.text.clone(), x0, y0, x1, y1 }
        })
        .collect()
}

/// Full recognition of one image into paragraphs.
fn recognize_image(engs: &[OcrEngine], img: &RgbaImage, preprocess: bool) -> Result<Vec<Para>, String> {
    let maxd = OcrEngine::MaxImageDimension().unwrap_or(10000).min(10000);
    let mut img = fit_max(if preprocess { stretch_contrast(img) } else { img.clone() }, maxd);
    let primary = &engs[0];
    let (mut lines, angle) = recognize_raw(primary, &img)?;
    let mut changed = false;
    // Deskew photos of pages.
    if let Some(a) = angle {
        if a.abs() > 0.3 && a.abs() < 45.0 {
            img = fit_max(rotate(&img, -a), maxd);
            changed = true;
        }
    }
    // The engine is most accurate at ~30 px text height: enlarge small text, shrink huge text.
    let h = if changed { 0.0 } else { median_word_height(&lines) };
    let h = if h == 0.0 && changed { median_word_height(&recognize_raw(primary, &img)?.0) } else { h };
    let scale = if h > 0.0 && h < 22.0 {
        (32.0 / h).min(4.0)
    } else if h > 120.0 {
        48.0 / h
    } else if lines.is_empty() && img.width().max(img.height()) < 1600 {
        2.0
    } else {
        1.0
    };
    if (scale - 1.0).abs() > 0.05 {
        let nw = ((img.width() as f32 * scale) as u32).clamp(1, maxd);
        let nh = ((img.height() as f32 * scale) as u32).clamp(1, maxd);
        img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Lanczos3);
        changed = true;
    }
    if changed {
        lines = recognize_raw(primary, &img)?.0;
    }
    let tl = if engs.len() > 1 {
        let (en, _) = recognize_raw(&engs[1], &img)?;
        fuse(&lines, &en)
    } else {
        to_tlines(&lines)
    };
    Ok(layout::paragraphs(tl))
}

// ── PDF text layer (positioned glyphs) ─────────────────────────────────

struct Glyph {
    x: f32,
    y: f32,
    w: f32,
    size: f32,
    ch: String,
}

#[derive(Default)]
struct PosDev {
    pages: Vec<Vec<Glyph>>,
    height: f64,
}

impl pdf_extract::OutputDev for PosDev {
    fn begin_page(&mut self, _n: u32, mb: &pdf_extract::MediaBox, _a: Option<(f64, f64, f64, f64)>) -> Result<(), pdf_extract::OutputError> {
        self.height = mb.ury - mb.lly;
        self.pages.push(vec![]);
        Ok(())
    }
    fn end_page(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }
    fn output_character(&mut self, trm: &pdf_extract::Transform, width: f64, _sp: f64, font_size: f64, ch: &str) -> Result<(), pdf_extract::OutputError> {
        let vx = font_size * trm.m11 + font_size * trm.m21;
        let vy = font_size * trm.m12 + font_size * trm.m22;
        let mut size = (vx * vy).abs().sqrt();
        if !(size > 0.1) {
            size = font_size.abs().max(1.0);
        }
        if let Some(p) = self.pages.last_mut() {
            p.push(Glyph { x: trm.m31 as f32, y: (self.height - trm.m32) as f32, w: (width * size) as f32, size: size as f32, ch: ch.to_string() });
        }
        Ok(())
    }
    fn begin_word(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }
    fn end_word(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }
    fn end_line(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }
}

struct Run {
    text: String,
    x0: f32,
    x1: f32,
    base: f32,
    size: f32,
    /// End of the previous glyph, with implausible advance widths capped
    end: f32,
    last_x: f32,
}

fn glyph_lines(glyphs: &[Glyph]) -> Vec<TLine> {
    let mut out = vec![];
    let mut cur: Option<Run> = None;
    let mut pending_space = false;
    let flush = |c: Option<Run>, out: &mut Vec<TLine>| {
        if let Some(r) = c {
            if !r.text.trim().is_empty() {
                out.push(TLine { text: r.text.trim().to_string(), x0: r.x0, y0: r.base - r.size * 0.8, x1: r.x1, y1: r.base + r.size * 0.2 });
            }
        }
    };
    for g in glyphs {
        if g.ch.trim().is_empty() {
            pending_space = true;
            continue;
        }
        // Some fonts report a full-em advance for punctuation; never trust more than 0.8 em.
        let adv = g.w.clamp(0.0, 0.8 * g.size);
        let same = cur.as_ref().is_some_and(|c| {
            let s = c.size.max(g.size);
            (g.y - c.base).abs() < 0.45 * s
                && g.x >= c.last_x - 0.25 * s
                // A gap wider than ~1.6 em is a column gutter or a table cell, not a word space.
                && g.x - c.end < 1.6 * s
        });
        if same {
            let c = cur.as_mut().unwrap();
            if pending_space || g.x - c.end > 0.2 * g.size {
                c.text.push(' ');
            }
            c.text.push_str(&g.ch);
            c.x1 = c.x1.max(g.x + adv);
            c.size = c.size.max(g.size);
            c.end = g.x + adv;
            c.last_x = g.x;
        } else {
            flush(cur.take(), &mut out);
            cur = Some(Run { text: g.ch.clone(), x0: g.x, x1: g.x + adv, base: g.y, size: g.size, end: g.x + adv, last_x: g.x });
        }
        pending_space = false;
    }
    flush(cur, &mut out);
    out
}

/// Real text (not garbage from broken font encodings)?
fn text_ok(lines: &[TLine]) -> bool {
    let t: String = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" ");
    let total = t.chars().filter(|c| !c.is_whitespace()).count();
    if total < 20 {
        return false;
    }
    let good = t.chars().filter(|c| !c.is_whitespace() && (c.is_alphanumeric() || c.is_ascii_punctuation() || "«»…№•\u{2013}\u{2014}".contains(*c))).count();
    let bad = t.chars().filter(|c| *c == '\u{FFFD}' || (c.is_control() && !c.is_whitespace())).count();
    good as f64 >= total as f64 * 0.9 && bad * 50 <= total
}

fn pdf_text_pages(src: &Path) -> Option<Vec<Vec<TLine>>> {
    let src = src.to_path_buf();
    std::panic::catch_unwind(move || {
        let mut doc = pdf_extract::Document::load(&src).ok()?;
        if doc.is_encrypted() {
            doc.decrypt("").ok()?;
        }
        let mut dev = PosDev::default();
        pdf_extract::output_doc(&doc, &mut dev).ok()?;
        if let Some(p) = std::env::var_os("OC_DEBUG_GLYPHS") {
            let mut s = String::new();
            for page in &dev.pages {
                for l in glyph_lines(page) {
                    s.push_str(&format!("x {:.0}-{:.0}\ty {:.0}-{:.0}\t{}\n", l.x0, l.x1, l.y0, l.y1, l.text));
                }
            }
            let _ = std::fs::write(p, s);
        }
        Some(dev.pages.iter().map(|g| glyph_lines(g)).collect())
    })
    .ok()
    .flatten()
}

struct PdfPages {
    pdf: PdfDocument,
}

impl PdfPages {
    fn open(path: &Path) -> Result<Self, String> {
        let full = std::fs::canonicalize(path).unwrap_or(path.to_path_buf());
        let p = full.to_string_lossy().trim_start_matches(r"\\?\").to_string();
        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(p)).map_err(|e| e.to_string())?.join().map_err(|e| e.to_string())?;
        let pdf = PdfDocument::LoadFromFileAsync(&file)
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_| "Не удалось открыть PDF (возможно, он защищён паролем)".to_string())?;
        Ok(Self { pdf })
    }
    fn count(&self) -> u32 {
        self.pdf.PageCount().unwrap_or(0)
    }
    fn render(&self, i: u32) -> Result<RgbaImage, String> {
        let page = self.pdf.GetPage(i).map_err(|e| e.to_string())?;
        let size = page.Size().map_err(|e| e.to_string())?;
        // ~300 dpi (page size is in DIPs, 96 per inch)
        let target_w = ((size.Width as f64) * 300.0 / 96.0).clamp(800.0, 6000.0) as u32;
        let stream = InMemoryRandomAccessStream::new().map_err(|e| e.to_string())?;
        let opts = PdfPageRenderOptions::new().map_err(|e| e.to_string())?;
        opts.SetDestinationWidth(target_w).map_err(|e| e.to_string())?;
        page.RenderWithOptionsToStreamAsync(&stream, &opts).map_err(|e| e.to_string())?.join().map_err(|e| e.to_string())?;
        let dec = BitmapDecoder::CreateAsync(&stream).map_err(|e| e.to_string())?.join().map_err(|e| e.to_string())?;
        let bmp = dec.GetSoftwareBitmapAsync().map_err(|e| e.to_string())?.join().map_err(|e| e.to_string())?;
        from_bitmap(&bmp).map_err(|e| e.to_string())
    }
}

// ── Output ──────────────────────────────────────────────────────────────

struct Page {
    paras: Vec<Para>,
    image: Option<RgbaImage>,
}

fn write_docx(pages: &[Page], dst: &Path) -> Result<(), String> {
    use docx_rs::*;
    let mut doc = Docx::new().default_fonts(RunFonts::new().ascii("Calibri").hi_ansi("Calibri").cs("Calibri").east_asia("Calibri")).default_size(22);
    for (i, p) in pages.iter().enumerate() {
        if i > 0 {
            doc = doc.add_paragraph(Paragraph::new().add_run(Run::new().add_break(BreakType::Page)));
        }
        if let Some(img) = &p.image {
            let small = if img.width() > 1600 { image::imageops::resize(img, 1600, img.height() * 1600 / img.width(), image::imageops::FilterType::Triangle) } else { img.clone() };
            let mut buf = Vec::new();
            DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(small.clone()).to_rgb8())
                .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Jpeg)
                .map_err(|e| e.to_string())?;
            let w_emu: u32 = 5_760_000; // 16 cm
            let h_emu = (w_emu as u64 * small.height() as u64 / small.width().max(1) as u64) as u32;
            doc = doc.add_paragraph(Paragraph::new().add_run(Run::new().add_image(Pic::new(&buf).size(w_emu, h_emu))));
        }
        for t in &p.paras {
            let mut run = Run::new().add_text(&t.text);
            if t.heading {
                run = run.bold().size(30);
            }
            let mut para = Paragraph::new().add_run(run);
            if !t.spaced {
                para = para.line_spacing(LineSpacing::new().before(0).after(0));
            }
            doc = doc.add_paragraph(para);
        }
    }
    let f = std::fs::File::create(dst).map_err(|e| e.to_string())?;
    doc.build().pack(f).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn run(src: &Path, o: &OcrOpts, ctx: &Ctx) -> JobResult {
    init_thread();
    let is_pdf = paths::ext_of(src) == "pdf";
    let out_ext = if o.output == "docx" { "docx" } else { "txt" };
    ctx.stage(0.02, "Подготовка распознавания");
    let mut pages: Vec<Page> = vec![];
    let mut ocr_pages = 0usize;
    let mut text_pages = 0usize;

    if is_pdf {
        let layer = if o.pdf_mode == "ocr" { None } else { pdf_text_pages(src) };
        let renderer = PdfPages::open(src);
        let n = match (&renderer, &layer) {
            (Ok(r), _) => r.count() as usize,
            (Err(_), Some(l)) => l.len(),
            (Err(e), None) => return Err(e.clone()),
        };
        let mut engs: Option<Vec<OcrEngine>> = None;
        for i in 0..n {
            if ctx.cancelled() {
                return Err(CANCELLED.into());
            }
            ctx.stage(0.05 + 0.88 * i as f64 / n.max(1) as f64, &format!("Страница {} из {}", i + 1, n));
            let tl = layer.as_ref().and_then(|l| l.get(i)).filter(|l| o.pdf_mode == "text" || text_ok(l));
            let need_img = tl.is_none() || o.include_images;
            let img = if need_img { renderer.as_ref().ok().map(|r| r.render(i as u32)).transpose()? } else { None };
            let paras = match tl {
                Some(lines) => {
                    text_pages += 1;
                    layout::paragraphs(lines.clone())
                }
                None => match &img {
                    Some(im) => {
                        if engs.is_none() {
                            engs = Some(engines(&o.lang)?);
                        }
                        ocr_pages += 1;
                        recognize_image(engs.as_ref().unwrap(), im, o.preprocess)?
                    }
                    None => vec![],
                },
            };
            pages.push(Page { paras, image: if o.include_images { img } else { None } });
        }
    } else {
        let engs = engines(&o.lang)?;
        ctx.stage(0.1, "Чтение изображения");
        let loaded = super::image_ops::load(src)?;
        let mut img = loaded.img;
        if let Some(or) = loaded.orientation {
            img.apply_orientation(or);
        }
        ctx.stage(0.3, "Распознавание");
        let rgba = img.to_rgba8();
        let paras = recognize_image(&engs, &rgba, o.preprocess)?;
        ocr_pages += 1;
        pages.push(Page { paras, image: if o.include_images { Some(rgba) } else { None } });
    }
    if ctx.cancelled() {
        return Err(CANCELLED.into());
    }
    ctx.stage(0.95, "Сохранение");
    let text: String = pages.iter().map(|p| layout::to_text(&p.paras)).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n");
    let dst = paths::reserve(src, ctx.out_dir.as_deref(), "", out_ext)?;
    let res = if out_ext == "docx" {
        write_docx(&pages, &dst)
    } else {
        // UTF-8 with BOM so Notepad and Word open Cyrillic correctly.
        let mut data = vec![0xEF, 0xBB, 0xBF];
        data.extend_from_slice(text.replace('\n', "\r\n").as_bytes());
        std::fs::write(&dst, data).map_err(|e| e.to_string())
    };
    if let Err(e) = res {
        paths::discard(&dst);
        return Err(e);
    }
    let chars = text.chars().filter(|c| !c.is_whitespace()).count();
    let note = if chars == 0 {
        "Текст не найден".to_string()
    } else if text_pages > 0 && ocr_pages == 0 {
        format!("Текстовый слой PDF · символов: {chars}")
    } else if text_pages > 0 {
        format!("Текстовый слой + распознавание · символов: {chars}")
    } else {
        format!("Распознано символов: {chars}")
    };
    let mut out = output(&dst, Some(note));
    out.text = Some(text.chars().take(200_000).collect());
    Ok(out)
}
