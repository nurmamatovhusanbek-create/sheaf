//! MOD-PDF: PDFium operations. Runs only inside PROC-WORKER (SEC-ISOLATE).
//!
//! Scope so far: open, probe, render, build, text extraction and the OCR
//! text-layer overlay. lopdf-based operations (encryption, metadata,
//! stamps) arrive in PH-3.
//! See `ARCHITECTURE.md`, anchor `mod-pdf`.

use pdfium_render::prelude::*;
use sheaf_core::consts::{MAX_PAGE_PIXELS, MAX_PAGE_SIDE, MAX_SOURCE_PAGES, RASTER_DPI};
use sheaf_core::proto::ProbeResult;
use sheaf_core::{AppError, ErrorCode, NormRect, Result, Rot, SourceKind};
use sheaf_image::RgbaImage;
use std::path::{Path, PathBuf};

fn map_err(e: PdfiumError) -> AppError {
    use PdfiumInternalError as I;
    match e {
        PdfiumError::PdfiumLibraryInternalError(I::PasswordError) => {
            AppError::new(ErrorCode::NeedsPassword, "document is password protected")
        }
        PdfiumError::PdfiumLibraryInternalError(I::FormatError | I::FileError | I::PageError) => {
            AppError::new(ErrorCode::ParseError, "not a readable PDF")
        }
        PdfiumError::PdfiumLibraryInternalError(I::SecurityError) => {
            AppError::new(ErrorCode::Unsupported, "unsupported security handler")
        }
        PdfiumError::IoError(io) => AppError::io("pdf i/o", &io),
        PdfiumError::PageIndexOutOfBounds => AppError::invalid("page index out of range"),
        // Variant names only: messages could quote document content (SEC-LOG).
        other => AppError::new(
            ErrorCode::ParseError,
            format!("pdf engine: {}", variant_name(&other)),
        ),
    }
}

fn variant_name(e: &PdfiumError) -> String {
    let dbg = format!("{e:?}");
    dbg.split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or("error")
        .to_string()
}

fn to_rotation(rot: Rot) -> PdfPageRenderRotation {
    match rot {
        Rot::R0 => PdfPageRenderRotation::None,
        Rot::R90 => PdfPageRenderRotation::Degrees90,
        Rot::R180 => PdfPageRenderRotation::Degrees180,
        Rot::R270 => PdfPageRenderRotation::Degrees270,
    }
}

fn from_rotation(rot: PdfPageRenderRotation) -> Rot {
    match rot {
        PdfPageRenderRotation::None => Rot::R0,
        PdfPageRenderRotation::Degrees90 => Rot::R90,
        PdfPageRenderRotation::Degrees180 => Rot::R180,
        PdfPageRenderRotation::Degrees270 => Rot::R270,
    }
}

/// OCR-PIPELINE step 1: does the page carry any real text? Looks at the
/// characters themselves rather than a bounding-box query, which misses text
/// on pages with a `/Rotate` entry.
fn has_text_layer(page: &PdfPage) -> bool {
    page.text().is_ok_and(|text| {
        text.chars()
            .iter()
            .take(5_000)
            .any(|c| c.unicode_char().is_some_and(|ch| !ch.is_whitespace()))
    })
}

/// A loaded PDFium library. One per worker process: PDFium is not thread-safe
/// and its authors recommend process-level parallelism (ADR-WORKER-PROCS).
pub struct Engine {
    pdfium: Pdfium,
}

impl Engine {
    /// Bind to the PDFium library inside `dir`.
    pub fn load(dir: &Path) -> Result<Engine> {
        let lib = Pdfium::pdfium_platform_library_name_at_path(dir);
        let bindings = Pdfium::bind_to_library(&lib).map_err(|_| {
            AppError::new(
                ErrorCode::EngineMissing,
                "PDFium library could not be loaded",
            )
        })?;
        Ok(Engine {
            pdfium: Pdfium::new(bindings),
        })
    }

    fn open(&self, path: &Path) -> Result<PdfDocument<'_>> {
        self.pdfium.load_pdf_from_file(path, None).map_err(map_err)
    }

    /// `import_probe` for a PDF: page count, encryption, per-page text flag and size.
    pub fn probe(&self, path: &Path) -> Result<ProbeResult> {
        let doc = match self.open(path) {
            Ok(doc) => doc,
            Err(e) if e.code == ErrorCode::NeedsPassword => {
                return Ok(ProbeResult {
                    kind: SourceKind::Pdf,
                    page_count: 0,
                    encrypted: true,
                    has_text: Vec::new(),
                    sizes_pt: Vec::new(),
                });
            }
            Err(e) => return Err(e),
        };
        let count = doc.pages().len().max(0) as u32;
        if count == 0 {
            return Err(AppError::new(ErrorCode::ParseError, "PDF has no pages"));
        }
        if count > MAX_SOURCE_PAGES {
            return Err(AppError::limit("PDF has more pages than the limit"));
        }
        let mut has_text = Vec::with_capacity(count as usize);
        let mut sizes_pt = Vec::with_capacity(count as usize);
        for i in 0..count {
            let page = doc.pages().get(i as i32).map_err(map_err)?;
            sizes_pt.push((page.width().value, page.height().value));
            has_text.push(has_text_layer(&page));
        }
        Ok(ProbeResult {
            kind: SourceKind::Pdf,
            page_count: count,
            encrypted: false,
            has_text,
            sizes_pt,
        })
    }

    /// Render one page as displayed (its own `/Rotate` applied), scaled to fit
    /// `max_px` and never finer than [`RASTER_DPI`].
    pub fn render(&self, path: &Path, index: u32, max_px: (u32, u32)) -> Result<RgbaImage> {
        let doc = self.open(path)?;
        let page = doc.pages().get(index as i32).map_err(map_err)?;
        let (w_pt, h_pt) = (
            f64::from(page.width().value),
            f64::from(page.height().value),
        );
        if !(w_pt >= 1.0 && h_pt >= 1.0) {
            return Err(AppError::new(ErrorCode::ParseError, "page has no area"));
        }
        let (max_w, max_h) = (
            f64::from(max_px.0.clamp(1, MAX_PAGE_SIDE)),
            f64::from(max_px.1.clamp(1, MAX_PAGE_SIDE)),
        );
        let budget = (MAX_PAGE_PIXELS as f64 / (w_pt * h_pt)).sqrt();
        let scale = (max_w / w_pt)
            .min(max_h / h_pt)
            .min(f64::from(RASTER_DPI) / 72.0)
            .min(budget);
        let (w, h) = (
            ((w_pt * scale).round() as i32).max(1),
            ((h_pt * scale).round() as i32).max(1),
        );
        let config = PdfRenderConfig::new()
            .set_target_width(w)
            .set_maximum_height(h);
        let bitmap = page.render_with_config(&config).map_err(map_err)?;
        Ok(bitmap.as_image().map_err(map_err)?.into_rgba8())
    }

    /// OCR-PIPELINE step 1 for one page.
    pub fn page_has_text(&self, path: &Path, index: u32) -> Result<bool> {
        let doc = self.open(path)?;
        let page = doc.pages().get(index as i32).map_err(map_err)?;
        Ok(has_text_layer(&page))
    }

    /// Text on a page, in the order PDFium reports it. Invisible OCR text is
    /// included: this is what "searchable" means. With `within`, only text
    /// inside that part of the page as displayed.
    pub fn page_text(&self, path: &Path, index: u32, within: Option<NormRect>) -> Result<String> {
        let doc = self.open(path)?;
        let page = doc.pages().get(index as i32).map_err(map_err)?;
        let text = page.text().map_err(map_err)?;
        let clean = |s: String| s.chars().filter(|c| *c != '\u{0}').collect::<String>();
        let Some(rect) = within else {
            return Ok(clean(
                text.chars()
                    .iter()
                    .filter_map(|c| c.unicode_char())
                    .collect(),
            ));
        };
        let in_range = [rect.x0, rect.y0, rect.x1, rect.y1]
            .iter()
            .all(|v| (0.0..=1.0).contains(v));
        if !(in_range && rect.x0 < rect.x1 && rect.y0 < rect.y1) {
            return Err(AppError::invalid("text region out of range"));
        }
        // Displayed frame (top-left origin, fractions) to PDF user space.
        let own = from_rotation(page.rotation().map_err(map_err)?);
        let shown = (page.width().value, page.height().value);
        let media = swap_if_turned(own, shown);
        let [a, b, c, d, e, f] = unrotate(own, media.0, media.1);
        let corner = |nx: f32, ny: f32| {
            let (x, y) = (nx * shown.0, (1.0 - ny) * shown.1);
            (a * x + c * y + e, b * x + d * y + f)
        };
        let (p, q) = (corner(rect.x0, rect.y0), corner(rect.x1, rect.y1));
        let area = PdfRect::new_from_values(p.1.min(q.1), p.0.min(q.0), p.1.max(q.1), p.0.max(q.0));
        Ok(clean(text.inside_rect(area)))
    }

    /// Width and height of a page in points, as displayed.
    pub fn page_size_pt(&self, path: &Path, index: u32) -> Result<(f32, f32)> {
        let doc = self.open(path)?;
        let page = doc.pages().get(index as i32).map_err(map_err)?;
        Ok((page.width().value, page.height().value))
    }

    /// Start assembling an output PDF.
    pub fn builder(&self) -> Result<Builder<'_>> {
        Ok(Builder {
            engine: self,
            doc: self.pdfium.create_new_pdf().map_err(map_err)?,
            sources: Vec::new(),
            stats: BuildStats::default(),
        })
    }
}

/// An invisible text layer for one page (OCR-TEXTLAYER): a PDF whose single
/// page holds only invisible text, as large as the image the OCR engine saw.
#[derive(Debug, Clone, Copy)]
pub struct TextLayer<'p> {
    pub pdf: &'p Path,
    /// Clockwise turn applied to the page image before recognition.
    pub rotate: Rot,
}

/// Affine map `[a b c d e f]`: `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
type Affine = [f32; 6];

fn compose(outer: Affine, inner: Affine) -> Affine {
    let [a1, b1, c1, d1, e1, f1] = outer;
    let [a2, b2, c2, d2, e2, f2] = inner;
    [
        a1 * a2 + c1 * b2,
        b1 * a2 + d1 * b2,
        a1 * c2 + c1 * d2,
        b1 * c2 + d1 * d2,
        a1 * e2 + c1 * f2 + e1,
        b1 * e2 + d1 * f2 + f1,
    ]
}

/// Take a point in the frame you get by turning a `w`×`h` box clockwise by
/// `rot` back to the box's own coordinates (PDF axes: origin bottom-left).
fn unrotate(rot: Rot, w: f32, h: f32) -> Affine {
    match rot {
        Rot::R0 => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        Rot::R90 => [0.0, 1.0, -1.0, 0.0, w, 0.0],
        Rot::R180 => [-1.0, 0.0, 0.0, -1.0, w, h],
        Rot::R270 => [0.0, -1.0, 1.0, 0.0, 0.0, h],
    }
}

fn swap_if_turned(rot: Rot, (w, h): (f32, f32)) -> (f32, f32) {
    if matches!(rot, Rot::R90 | Rot::R270) {
        (h, w)
    } else {
        (w, h)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BuildStats {
    pub pages: u32,
    /// Imported from their source without re-encoding (PERF-LOSSLESS).
    pub copied: u32,
    pub rasterized: u32,
    /// Pages that received an OCR text layer.
    pub text_layers: u32,
}

/// Assembles a PDF page by page, so a long document never has to hold all of
/// its rasterized pages in memory at once.
pub struct Builder<'e> {
    engine: &'e Engine,
    doc: PdfDocument<'e>,
    /// Source documents stay open for the whole build: one parse per file.
    sources: Vec<(PathBuf, PdfDocument<'e>)>,
    stats: BuildStats,
}

impl<'e> Builder<'e> {
    fn source_slot(&mut self, path: &Path) -> Result<usize> {
        if let Some(i) = self.sources.iter().position(|(p, _)| p == path) {
            return Ok(i);
        }
        let doc = self.engine.open(path)?;
        self.sources.push((path.to_path_buf(), doc));
        Ok(self.sources.len() - 1)
    }

    /// Lay `layer` over the last page. Must run before any extra rotation is
    /// added, while the page still shows as its source displays it.
    fn overlay_last(&mut self, layer: Option<TextLayer>) -> Result<()> {
        let Some(layer) = layer else { return Ok(()) };
        let text_doc = self.engine.open(layer.pdf)?;
        let mut text_page = text_doc.pages().get(0).map_err(map_err)?;
        let text_size = (text_page.width().value, text_page.height().value);
        if !(text_size.0 >= 1.0 && text_size.1 >= 1.0) {
            return Err(AppError::new(
                ErrorCode::ParseError,
                "text layer has no area",
            ));
        }
        // `objects_mut` is the accessor whose lifetime matches the destination document.
        let mut object = text_page
            .objects_mut()
            .copy_into_x_object_form_object(&mut self.doc)
            .map_err(map_err)?;

        let mut page = self.doc.pages().last().map_err(map_err)?;
        let own = from_rotation(page.rotation().map_err(map_err)?);
        // As displayed (own /Rotate applied), and the unrotated media size under it.
        let shown = (page.width().value, page.height().value);
        let media = swap_if_turned(own, shown);
        // The OCR image is the displayed page, turned once more by `layer.rotate`.
        let ocr = swap_if_turned(layer.rotate, shown);
        let scale: Affine = [ocr.0 / text_size.0, 0.0, 0.0, ocr.1 / text_size.1, 0.0, 0.0];
        let to_shown = unrotate(layer.rotate, shown.0, shown.1);
        let to_media = unrotate(own, media.0, media.1);
        let [a, b, c, d, e, f] = compose(to_media, compose(to_shown, scale));
        object.transform(a, b, c, d, e, f).map_err(map_err)?;
        page.objects_mut().add_object(object).map_err(map_err)?;
        self.stats.text_layers += 1;
        Ok(())
    }

    fn rotate_last(&mut self, extra: Rot) -> Result<()> {
        if extra == Rot::R0 {
            return Ok(());
        }
        let mut page = self.doc.pages().last().map_err(map_err)?;
        let current = from_rotation(page.rotation().map_err(map_err)?);
        page.set_rotation(to_rotation(current.plus(extra)));
        Ok(())
    }

    /// Import a page untouched and add `rot` to its `/Rotate` (TOOL-ROTATE).
    pub fn copy_page(
        &mut self,
        path: &Path,
        index: u32,
        rot: Rot,
        text: Option<TextLayer>,
    ) -> Result<()> {
        let slot = self.source_slot(path)?;
        let dest = self.doc.pages().len();
        let source = &self.sources[slot].1;
        if index as i32 >= source.pages().len() {
            return Err(AppError::invalid("page index out of range"));
        }
        self.doc
            .pages_mut()
            .copy_page_from_document(source, index as i32, dest)
            .map_err(map_err)?;
        self.overlay_last(text)?;
        self.rotate_last(rot)?;
        self.stats.pages += 1;
        self.stats.copied += 1;
        Ok(())
    }

    /// Add a page that shows `img` edge to edge. `dpi` fixes the page size.
    pub fn image_page(
        &mut self,
        img: &RgbaImage,
        dpi: u32,
        rot: Rot,
        text: Option<TextLayer>,
    ) -> Result<()> {
        let dpi = dpi.clamp(36, 2400) as f32;
        let (w_pt, h_pt) = (
            img.width() as f32 * 72.0 / dpi,
            img.height() as f32 * 72.0 / dpi,
        );
        // PDF images have no alpha here: flatten onto white.
        let mut rgb = image::RgbImage::new(img.width(), img.height());
        for (dst, src) in rgb.pixels_mut().zip(img.pixels()) {
            let a = u32::from(src[3]);
            for c in 0..3 {
                dst[c] = ((u32::from(src[c]) * a + 255 * (255 - a)) / 255) as u8;
            }
        }
        let size = PdfPagePaperSize::Custom(PdfPoints::new(w_pt), PdfPoints::new(h_pt));
        let mut page = self
            .doc
            .pages_mut()
            .create_page_at_end(size)
            .map_err(map_err)?;
        let object = PdfPageImageObject::new_with_size(
            &self.doc,
            &image::DynamicImage::ImageRgb8(rgb),
            PdfPoints::new(w_pt),
            PdfPoints::new(h_pt),
        )
        .map_err(map_err)?;
        page.objects_mut()
            .add_image_object(object)
            .map_err(map_err)?;
        drop(page);
        self.overlay_last(text)?;
        self.rotate_last(rot)?;
        self.stats.pages += 1;
        self.stats.rasterized += 1;
        Ok(())
    }

    /// Add an A4 page of visible text set in the TrueType font at `font`,
    /// one entry of `lines` per line. Groundwork for TOOL-WATERMARK and
    /// TOOL-PAGENUM; today it produces the synthetic OCR fixtures.
    pub fn text_page(&mut self, lines: &[&str], font: &Path, size_pt: f32) -> Result<()> {
        let font = self
            .doc
            .fonts_mut()
            .load_true_type_from_file(font, true)
            .map_err(map_err)?;
        let (w, h, margin) = (595.0f32, 842.0f32, 56.0f32);
        let size = PdfPagePaperSize::Custom(PdfPoints::new(w), PdfPoints::new(h));
        let mut page = self
            .doc
            .pages_mut()
            .create_page_at_end(size)
            .map_err(map_err)?;
        for (i, line) in lines.iter().enumerate() {
            let y = h - margin - size_pt * 1.6 * (i as f32 + 1.0);
            if y < margin {
                return Err(AppError::invalid("too many lines for one page"));
            }
            let mut object =
                PdfPageTextObject::new(&self.doc, *line, font, PdfPoints::new(size_pt))
                    .map_err(map_err)?;
            object
                .translate(PdfPoints::new(margin), PdfPoints::new(y))
                .map_err(map_err)?;
            page.objects_mut()
                .add_text_object(object)
                .map_err(map_err)?;
        }
        self.stats.pages += 1;
        Ok(())
    }

    /// Add an empty page of the given size in points.
    pub fn blank_page(&mut self, width_pt: f32, height_pt: f32, rot: Rot) -> Result<()> {
        let size = PdfPagePaperSize::Custom(PdfPoints::new(width_pt), PdfPoints::new(height_pt));
        self.doc
            .pages_mut()
            .create_page_at_end(size)
            .map_err(map_err)?;
        self.rotate_last(rot)?;
        self.stats.pages += 1;
        Ok(())
    }

    /// Write the document. The caller picks a path inside the session dir;
    /// publishing to the user's folder is MOD-STORE's job.
    pub fn save(self, out: &Path) -> Result<BuildStats> {
        if self.stats.pages == 0 {
            return Err(AppError::invalid("nothing to export"));
        }
        self.doc.save_to_file(out).map_err(map_err)?;
        Ok(self.stats)
    }
}
