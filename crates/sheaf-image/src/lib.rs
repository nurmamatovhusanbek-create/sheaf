//! MOD-IMAGE: decode, deskew, binarize, blank detect, encode.
//!
//! Pure image functions; the only I/O is reading the file handed to
//! [`decode_file`]. See `ARCHITECTURE.md`, anchor `mod-image`.
//!
//! Multipage TIFF is supported for 8-bit gray and RGB pages. CCITT G4
//! output for bilevel pages is a later option.

mod analyze;
mod codec;
mod transform;

pub use analyze::{binarize_sauvola, estimate_skew, ink_coverage, is_blank, otsu_threshold};
pub use codec::{
    check_dimensions, decode_bytes, decode_file, decode_page, dimensions, encode_jpeg, encode_png, encode_ppm, is_jpeg,
    jpeg_passthrough, page_count,
};
pub use transform::{
    apply_filter, apply_filter_report, crop_norm, despeckle, despeckle_report, downsample, fit_within, rotate_quarter,
    rotate_small, white,
};

pub use image::{DynamicImage, GrayImage, RgbaImage};

/// Default sensitivity for [`is_blank`]: the fraction of the page that may be
/// ink before the page stops counting as blank.
pub const BLANK_SENSITIVITY: f32 = 0.002;
