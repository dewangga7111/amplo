//! Album art: read from file tags on a worker thread, with drawn placeholders for the rest.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureId, TextureOptions};
use image::imageops::FilterType;
use lofty::{
    file::TaggedFileExt,
    picture::PictureType,
    probe::Probe,
};

const MAX_SIZE: u32 = 300;
/// The cover of the playing track in the full layout. Larger art is scaled down to this.
const LARGE_MAX_SIZE: u32 = 1200;
const PLACEHOLDER_SIZE: usize = 128;
/// More textures than this means many playlists were opened, so old ones are dropped.
const CACHE_LIMIT: usize = 64;
/// The dark and bright color pairs of the design's sample covers.
const PALETTE: [(u32, u32); 8] = [
    (0x2B1E5C, 0xFF7AB6),
    (0x14324A, 0x5AD1F2),
    (0x3A1630, 0xFFB86B),
    (0x1E2A1A, 0xC8F25A),
    (0x163838, 0xF2D16B),
    (0x3A2418, 0xF28F5A),
    (0x1C2430, 0xE8E3D3),
    (0x2A0F14, 0xFF4D5E),
];

struct Request {
    path: PathBuf,
    large: bool,
}

struct Loaded {
    path: PathBuf,
    large: bool,
    image: Option<ColorImage>,
}

enum Slot {
    Loading,
    Missing,
    Ready(TextureHandle),
}

/// The large cover of one track, for the full layout.
pub enum LargeCover {
    Loading,
    Missing,
    /// The texture and its size in pixels.
    Ready(TextureId, egui::Vec2),
}

pub struct Covers {
    requests: Sender<Request>,
    results: Receiver<Loaded>,
    /// By path, so covers stay loaded when a rescan gives tracks new positions.
    slots: HashMap<PathBuf, Slot>,
    /// Only one at a time, because only the playing track shows its cover this large.
    large: Option<(PathBuf, Slot)>,
    placeholders: [Option<TextureHandle>; PALETTE.len()],
}

/// Newest request first, so the playing track does not wait behind a list of playlists.
fn run_worker(requests: &Receiver<Request>, results: &Sender<Loaded>, ctx: &egui::Context) {
    let mut pending = Vec::new();
    loop {
        if pending.is_empty() {
            match requests.recv() {
                Ok(request) => pending.push(request),
                Err(_) => return,
            }
        }
        pending.extend(requests.try_iter());
        let Some(Request { path, large }) = pending.pop() else { continue };
        let max_size = if large { LARGE_MAX_SIZE } else { MAX_SIZE };
        let image = cover_bytes(&path).and_then(|bytes| decode_cover(&bytes, max_size));
        if results.send(Loaded { path, large, image }).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// The encoded front cover from the file's tags, or else the first picture they have.
pub fn cover_bytes(path: &Path) -> Option<Vec<u8>> {
    let tagged = Probe::open(path).ok()?.guess_file_type().ok()?.read().ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let picture = tag
        .pictures()
        .iter()
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| tag.pictures().first())?;
    Some(picture.data().to_vec())
}

/// Decodes to a square, cropped at the center, no bigger than `max_size` pixels.
fn decode_cover(bytes: &[u8], max_size: u32) -> Option<ColorImage> {
    let image = image::load_from_memory(bytes).ok()?;
    let size = image.width().min(image.height()).min(max_size).max(1);
    let square = image.resize_to_fill(size, size, FilterType::Triangle).into_rgba8();
    Some(ColorImage::from_rgba_unmultiplied([size as usize, size as usize], square.as_raw()))
}

fn color(rgb: u32) -> [f32; 3] {
    [(rgb >> 16 & 0xFF) as f32, (rgb >> 8 & 0xFF) as f32, (rgb & 0xFF) as f32]
}

/// The design's generated cover: a bright disc and thin ring on dark diagonal stripes.
fn placeholder_image(index: usize) -> ColorImage {
    let (dark, bright) = PALETTE[index];
    let (dark, bright) = (color(dark), color(bright));
    let center_x = (28 + (index * 17) % 44) as f32 / 100.0;
    let center_y = (30 + (index * 23) % 38) as f32 / 100.0;
    let reach = center_x.max(1.0 - center_x).hypot(center_y.max(1.0 - center_y));
    let scale = PLACEHOLDER_SIZE as f32;

    let pixels = (0..PLACEHOLDER_SIZE * PLACEHOLDER_SIZE)
        .map(|pixel| {
            let x = ((pixel % PLACEHOLDER_SIZE) as f32 + 0.5) / scale;
            let y = ((pixel / PLACEHOLDER_SIZE) as f32 + 0.5) / scale;
            let distance = (x - center_x).hypot(y - center_y) / reach;
            let soft = 1.0 / (scale * reach);
            let inside = |edge: f32| ((edge - distance) / soft + 0.5).clamp(0.0, 1.0);

            // CSS draws stripes 6 of every 14 pixels on a 150 pixel cover.
            let stripe = ((x + y) * 150.0 / std::f32::consts::SQRT_2).rem_euclid(14.0) < 6.0;
            let ring = inside(0.36) * (1.0 - inside(0.345));
            let white = if stripe { 0.05 } else { 0.0 } + ring * 0.10;
            let disc = inside(0.23);
            let channel = |c: usize| {
                let base = dark[c] + (255.0 - dark[c]) * white;
                (base + (bright[c] - base) * disc) as u8
            };
            Color32::from_rgb(channel(0), channel(1), channel(2))
        })
        .collect();
    ColorImage::new([PLACEHOLDER_SIZE, PLACEHOLDER_SIZE], pixels)
}

/// Picks a stable placeholder for a file, from its bytes instead of its position in a list.
fn placeholder_index(path: &Path) -> usize {
    let hash = path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3));
    (hash % PALETTE.len() as u64) as usize
}

impl Covers {
    pub fn new(ctx: &egui::Context) -> Self {
        let (requests, worker_requests) = mpsc::channel();
        let (worker_results, results) = mpsc::channel();
        let worker_ctx = ctx.clone();
        thread::spawn(move || run_worker(&worker_requests, &worker_results, &worker_ctx));
        Self {
            requests,
            results,
            slots: HashMap::new(),
            large: None,
            placeholders: Default::default(),
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        for loaded in self.results.try_iter() {
            let kind = if loaded.large { "large-cover" } else { "cover" };
            let slot = match loaded.image {
                Some(image) => {
                    let name = format!("{kind}-{}", loaded.path.display());
                    Slot::Ready(ctx.load_texture(name, image, TextureOptions::LINEAR))
                }
                None => Slot::Missing,
            };
            if !loaded.large {
                self.slots.insert(loaded.path, slot);
            } else if let Some((path, large)) = &mut self.large
                && *path == loaded.path
            {
                *large = slot;
            }
        }
    }

    /// The cover of a track at its own resolution, up to a limit. Showing it no larger
    /// than that keeps it sharp.
    pub fn large(&mut self, path: &Path) -> LargeCover {
        if self.large.as_ref().is_none_or(|(large, _)| large != path) {
            // The worker only stops when this struct is gone.
            let _ = self.requests.send(Request { path: path.to_owned(), large: true });
            self.large = Some((path.to_owned(), Slot::Loading));
        }
        match self.large.as_ref().map(|(_, slot)| slot) {
            Some(Slot::Ready(texture)) => LargeCover::Ready(texture.id(), texture.size_vec2()),
            Some(Slot::Missing) => LargeCover::Missing,
            _ => LargeCover::Loading,
        }
    }

    /// Returns the cover of a track. Until it has loaded, and when it has none, that is a
    /// placeholder.
    pub fn texture(&mut self, ctx: &egui::Context, path: &Path) -> TextureId {
        if !self.slots.contains_key(path) {
            if self.slots.len() > CACHE_LIMIT {
                self.slots.retain(|_, slot| matches!(slot, Slot::Loading));
            }
            // The worker only stops when this struct is gone.
            let _ = self.requests.send(Request { path: path.to_owned(), large: false });
            self.slots.insert(path.to_owned(), Slot::Loading);
        }
        if let Some(Slot::Ready(texture)) = self.slots.get(path) {
            return texture.id();
        }

        let index = placeholder_index(path);
        self.placeholders[index]
            .get_or_insert_with(|| {
                let name = format!("placeholder-{index}");
                ctx.load_texture(name, placeholder_image(index), TextureOptions::LINEAR)
            })
            .id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = RgbaImage::from_pixel(width, height, Rgba([200, 10, 20, 255]));
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image).write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn crops_covers_to_squares_and_limits_their_size() {
        assert_eq!(decode_cover(&png(900, 600), MAX_SIZE).unwrap().size, [300, 300]);
        assert_eq!(decode_cover(&png(900, 600), LARGE_MAX_SIZE).unwrap().size, [600, 600]);
        let small = decode_cover(&png(80, 120), MAX_SIZE).unwrap();
        assert_eq!(small.size, [80, 80]);
        assert_eq!(small.pixels[0], Color32::from_rgb(200, 10, 20));
    }

    #[test]
    fn broken_picture_data_gives_no_cover() {
        assert!(decode_cover(b"not an image", MAX_SIZE).is_none());
    }

    #[test]
    fn placeholder_shows_the_disc_and_the_dark_background() {
        let image = placeholder_image(0);
        let (dark, bright) = (PALETTE[0].0, PALETTE[0].1);
        let center = (30 * PLACEHOLDER_SIZE / 100) * PLACEHOLDER_SIZE + 28 * PLACEHOLDER_SIZE / 100;
        let [r, g, b, _] = image.pixels[center].to_array();
        assert_eq!(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b), bright);
        let corner = image.pixels[PLACEHOLDER_SIZE * PLACEHOLDER_SIZE - 1];
        assert!(u32::from(corner.r()) <= (dark >> 16) + 16, "corner was {corner:?}");
    }
}
