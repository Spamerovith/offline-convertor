//! Reading-order and paragraph reconstruction from positioned text lines.
//! Shared by OCR (Windows.Media.Ocr lines) and PDF text layers (positioned glyphs).

#[derive(Debug, Clone)]
pub struct TLine {
    pub text: String,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl TLine {
    fn h(&self) -> f32 {
        (self.y1 - self.y0).max(1.0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Para {
    pub text: String,
    pub heading: bool,
    /// Separated from the previous paragraph by vertical space (blank line in TXT)
    pub spaced: bool,
}

struct Block {
    lines: Vec<TLine>,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Block {
    fn new(l: TLine) -> Self {
        Self { x0: l.x0, y0: l.y0, x1: l.x1, y1: l.y1, lines: vec![l] }
    }
    fn push(&mut self, l: TLine) {
        self.x0 = self.x0.min(l.x0);
        self.y0 = self.y0.min(l.y0);
        self.x1 = self.x1.max(l.x1);
        self.y1 = self.y1.max(l.y1);
        self.lines.push(l);
    }
}

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return 12.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

/// Groups lines into blocks: a block is a run of lines stacked closely in one column.
fn blocks(mut lines: Vec<TLine>) -> Vec<Block> {
    lines.sort_by(|a, b| a.y0.partial_cmp(&b.y0).unwrap().then(a.x0.partial_cmp(&b.x0).unwrap()));
    let mut out: Vec<Block> = vec![];
    for l in lines {
        let h = l.h();
        let mut best: Option<(usize, f32)> = None;
        for (i, b) in out.iter().enumerate() {
            let last = b.lines.last().unwrap();
            let lh = last.h();
            let ratio = h / lh;
            if !(0.6..=1.65).contains(&ratio) {
                continue;
            }
            let gap = l.y0 - last.y1;
            if gap < -0.5 * h.min(lh) || gap > 2.0 * h.max(lh) {
                continue;
            }
            // Compare with the line directly above, not the whole block: a full-width line above
            // two columns must not pull the right column into the paragraph on the left.
            let ov = overlap(l.x0, l.x1, last.x0, last.x1);
            let minw = (l.x1 - l.x0).min(last.x1 - last.x0).max(1.0);
            if ov / minw < 0.3 {
                continue;
            }
            if best.is_none_or(|(_, g)| gap < g) {
                best = Some((i, gap));
            }
        }
        match best {
            Some((i, _)) => out[i].push(l),
            None => out.push(Block::new(l)),
        }
    }
    out
}

fn cut(bs: Vec<Block>, vertical: bool) -> Vec<Vec<Block>> {
    let mut bs = bs;
    if vertical {
        bs.sort_by(|a, b| a.x0.partial_cmp(&b.x0).unwrap());
    } else {
        bs.sort_by(|a, b| a.y0.partial_cmp(&b.y0).unwrap());
    }
    let mut groups: Vec<Vec<Block>> = vec![];
    let mut max = f32::MIN;
    for b in bs {
        let (start, end) = if vertical { (b.x0, b.x1) } else { (b.y0, b.y1) };
        if groups.is_empty() || start >= max - 1.0 {
            groups.push(vec![]);
        }
        max = max.max(end);
        groups.last_mut().unwrap().push(b);
    }
    groups
}

fn extent_y(g: &[Block]) -> (f32, f32) {
    g.iter().fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x.y0), b.max(x.y1)))
}

/// Recursive XY-cut. A vertical cut (columns, sidebar) is taken first when every side runs
/// most of the region's height; otherwise rows are cut first, then columns.
fn order(bs: Vec<Block>, out: &mut Vec<Block>) {
    if bs.len() <= 1 {
        out.extend(bs);
        return;
    }
    let (ry0, ry1) = extent_y(&bs);
    let rh = (ry1 - ry0).max(1.0);
    let v = cut(bs, true);
    // At least two real columns (sidebar + content, text columns); small side notes don't count.
    let tall = v.len() > 1
        && v.iter()
            .filter(|g| {
                let (a, b) = extent_y(g);
                (b - a) >= 0.6 * rh
            })
            .count()
            >= 2;
    if tall {
        for g in v {
            order(g, out);
        }
        return;
    }
    let all: Vec<Block> = v.into_iter().flatten().collect();
    let h = cut(all, false);
    if h.len() > 1 {
        for g in h {
            order(g, out);
        }
        return;
    }
    let all: Vec<Block> = h.into_iter().flatten().collect();
    let v = cut(all, true);
    if v.len() > 1 {
        for g in v {
            order(g, out);
        }
        return;
    }
    let mut groups = v;
    // Overlapping blocks: plain top-to-bottom, left-to-right.
    let mut bs = groups.pop().unwrap();
    bs.sort_by(|a, b| a.y0.partial_cmp(&b.y0).unwrap().then(a.x0.partial_cmp(&b.x0).unwrap()));
    out.extend(bs);
}

fn is_bullet(s: &str) -> bool {
    let t = s.trim_start();
    let mut ch = t.chars();
    let first = ch.next().unwrap_or(' ');
    if "•·▪◦●○■□➢►-*\u{2013}\u{2014}".contains(first) {
        return true;
    }
    // "1." "2)" "a)" "IV."
    let head: String = t.chars().take_while(|c| c.is_alphanumeric()).collect();
    let rest = &t[head.len()..];
    !head.is_empty() && head.chars().count() <= 3 && (rest.starts_with(". ") || rest.starts_with(") ")) && head.chars().all(|c| c.is_ascii_digit() || c.is_alphabetic())
        && (head.chars().all(|c| c.is_ascii_digit()) || head.chars().count() == 1)
}

fn join(acc: &mut String, next: &str) {
    if acc.is_empty() {
        acc.push_str(next);
        return;
    }
    let ends_hyphen = acc.ends_with('-') && acc.chars().rev().nth(1).is_some_and(|c| c.is_alphabetic());
    let next_lower = next.chars().next().is_some_and(|c| c.is_lowercase());
    if ends_hyphen && next_lower {
        acc.pop(); // soft hyphenation: "перено-" + "сы" = "переносы"
        acc.push_str(next);
    } else {
        acc.push(' ');
        acc.push_str(next);
    }
}

/// Lines in reading order, joined into paragraphs.
pub fn paragraphs(lines: Vec<TLine>) -> Vec<Para> {
    let lines: Vec<TLine> = lines.into_iter().filter(|l| !l.text.trim().is_empty()).collect();
    if lines.is_empty() {
        return vec![];
    }
    let med_h = median(lines.iter().map(|l| l.h()).collect());
    let mut ordered = vec![];
    order(blocks(lines), &mut ordered);

    let mut out: Vec<Para> = vec![];
    for b in ordered {
        let bw = (b.x1 - b.x0).max(1.0);
        // OCR boxes change height with ascenders/descenders: compare against the block's typical line.
        let bh = median(b.lines.iter().map(|l| l.h()).collect());
        let mut cur: Option<Para> = None;
        let mut prev: Option<&TLine> = None;
        for (k, l) in b.lines.iter().enumerate() {
            let h = l.h();
            let text = l.text.trim();
            let heading = h >= med_h * 1.3;
            let mut new_para = true;
            let mut spaced = true;
            if let (Some(p), Some(c)) = (prev, cur.as_ref()) {
                let gap = l.y0 - p.y1;
                // The previous line is "full" when it reaches the right edge of its neighbours;
                // a short line ends a paragraph (or is a list item / address line).
                let lo = k.saturating_sub(4);
                let hi = k + 1;
                let right = b.lines[lo..hi].iter().map(|x| x.x1).fold(f32::MIN, f32::max);
                let prev_full = p.x1 >= right - 0.08 * (right - b.x0) - 2.5 * p.h();
                let indented = l.x0 > b.x0 + 1.2 * bh && prev_full && bw > 12.0 * bh;
                let same_size = (h / p.h() - 1.0).abs() < 0.4;
                spaced = gap > 0.75 * bh;
                new_para = spaced || !same_size || is_bullet(text) || !prev_full || indented || c.heading != heading;
            }
            if new_para {
                if let Some(p) = cur.take() {
                    out.push(p);
                }
                cur = Some(Para { text: text.to_string(), heading, spaced: spaced || out.is_empty() });
            } else if let Some(c) = cur.as_mut() {
                join(&mut c.text, text);
            }
            prev = Some(l);
        }
        if let Some(p) = cur.take() {
            out.push(p);
        }
    }
    // Headings are short; long "large" lines are just big body text.
    for p in &mut out {
        if p.heading && p.text.chars().count() > 120 {
            p.heading = false;
        }
    }
    out
}

/// Plain-text rendering: blank line between spaced paragraphs, single newline otherwise.
pub fn to_text(paras: &[Para]) -> String {
    let mut s = String::new();
    for (i, p) in paras.iter().enumerate() {
        if i > 0 {
            s.push_str(if p.spaced { "\n\n" } else { "\n" });
        }
        s.push_str(&p.text);
    }
    s
}
