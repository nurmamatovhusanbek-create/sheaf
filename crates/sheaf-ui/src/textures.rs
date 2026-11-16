//! Page pictures as GPU textures, loaded off the UI thread (PERF-LAZY).
//!
//! A picture is requested by page, version and size the first time a row or
//! the preview wants it; it arrives as a message and becomes a texture. The
//! version changes when the page's edits do, so an edit refreshes that page
//! and nothing else.

use crate::app::{Msg, SheafApp};
use eframe::egui::{self, ColorImage, TextureHandle, TextureId, TextureOptions};
use std::collections::HashMap;

/// Page id, version fingerprint, longest side in pixels.
pub type TexKey = (u64, String, u32);

enum Slot {
    Loading,
    Ready {
        tex: TextureHandle,
        size: [usize; 2],
        used: u64,
    },
    Failed,
}

#[derive(Default)]
pub struct Textures {
    map: HashMap<TexKey, Slot>,
    frame: u64,
    loading: usize,
}

/// Most pictures decoded at once.
const MAX_LOADING: usize = 6;
/// Most textures kept; the least recently drawn go first.
const MAX_KEPT: usize = 320;

impl Textures {
    pub fn new_frame(&mut self) {
        self.frame += 1;
    }

    /// A picture came back from a helper thread.
    pub fn arrived(&mut self, ctx: &egui::Context, key: TexKey, image: Option<ColorImage>) {
        self.loading = self.loading.saturating_sub(1);
        let slot = match image {
            Some(img) => {
                let size = img.size;
                let tex = ctx.load_texture(format!("page-{}-{}", key.0, key.2), img, TextureOptions::LINEAR);
                Slot::Ready {
                    tex,
                    size,
                    used: self.frame,
                }
            }
            None => Slot::Failed,
        };
        self.map.insert(key, slot);
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
            Some(Slot::Ready { tex, size, used }) => {
                *used = frame;
                Lookup::Ready(tex.id(), *size)
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
    Loading,
    Failed,
}

impl SheafApp {
    /// The picture of `page` at longest side `px`, starting its load if needed.
    pub(crate) fn picture(&mut self, page: u64, ver: &str, px: u32) -> Pic {
        let key: TexKey = (page, ver.to_string(), px);
        match self.tex.lookup(&key) {
            Lookup::Ready(id, size) => Pic::Ready(id, size),
            Lookup::Pending => Pic::Loading,
            Lookup::Failed => Pic::Failed,
            Lookup::Missing => {
                if self.tex.loading >= MAX_LOADING {
                    self.ctx.request_repaint_after(std::time::Duration::from_millis(40));
                    return Pic::Loading;
                }
                self.tex.loading += 1;
                self.tex.map.insert(key.clone(), Slot::Loading);
                self.spawn(move |b| {
                    let image = b
                        .thumb(key.0, key.2)
                        .ok()
                        .and_then(|png| image::load_from_memory(&png).ok())
                        .map(|img| {
                            let rgba = img.to_rgba8();
                            ColorImage::from_rgba_unmultiplied(
                                [rgba.width() as usize, rgba.height() as usize],
                                rgba.as_raw(),
                            )
                        });
                    Msg::Thumb { key, image }
                });
                Pic::Loading
            }
        }
    }
}
