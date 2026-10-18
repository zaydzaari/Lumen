//! Album art: smallest suitable image downloaded, downscaled on a worker thread
//! *before* GPU upload, kept in a hard-capped in-memory LRU of textures and a
//! size-capped disk cache of the (small) original files.

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::Arc;

use egui::{ColorImage, TextureHandle, TextureOptions};
use lru::LruCache;
use tokio::sync::Semaphore;

use crate::cache::DiskCache;
use crate::events::{AppEvent, UiTx};

/// Thumbnails are decoded to this size (displayed at 32–56 px; 2x for HiDPI).
pub const THUMB_PX: u32 = 96;
/// Now-playing / header art.
pub const LARGE_PX: u32 = 256;

/// Large textures (≥ 192 px) get their own, smaller budget: 64 × 256² RGBA ≈ 16 MB.
const LARGE_CAP: usize = 64;

pub struct ArtCache {
    /// Thumbnails (96 px): cheap, keep many.
    textures: LruCache<String, TextureHandle>,
    /// Big covers (cards, headers, now playing).
    large: LruCache<String, TextureHandle>,
    /// Dominant colour per image URL (for gradients / lyrics background).
    colors: LruCache<String, egui::Color32>,
    pending: HashSet<String>,
    failed: LruCache<String, ()>,
    disk: Arc<DiskCache>,
    http: reqwest::Client,
    rt: tokio::runtime::Handle,
    permits: Arc<Semaphore>,
    /// Big images (screenshots, full-size covers) are fetched and decoded one
    /// at a time: decoding a 1080p PNG briefly needs ~10 MB.
    big: Arc<Semaphore>,
    ui: UiTx,
}

fn key(url: &str, px: u32) -> String {
    format!("{px}|{url}")
}

impl ArtCache {
    pub fn new(
        cap: usize,
        disk: Arc<DiskCache>,
        http: reqwest::Client,
        rt: tokio::runtime::Handle,
        ui: UiTx,
    ) -> Self {
        let cap = NonZeroUsize::new(cap.max(1)).unwrap_or(NonZeroUsize::MIN);
        Self {
            textures: LruCache::new(cap),
            large: LruCache::new(NonZeroUsize::new(LARGE_CAP).unwrap_or(NonZeroUsize::MIN)),
            colors: LruCache::new(NonZeroUsize::new(512).unwrap_or(NonZeroUsize::MIN)),
            pending: HashSet::new(),
            failed: LruCache::new(NonZeroUsize::new(256).unwrap_or(NonZeroUsize::MIN)),
            disk,
            http,
            rt,
            permits: Arc::new(Semaphore::new(6)),
            big: Arc::new(Semaphore::new(1)),
            ui,
        }
    }

    pub fn len(&self) -> usize {
        self.textures.len() + self.large.len()
    }

    pub fn cap(&self) -> usize {
        self.textures.cap().get() + self.large.cap().get()
    }

    pub fn disk(&self) -> &Arc<DiskCache> {
        &self.disk
    }

    /// Texture for `url` at `px`, starting a background load if needed.
    pub fn get(&mut self, url: &str, px: u32) -> Option<egui::TextureId> {
        self.get_sized(url, px).map(|(t, _)| t)
    }

    /// Like [`Self::get`], also returning the texture's pixel size (for
    /// non-square images such as theme screenshots).
    pub fn get_sized(&mut self, url: &str, px: u32) -> Option<(egui::TextureId, egui::Vec2)> {
        let k = key(url, px);
        let hit = if px > THUMB_PX {
            self.large.get(&k)
        } else {
            self.textures.get(&k)
        };
        if let Some(t) = hit {
            return Some((t.id(), t.size_vec2()));
        }
        if self.pending.contains(&k) || self.failed.contains(&k) || self.pending.len() > 64 {
            return None;
        }
        self.pending.insert(k.clone());
        let permits = if px > LARGE_PX {
            self.big.clone()
        } else {
            self.permits.clone()
        };
        let (disk, http, ui, url) = (
            self.disk.clone(),
            self.http.clone(),
            self.ui.clone(),
            url.to_string(),
        );
        self.rt.spawn(async move {
            let _permit = permits.acquire().await;
            let bytes = match if crate::local::path(&url).is_some() {
                None
            } else {
                disk.get(&url, "img")
            } {
                Some(b) => Some(b),
                None => match if let Some(path) = crate::local::path(&url) {
                    let path = path.to_path_buf();
                    tokio::task::spawn_blocking(move || crate::local::cover(&path))
                        .await
                        .unwrap_or_else(|e| Err(e.into()))
                } else {
                    download(&http, &url).await
                } {
                    Ok(b) => {
                        if let Err(e) = if crate::local::path(&url).is_none() {
                            disk.put(&url, "img", &b)
                        } else {
                            Ok(())
                        } {
                            tracing::debug!("art cache write: {e:#}");
                        }
                        Some(b)
                    }
                    Err(e) => {
                        tracing::debug!("art download {url}: {e:#}");
                        None
                    }
                },
            };
            let img = match bytes {
                Some(b) => tokio::task::spawn_blocking(move || {
                    let big = b.len() > 1 << 20;
                    let img = decode(&b, px);
                    drop(b);
                    if big {
                        // Hand the decode buffers back to the OS right away.
                        crate::memory::release_heap();
                    }
                    img
                })
                .await
                .ok()
                .flatten(),
                None => None,
            };
            ui.send(AppEvent::Image(key(&url, px), img));
        });
        None
    }

    /// Called on the UI thread when a decoded image arrives.
    pub fn insert(&mut self, ctx: &egui::Context, k: String, img: Option<ColorImage>) {
        self.pending.remove(&k);
        match img {
            Some(img) => {
                if let Some((_, url)) = k.split_once('|') {
                    if !self.colors.contains(url) {
                        self.colors.put(url.to_owned(), dominant_color(&img));
                    }
                }
                let large = img.size[0].max(img.size[1]) > THUMB_PX as usize;
                let tex = ctx.load_texture(&k, img, TextureOptions::LINEAR);
                // Remember when it arrived so it can fade in (like Spotify's covers).
                ctx.data_mut(|d| d.insert_temp(fade_id(tex.id()), std::time::Instant::now()));
                // Evicted handles are dropped here, freeing the GPU texture.
                if large {
                    self.large.put(k, tex);
                } else {
                    self.textures.put(k, tex);
                }
            }
            None => {
                self.failed.put(k, ());
            }
        }
    }

    /// Dominant colour of an already-loaded image (any size).
    pub fn color(&mut self, url: &str) -> Option<egui::Color32> {
        self.colors.get(url).copied()
    }

    pub fn clear_memory(&mut self) {
        self.textures.clear();
        self.large.clear();
        self.failed.clear();
    }
}

/// A vivid but dark-enough colour representing the artwork: saturation-weighted
/// average, then scaled so the brightest channel sits around 170/255.
pub fn dominant_color(img: &ColorImage) -> egui::Color32 {
    let (mut r, mut g, mut b, mut wsum) = (0f64, 0f64, 0f64, 0f64);
    let step = (img.pixels.len() / 4096).max(1);
    for px in img.pixels.iter().step_by(step) {
        let [pr, pg, pb, _] = px.to_srgba_unmultiplied();
        let (mx, mn) = (pr.max(pg).max(pb) as f64, pr.min(pg).min(pb) as f64);
        let sat = if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
        // Ignore near-black/near-white pixels, favour saturated ones.
        let w = if mx < 25.0 || mn > 235.0 {
            0.02
        } else {
            0.15 + sat * sat * 2.0
        };
        r += pr as f64 * w;
        g += pg as f64 * w;
        b += pb as f64 * w;
        wsum += w;
    }
    if wsum <= 0.0 {
        return egui::Color32::from_rgb(80, 80, 80);
    }
    let (r, g, b) = (r / wsum, g / wsum, b / wsum);
    let mx = r.max(g).max(b).max(1.0);
    let k = 170.0 / mx;
    let c = |v: f64| (v * k).clamp(0.0, 255.0) as u8;
    egui::Color32::from_rgb(c(r), c(g), c(b))
}

async fn download(http: &reqwest::Client, url: &str) -> anyhow::Result<Vec<u8>> {
    // `file://` covers are used by the `--demo` preview.
    if let Some(path) = url.strip_prefix("file://") {
        let path = path.to_owned();
        return Ok(tokio::task::spawn_blocking(move || std::fs::read(path)).await??);
    }
    let resp = http
        .get(url)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await?
        .error_for_status()?;
    let bytes = resp.bytes().await?;
    if bytes.len() > 8 * 1024 * 1024 {
        anyhow::bail!("image too large");
    }
    Ok(bytes.to_vec())
}

pub fn fade_id(t: egui::TextureId) -> egui::Id {
    egui::Id::new(("tex-fade", t))
}

/// 0→1 over 250 ms after the texture arrived (1 for anything older).
pub fn fade_alpha(ctx: &egui::Context, t: egui::TextureId) -> f32 {
    let Some(born) = ctx.data(|d| d.get_temp::<std::time::Instant>(fade_id(t))) else {
        return 1.0;
    };
    let a = born.elapsed().as_secs_f32() / 0.25;
    if a >= 1.0 {
        ctx.data_mut(|d| d.remove::<std::time::Instant>(fade_id(t)));
        1.0
    } else {
        ctx.request_repaint();
        a
    }
}

/// Decode and downscale to at most `px`×`px` before it ever reaches the GPU.
pub fn decode(bytes: &[u8], px: u32) -> Option<ColorImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width() > px || img.height() > px {
        img.thumbnail(px, px)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_downscales() {
        let mut buf = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(640, 640, image::Rgb([200, 10, 10]))
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let img = decode(buf.get_ref(), THUMB_PX).unwrap();
        assert_eq!(img.size, [THUMB_PX as usize, THUMB_PX as usize]);
        assert!(decode(b"not an image", 64).is_none());
    }

    #[test]
    fn dominant_color_prefers_saturated_pixels() {
        let mut img = ColorImage::filled([10, 10], egui::Color32::from_rgb(20, 20, 20));
        for p in img.pixels.iter_mut().take(30) {
            *p = egui::Color32::from_rgb(200, 30, 30);
        }
        let c = dominant_color(&img);
        assert!(c.r() > c.g() * 2 && c.r() > c.b() * 2, "{c:?}");
        assert!(c.r() >= 160);
    }

    #[test]
    fn lru_cap_is_enforced() {
        let mut lru: LruCache<u32, ()> = LruCache::new(NonZeroUsize::new(150).unwrap());
        for i in 0..1000 {
            lru.put(i, ());
        }
        assert_eq!(lru.len(), 150);
        assert!(lru.contains(&999) && !lru.contains(&0));
    }
}
