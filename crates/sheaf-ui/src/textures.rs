//! Page pictures as GPU textures, loaded off the UI thread (PERF-LAZY).
//!
//! A picture is requested by page, version and size the first time a row or
//! the preview wants it; it arrives as a message and becomes a texture. The
//! version changes when the page's edits do, so an edit refreshes that page
//! and nothing else.

use crate::app::{Msg, SheafApp};
use eframe::egui::{self, ColorImage, TextureHandle, TextureId, TextureOptions};
use sheaf_core::proto::FilterNote;
use std::collections::HashMap;

/// Page id, version fingerprint, longest side in pixels.
pub type TexKey = (u64, String, u32);

enum Slot {
    Loading,
    Ready {
        tex: TextureHandle,
        size: [usize; 2],
        used: u64,
        /// What the page's filters did to this picture, when any was asked for.
        note: Option<FilterNote>,
    },
    Failed,
}

#[derive(Default)]
pub struct Textures {
    map: HashMap<TexKey, Slot>,
    frame: u64,
    loading: usize,
    /// The last picture shown for each page (thumbnail or preview), kept while the next one loads, so a
    /// page whose version changed (a filter, a rotation) never turns into an empty placeholder for the
    /// moment it takes to draw it again.
    last_good: HashMap<(u64, bool), (TextureHandle, [usize; 2], u64)>,
}

/// Pictures this size or smaller are thumbnails (the preview asks for more).
const THUMB_MAX: u32 = 256;

/// Most pictures decoded at once.
const MAX_LOADING: usize = 6;
/// Most textures kept; the least recently drawn go first.
const MAX_KEPT: usize = 320;

impl Textures {
    pub fn new_frame(&mut self) {
        self.frame += 1;
    }

    /// A picture came back from a helper thread.
    pub fn arrived(&mut self, ctx: &egui::Context, key: TexKey, image: Option<ColorImage>, note: Option<FilterNote>) {
        self.loading = self.loading.saturating_sub(1);
        let slot = match image {
            Some(img) => {
                let size = img.size;
                let tex = ctx.load_texture(format!("page-{}-{}", key.0, key.2), img, TextureOptions::LINEAR);
                Slot::Ready {
                    tex,
                    size,
                    used: self.frame,
                    note,
                }
            }
            None => Slot::Failed,
        };
        self.map.insert(key, slot);
        if self.last_good.len() > MAX_KEPT {
            let frame = self.frame;
            self.last_good.retain(|_, (_, _, used)| *used + 2 >= frame);
        }
        if self.map.len() > MAX_KEPT {
            let frame = self.frame;
            let mut ready: Vec<(u64, TexKey)> = self
                .map
                .iter()
                .filter_map(|(k, s)| match s {
                    Slot::Ready { used, .. } if *used + 2 < frame => Some((*used, k.clone())),
                    _ => None,
                })
                .collect();
            ready.sort();
            for (_, k) in ready.into_iter().take(self.map.len() - MAX_KEPT) {
                self.map.remove(&k);
            }
        }
        ctx.request_repaint();
    }

    fn lookup(&mut self, key: &TexKey) -> Lookup {
        let frame = self.frame;
        match self.map.get_mut(key) {
            Some(Slot::Ready { tex, size, used, .. }) => {
                *used = frame;
                let (id, size) = (tex.id(), *size);
                self.last_good
                    .insert((key.0, key.2 > THUMB_MAX), (tex.clone(), size, frame));
                Lookup::Ready(id, size)
            }
            Some(Slot::Loading) => Lookup::Pending,
            Some(Slot::Failed) => Lookup::Failed,
            None => Lookup::Missing,
        }
    }
}

enum Lookup {
    Ready(TextureId, [usize; 2]),
    Pending,
    Failed,
    Missing,
}

/// What a caller gets for a picture that may not be there yet.
pub enum Pic {
    Ready(TextureId, [usize; 2]),
    /// The new picture is not here yet; this is the page as it was a moment ago.
    Stale(TextureId, [usize; 2]),
    Loading,
    Failed,
}

impl Textures {
    /// What the page's filters did to the picture with this key, once it has arrived.
    pub fn note(&self, key: &TexKey) -> Option<FilterNote> {
        match self.map.get(key) {
            Some(Slot::Ready { note, .. }) => *note,
            _ => None,
        }
    }

    /// The page as it was last drawn, for the moment its new picture is on the way.
    fn stale(&self, page: u64, px: u32) -> Option<(TextureId, [usize; 2])> {
        self.last_good
            .get(&(page, px > THUMB_MAX))
            .map(|(tex, size, _)| (tex.id(), *size))
    }
}

impl SheafApp {
    /// The picture of `page` at longest side `px`, starting its load if needed. While the picture of
    /// a changed page loads, the one before it is returned as [`Pic::Stale`] instead of nothing.
    pub(crate) fn picture(&mut self, page: u64, ver: &str, px: u32) -> Pic {
        let key: TexKey = (page, ver.to_string(), px);
        let waiting = |tex: &Textures| match tex.stale(page, px) {
            Some((id, size)) => Pic::Stale(id, size),
            None => Pic::Loading,
        };
        match self.tex.lookup(&key) {
            Lookup::Ready(id, size) => Pic::Ready(id, size),
            Lookup::Pending => waiting(&self.tex),
            Lookup::Failed => Pic::Failed,
            Lookup::Missing => {
                if self.tex.loading >= MAX_LOADING {
                    self.ctx.request_repaint_after(std::time::Duration::from_millis(40));
                    return waiting(&self.tex);
                }
                self.tex.loading += 1;
                self.tex.map.insert(key.clone(), Slot::Loading);
                self.spawn(move |b| {
                    let (png, note) = match b.thumb_noted(key.0, key.2) {
                        Ok((png, note)) => (Some(png), note),
                        Err(_) => (None, None),
                    };
                    let image = png.and_then(|png| image::load_from_memory(&png).ok()).map(|img| {
                        let rgba = img.to_rgba8();
                        ColorImage::from_rgba_unmultiplied(
                            [rgba.width() as usize, rgba.height() as usize],
                            rgba.as_raw(),
                        )
                    });
                    Msg::Thumb { key, image, note }
                });
                Pic::Loading
            }
        }
    }
}
