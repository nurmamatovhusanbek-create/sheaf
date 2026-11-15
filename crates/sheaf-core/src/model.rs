//! DATA-SOURCE, DATA-PAGE, DATA-FILTER, DATA-OCRSTATE.

use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SourceId(pub u32);

/// Never reused within a workspace, not even after undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PageId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    Pdf,
    Image,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub id: SourceId,
    pub path: PathBuf,
    pub kind: SourceKind,
    pub sha256: [u8; 32],
    pub page_count: u32,
    pub encrypted: bool,
    pub unlocked: bool,
}

/// Clockwise quarter turns added on top of whatever the source page says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Rot {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

impl Rot {
    pub fn degrees(self) -> u16 {
        match self {
            Rot::R0 => 0,
            Rot::R90 => 90,
            Rot::R180 => 180,
            Rot::R270 => 270,
        }
    }
    pub fn from_degrees(deg: i32) -> Result<Rot> {
        match deg.rem_euclid(360) {
            0 => Ok(Rot::R0),
            90 => Ok(Rot::R90),
            180 => Ok(Rot::R180),
            270 => Ok(Rot::R270),
            _ => Err(AppError::invalid("rotation must be a multiple of 90")),
        }
    }
    pub fn plus(self, other: Rot) -> Rot {
        match (self.degrees() + other.degrees()) % 360 {
            0 => Rot::R0,
            90 => Rot::R90,
            180 => Rot::R180,
            _ => Rot::R270,
        }
    }
}

/// Rectangle in fractions of the page, origin top-left, in the page's frame
/// *before* [`Page::rot`] is applied. Rotating a page does not change its crop.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NormRect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl NormRect {
    /// Smallest side a crop may have, as a fraction of the page.
    pub const MIN_SIDE: f32 = 0.01;

    pub fn validate(&self) -> Result<()> {
        let ok = [self.x0, self.y0, self.x1, self.y1].iter().all(|v| v.is_finite())
            && self.x0 >= 0.0
            && self.y0 >= 0.0
            && self.x1 <= 1.0
            && self.y1 <= 1.0
            && self.x1 - self.x0 >= Self::MIN_SIDE
            && self.y1 - self.y0 >= Self::MIN_SIDE;
        if ok {
            Ok(())
        } else {
            Err(AppError::invalid("crop rectangle out of range"))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ColorMode {
    #[default]
    Color,
    Gray,
    BW,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Threshold {
    #[default]
    Auto,
    Fixed(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum Deskew {
    #[default]
    Off,
    Auto,
    /// Degrees; the image is rotated by the negative of this.
    Angle(f32),
}

/// DATA-FILTER. Applied by MOD-IMAGE at render/export time only.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Filter {
    pub mode: ColorMode,
    pub threshold: Threshold,
    pub contrast: i8,
    pub deskew: Deskew,
    pub despeckle: bool,
}

impl Filter {
    /// True when applying the filter leaves pixels untouched.
    pub fn is_identity(&self) -> bool {
        *self == Filter::default()
    }
    pub fn validate(&self) -> Result<()> {
        match self.deskew {
            Deskew::Angle(a) if !a.is_finite() || a.abs() > 45.0 => Err(AppError::invalid("deskew angle out of range")),
            _ => Ok(()),
        }
    }
}

/// DATA-OCRSTATE.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OcrState {
    #[default]
    None,
    Queued,
    Running,
    Done {
        engine: String,
        langs: Vec<String>,
        key: String,
    },
    SkippedHasText,
    Failed {
        reason: String,
    },
}

/// Where a page's pixels come from. The spec sketch has `src` + `src_index`
/// directly on `Page`; a blank page (`Op::InsertBlank`) has no source, so the
/// two are folded into this enum.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PageContent {
    Source {
        src: SourceId,
        index: u32,
    },
    /// Empty page, size in PDF points.
    Blank {
        width_pt: f32,
        height_pt: f32,
    },
}

/// DATA-PAGE: a pointer into a source plus edits (ADR-NONDESTRUCTIVE).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub id: PageId,
    pub content: PageContent,
    pub rot: Rot,
    pub crop: Option<NormRect>,
    pub filter: Filter,
    pub ocr: OcrState,
    pub has_text: Option<bool>,
}

impl Page {
    /// True when the page must be rasterized to honor its edits; false means
    /// it can be copied from its source without re-encoding (PERF-LOSSLESS).
    pub fn has_pixel_edits(&self) -> bool {
        self.crop.is_some() || !self.filter.is_identity()
    }
}
