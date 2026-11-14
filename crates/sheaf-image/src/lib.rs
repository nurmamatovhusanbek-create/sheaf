//! MOD-IMAGE: decode, deskew, binarize, blank detect, encode.
//!
//! Pure image functions; the only I/O is reading the file handed to
//! [`decode_file`]. See `ARCHITECTURE.md`, anchor `mod-image`.
//!
//! Multipage TIFF (only the first page is decoded today) and CCITT G4
//! output arrive with TOOL-IMG2PDF in PH-3.

mod analyze;
mod codec;
mod transform;

pub use analyze::{binarize_sauvola, estimate_skew, ink_coverage, is_blank, otsu_threshold};
pub use codec::{
    check_dimensions, decode_bytes, decode_file, dimensions, encode_jpeg, encode_png, encode_ppm,
};
pub use transform::{
    apply_filter, crop_norm, despeckle, downsample, fit_within, rotate_quarter, rotate_small, white,
};

pub use image::{DynamicImage, GrayImage, RgbaImage};

/// Default sensitivity for [`is_blank`]: the fraction of the page that may be
/// ink before the page stops counting as blank.
pub const BLANK_SENSITIVITY: f32 = 0.002;
