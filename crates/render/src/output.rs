//! What a render writes and what the picture viewer keeps.
//!
//! - [`write_picture`] saves a film as an 8-bit sRGB PNG or a 32-bit float EXR with the chosen
//!   [`Pass`]es as layers, cut to the project's region.
//! - [`numbered`] names the files of a frame queue.
//! - [`History`] keeps the pictures of past renders, bounded by memory, for the picture viewer;
//!   [`Histogram`] and [`difference`] describe and compare them.
//!
//! Everything here is plain data: no window and no GPU.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use craft_render_core::Film;
use modelcraft_scene::stage::{OutputFormat, Pass, RenderConfig};

/// The part of a `width × height` picture a region (fractions: left, top, right, bottom) keeps, as
/// `(x, y, width, height)` in pixels; the whole picture without a region, and always at least one
/// pixel.
pub fn crop_rect(width: u32, height: u32, region: Option<[f32; 4]>) -> (u32, u32, u32, u32) {
    let Some(r) = region.filter(|r| r.iter().all(|v| v.is_finite())) else { return (0, 0, width, height) };
    let x0 = ((r[0].clamp(0.0, 1.0) * width as f32).floor() as u32).min(width.saturating_sub(1));
    let y0 = ((r[1].clamp(0.0, 1.0) * height as f32).floor() as u32).min(height.saturating_sub(1));
    let x1 = ((r[2].clamp(0.0, 1.0) * width as f32).ceil() as u32).clamp(x0 + 1, width.max(1));
    let y1 = ((r[3].clamp(0.0, 1.0) * height as f32).ceil() as u32).clamp(y0 + 1, height.max(1));
    (x0, y0, x1 - x0, y1 - y0)
}

fn crop_plane<T: Copy>(src: &[T], width: u32, rect: (u32, u32, u32, u32)) -> Vec<T> {
    let (x, y, w, h) = rect;
    let mut out = Vec::with_capacity(w as usize * h as usize);
    for row in y..y + h {
        let start = row as usize * width as usize + x as usize;
        if let Some(line) = src.get(start..start + w as usize) {
            out.extend_from_slice(line);
        }
    }
    out
}

/// The film as 8-bit sRGB (opaque RGBA), or the filtered image when there is one.
pub fn display_pixels(film: &Film, filtered: Option<&[[f32; 3]]>, exposure: f32) -> Vec<u8> {
    match filtered {
        Some(image) => Film::image_to_srgb8(image, exposure),
        None => film.to_srgb8(exposure),
    }
}

/// Save a PNG: the picture (or the filtered one) cut to `region`.
pub fn write_png(path: &Path, film: &Film, filtered: Option<&[[f32; 3]]>, exposure: f32, region: Option<[f32; 4]>) -> Result<(), String> {
    use craft_image_codecs::{ChannelLayout, EncodeOptions, Format, Image, SampleType};
    let rect = crop_rect(film.width, film.height, region);
    let pixels = display_pixels(film, filtered, exposure);
    let rgba: Vec<[u8; 4]> = pixels.as_chunks::<4>().0.to_vec();
    let cut: Vec<u8> = crop_plane(&rgba, film.width, rect).into_iter().flatten().collect();
    let image = Image::from_raw(rect.2, rect.3, ChannelLayout::Rgba, SampleType::U8, cut).map_err(|e| e.to_string())?;
    let bytes = craft_image_codecs::encode(&image, Format::Png, &EncodeOptions::default()).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Save an OpenEXR of 32-bit float channels: `R G B` (linear radiance, neither exposed nor tone
/// mapped), then each pass in the customary layer names (`albedo.R/G/B`, `N.X/Y/Z`, `Z`,
/// `motion.X/Y`, `variance`). A filtered picture is `R G B` and the original `noisy.R/G/B`.
pub fn write_exr(path: &Path, film: &Film, filtered: Option<&[[f32; 3]]>, passes: &[Pass], region: Option<[f32; 4]>) -> Result<(), String> {
    use craft_exr::decode::{Channel, Compression, Samples};
    let rect = crop_rect(film.width, film.height, region);
    let k = if film.samples > 0 { 1.0 / film.samples as f32 } else { 0.0 };
    let plane = |name: &str, values: Vec<f32>| Channel { name: name.to_string(), samples: Samples::F32(crop_plane(&values, film.width, rect)) };
    let of3 = |src: &[[f32; 3]], c: usize| src.iter().map(|p| p[c] * k).collect::<Vec<f32>>();
    let mut channels = Vec::new();
    for (c, name) in ["R", "G", "B"].into_iter().enumerate() {
        match filtered {
            Some(image) => {
                channels.push(plane(name, image.iter().map(|p| p[c]).collect()));
                channels.push(plane(&format!("noisy.{name}"), of3(&film.sum, c)));
            }
            None => channels.push(plane(name, of3(&film.sum, c))),
        }
        if passes.contains(&Pass::Albedo) {
            channels.push(plane(&format!("albedo.{name}"), of3(&film.albedo, c)));
        }
    }
    if passes.contains(&Pass::Normal) {
        for (c, name) in ["N.X", "N.Y", "N.Z"].into_iter().enumerate() {
            channels.push(plane(name, of3(&film.normal, c)));
        }
    }
    if passes.contains(&Pass::Depth) {
        channels.push(plane("Z", film.depth.iter().map(|d| d * k).collect()));
    }
    if passes.contains(&Pass::Motion) {
        channels.push(plane("motion.X", film.motion.iter().map(|m| m[0] * k).collect()));
        channels.push(plane("motion.Y", film.motion.iter().map(|m| m[1] * k).collect()));
    }
    if passes.contains(&Pass::Variance) {
        channels.push(plane("variance", film.variance()));
    }
    let bytes = craft_exr::decode::write(rect.2, rect.3, &channels, Compression::Zip16).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Save a film as the project's settings say: format, passes, region and exposure.
pub fn write_picture(path: &Path, film: &Film, filtered: Option<&[[f32; 3]]>, config: &RenderConfig) -> Result<(), String> {
    let c = config.sanitized();
    match c.format {
        OutputFormat::Png => write_png(path, film, filtered, c.exposure, c.region),
        OutputFormat::Exr => write_exr(path, film, filtered, &c.passes, c.region),
    }
}

/// The file a frame of a queue is written to. `base` with `#` signs in its name has them replaced
/// by the frame number (`shot_####.png`, four digits); otherwise `_` and four digits go before the
/// extension. Negative frames keep their sign, and the extension is `format`'s.
pub fn numbered(base: &Path, frame: i32, format: OutputFormat) -> PathBuf {
    let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "render".into());
    let hashes = stem.chars().filter(|c| *c == '#').count();
    let name = if hashes > 0 {
        let first = stem.find('#').unwrap_or(0);
        let last = stem.rfind('#').map_or(first, |i| i + 1);
        let digits = hashes.min(12);
        format!("{}{:0digits$}{}", &stem[..first], frame, &stem[last..], digits = digits)
    } else {
        format!("{stem}_{frame:04}")
    };
    base.with_file_name(format!("{name}.{}", format.extension()))
}

// ---------------------------------------------------------------- the picture viewer's data

/// A finished render kept for the picture viewer.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    /// What it was: the frame or time, for the list.
    pub label: String,
    pub size: [u32; 2],
    /// Opaque sRGB RGBA bytes, rows from the top.
    pub pixels: Vec<u8>,
    pub samples: u32,
    pub seconds: f64,
}

impl Picture {
    pub fn bytes(&self) -> usize {
        self.pixels.len() + self.label.len() + 64
    }
}

/// Past renders, newest last, held within a memory budget: adding one evicts the oldest ones
/// until the rest fit, and a picture larger than the whole budget is not kept.
#[derive(Clone, Debug)]
pub struct History {
    budget: usize,
    items: VecDeque<Picture>,
}

/// The default memory a history may hold.
pub const HISTORY_BYTES: usize = 256 << 20;

impl Default for History {
    fn default() -> Self {
        History::new(HISTORY_BYTES)
    }
}

impl History {
    pub fn new(budget: usize) -> History {
        History { budget, items: VecDeque::new() }
    }

    /// Keep `picture`; returns false (and keeps nothing new) when it alone is over the budget or
    /// its pixels do not match its size.
    pub fn push(&mut self, picture: Picture) -> bool {
        let expect = (picture.size[0] as usize).saturating_mul(picture.size[1] as usize).saturating_mul(4);
        if picture.bytes() > self.budget || picture.pixels.len() != expect || expect == 0 {
            return false;
        }
        self.items.push_back(picture);
        while self.bytes() > self.budget && self.items.len() > 1 {
            self.items.pop_front();
        }
        true
    }

    pub fn bytes(&self) -> usize {
        self.items.iter().map(Picture::bytes).sum()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, i: usize) -> Option<&Picture> {
        self.items.get(i)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Picture> {
        self.items.iter()
    }

    pub fn remove(&mut self, i: usize) -> Option<Picture> {
        self.items.remove(i)
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

/// Number of bins in a histogram.
pub const BINS: usize = 64;

/// How many pixels of a picture have each brightness, per colour channel and overall.
#[derive(Clone, Debug, PartialEq)]
pub struct Histogram {
    pub red: [u32; BINS],
    pub green: [u32; BINS],
    pub blue: [u32; BINS],
    /// Brightness (Rec. 709 weights on the encoded values).
    pub luma: [u32; BINS],
    pub pixels: u32,
}

impl Histogram {
    /// Count the pixels of sRGB RGBA bytes.
    pub fn of(rgba: &[u8]) -> Histogram {
        let mut h = Histogram { red: [0; BINS], green: [0; BINS], blue: [0; BINS], luma: [0; BINS], pixels: 0 };
        let bin = |v: u8| (v as usize * BINS / 256).min(BINS - 1);
        for p in rgba.as_chunks::<4>().0 {
            h.red[bin(p[0])] += 1;
            h.green[bin(p[1])] += 1;
            h.blue[bin(p[2])] += 1;
            let l = (0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])).round() as u8;
            h.luma[bin(l)] += 1;
            h.pixels = h.pixels.saturating_add(1);
        }
        h
    }

    /// The tallest bin of any channel, for scaling a drawing.
    pub fn peak(&self) -> u32 {
        [&self.red, &self.green, &self.blue].into_iter().flat_map(|c| c.iter().copied()).max().unwrap_or(0)
    }
}

/// How two pictures of the same size differ.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Difference {
    /// Mean absolute difference of the colour channels, 0 to 1.
    pub mean: f32,
    /// Largest difference in any channel, 0 to 1.
    pub max: f32,
    /// Share of pixels that differ by more than one level.
    pub changed: f32,
}

/// Compare two pictures; `None` when their sizes differ.
pub fn difference(a: &Picture, b: &Picture) -> Option<Difference> {
    if a.size != b.size || a.pixels.len() != b.pixels.len() || a.pixels.is_empty() {
        return None;
    }
    let (mut sum, mut max, mut changed) = (0u64, 0u8, 0usize);
    for (p, q) in a.pixels.as_chunks::<4>().0.iter().zip(b.pixels.as_chunks::<4>().0) {
        let d = (0..3).map(|k| p[k].abs_diff(q[k])).collect::<Vec<u8>>();
        sum += d.iter().map(|v| u64::from(*v)).sum::<u64>();
        max = max.max(d.iter().copied().max().unwrap_or(0));
        if d.iter().any(|v| *v > 1) {
            changed += 1;
        }
    }
    let n = (a.pixels.len() / 4) as f64;
    Some(Difference { mean: (sum as f64 / (3.0 * 255.0 * n)) as f32, max: f32::from(max) / 255.0, changed: (changed as f64 / n) as f32 })
}

/// The two pictures side by side in one: columns left of `at` (0..1 across) from `a`, the rest
/// from `b`. `None` when their sizes differ.
pub fn split(a: &Picture, b: &Picture, at: f32) -> Option<Vec<u8>> {
    if a.size != b.size || a.pixels.len() != b.pixels.len() {
        return None;
    }
    let w = a.size[0] as usize;
    let cut = ((at.clamp(0.0, 1.0) * w as f32).round() as usize).min(w);
    let mut out = Vec::with_capacity(a.pixels.len());
    for (row_a, row_b) in a.pixels.chunks_exact(w * 4).zip(b.pixels.chunks_exact(w * 4)) {
        out.extend_from_slice(&row_a[..cut * 4]);
        out.extend_from_slice(&row_b[cut * 4..]);
    }
    Some(out)
}

/// The absolute difference of two pictures, multiplied by `gain` so small differences show.
pub fn difference_image(a: &Picture, b: &Picture, gain: f32) -> Option<Vec<u8>> {
    if a.size != b.size || a.pixels.len() != b.pixels.len() {
        return None;
    }
    let gain = if gain.is_finite() { gain.clamp(0.0, 1000.0) } else { 1.0 };
    let mut out = Vec::with_capacity(a.pixels.len());
    for (p, q) in a.pixels.as_chunks::<4>().0.iter().zip(b.pixels.as_chunks::<4>().0) {
        for k in 0..3 {
            out.push((f32::from(p[k].abs_diff(q[k])) * gain).min(255.0) as u8);
        }
        out.push(255);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(label: &str, w: u32, h: u32, rgb: [u8; 3]) -> Picture {
        Picture { label: label.into(), size: [w, h], pixels: (0..w * h).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect(), samples: 4, seconds: 0.1 }
    }

    fn film(w: u32, h: u32) -> Film {
        let mut f = Film::new(w, h).unwrap();
        f.samples = 2;
        for (i, p) in f.sum.iter_mut().enumerate() {
            *p = [i as f32, 0.5, 1.0];
        }
        f
    }

    #[test]
    fn a_region_cuts_whole_pixels_and_never_nothing() {
        assert_eq!(crop_rect(200, 100, None), (0, 0, 200, 100));
        assert_eq!(crop_rect(200, 100, Some([0.25, 0.0, 0.75, 0.5])), (50, 0, 100, 50));
        assert_eq!(crop_rect(10, 10, Some([0.99, 0.99, 1.0, 1.0])), (9, 9, 1, 1));
        assert_eq!(crop_rect(10, 10, Some([0.5, 0.5, 0.5, 0.5])), (5, 5, 1, 1), "an empty region keeps one pixel");
        assert_eq!(crop_rect(10, 10, Some([f32::NAN, 0.0, 1.0, 1.0])), (0, 0, 10, 10));
    }

    #[test]
    fn frames_are_numbered_before_the_extension_or_in_the_hashes() {
        let n = |b: &str, f: i32, fmt| numbered(Path::new(b), f, fmt).display().to_string();
        assert_eq!(n("/out/shot.png", 7, OutputFormat::Png), "/out/shot_0007.png");
        assert_eq!(n("/out/shot_##.png", 7, OutputFormat::Png), "/out/shot_07.png");
        assert_eq!(n("/out/a####b.png", 12345, OutputFormat::Exr), "/out/a12345b.exr", "a frame wider than the hashes is not cut");
        assert_eq!(n("shot.png", -3, OutputFormat::Png), "shot_-003.png");
        assert_eq!(n("/out/shot.jpg", 1, OutputFormat::Exr), "/out/shot_0001.exr");
    }

    #[test]
    fn the_history_holds_to_its_memory_budget_and_drops_the_oldest() {
        let one = picture("a", 8, 8, [10, 20, 30]);
        let each = one.bytes();
        let mut h = History::new(each * 2 + each / 2);
        assert!(h.push(one));
        assert!(h.push(picture("b", 8, 8, [1, 1, 1])));
        assert!(h.push(picture("c", 8, 8, [2, 2, 2])));
        assert_eq!(h.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), vec!["b", "c"], "the oldest went");
        assert!(h.bytes() <= each * 2 + each / 2);
        // A picture over the whole budget is refused and the rest stays; a lying size is refused.
        assert!(!h.push(picture("big", 64, 64, [0; 3])));
        assert!(!h.push(Picture { size: [9, 9], ..picture("lie", 8, 8, [0; 3]) }));
        assert_eq!(h.len(), 2);
        h.remove(0);
        assert_eq!(h.get(0).map(|p| p.label.as_str()), Some("c"));
        h.clear();
        assert!(h.is_empty() && h.bytes() == 0);
    }

    #[test]
    fn the_histogram_counts_every_pixel_once_per_channel() {
        let mut p = picture("x", 4, 4, [0, 128, 255]);
        p.pixels[0..4].copy_from_slice(&[255, 255, 255, 255]);
        let h = Histogram::of(&p.pixels);
        assert_eq!(h.pixels, 16);
        for c in [&h.red, &h.green, &h.blue, &h.luma] {
            assert_eq!(c.iter().sum::<u32>(), 16);
        }
        assert_eq!((h.blue[BINS - 1], h.red[0], h.red[BINS - 1], h.green[BINS / 2]), (16, 15, 1, 15));
        assert_eq!(h.peak(), 16);
        assert_eq!(Histogram::of(&[]).peak(), 0);
    }

    #[test]
    fn two_pictures_compare_split_and_difference() {
        let a = picture("a", 4, 2, [100, 100, 100]);
        let b = picture("b", 4, 2, [110, 100, 100]);
        let d = difference(&a, &b).unwrap();
        assert!((d.mean - 10.0 / 765.0).abs() < 1e-6 && (d.max - 10.0 / 255.0).abs() < 1e-6 && d.changed == 1.0, "{d:?}");
        assert_eq!(difference(&a, &a).unwrap(), Difference { mean: 0.0, max: 0.0, changed: 0.0 });
        assert!(difference(&a, &picture("c", 2, 2, [0; 3])).is_none());
        // Half and half: the left columns are A's, the right B's.
        let s = split(&a, &b, 0.5).unwrap();
        assert_eq!((s[0], s[4 * 4 - 4], s.len()), (100, 110, a.pixels.len()));
        assert_eq!(split(&a, &b, 5.0).unwrap(), a.pixels, "past the end it is all A");
        let img = difference_image(&a, &b, 10.0).unwrap();
        assert_eq!((img[0], img[1], img[3]), (100, 0, 255));
        assert_eq!(difference_image(&a, &b, f32::NAN).unwrap()[0], 10);
    }

    #[test]
    fn pictures_are_written_cut_and_with_the_chosen_passes() {
        let dir = std::env::temp_dir().join(format!("mc-output-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = film(8, 4);
        let png = dir.join("a.png");
        write_png(&png, &f, None, 0.0, Some([0.0, 0.0, 0.5, 1.0])).unwrap();
        let back = craft_image_codecs::decode(&std::fs::read(&png).unwrap()).unwrap();
        assert_eq!((back.width(), back.height()), (4, 4));
        // The EXR holds exactly the layers asked for.
        let names = |passes: &[Pass]| {
            let path = dir.join("a.exr");
            write_exr(&path, &f, None, passes, None).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            let part = craft_exr::decode::decode_part(&bytes, 0, &|_| true, &craft_exr::decode::Limits::default()).unwrap();
            let mut n: Vec<String> = part.channels.iter().map(|c| c.name.clone()).collect();
            n.sort();
            n
        };
        assert_eq!(names(&[]), vec!["B", "G", "R"]);
        assert_eq!(names(&[Pass::Depth, Pass::Variance]), vec!["B", "G", "R", "Z", "variance"]);
        assert_eq!(names(&Pass::ALL).len(), 3 + 3 + 3 + 1 + 2 + 1);
        // By the project's settings, to the format it names.
        let cfg = RenderConfig { format: OutputFormat::Exr, passes: vec![Pass::Normal], region: Some([0.0, 0.0, 0.5, 0.5]), ..Default::default() };
        write_picture(&dir.join("b.exr"), &f, None, &cfg).unwrap();
        assert!(std::fs::metadata(dir.join("b.exr")).unwrap().len() > 0);
        std::fs::remove_dir_all(&dir).ok();
    }
}
