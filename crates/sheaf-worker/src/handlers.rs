use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sheaf_core::consts::{MAX_PAGE_SIDE, RASTER_DPI};
use sheaf_core::proto::{
    ops, BuildArgs, BuildResult, PageSpec, ProbeArgs, ProbeResult, RenderArgs, RenderResult,
    Request, SourceRef,
};
use sheaf_core::{AppError, ErrorCode, Result, Rot, SourceKind};
use sheaf_image::RgbaImage;
use sheaf_pdf::Engine;
use sheaf_store::Session;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Blank pages: about 1 cm to 5 m a side, in points.
const BLANK_SIDE_PT: std::ops::RangeInclusive<f32> = 28.0..=14_400.0;

pub struct Worker {
    session: Session,
    pdfium_dir: PathBuf,
    /// Loaded on first use: image-only jobs never pay for PDFium.
    engine: Option<Engine>,
}

fn parse<T: DeserializeOwned>(args: &Value) -> Result<T> {
    serde_json::from_value(args.clone())
        .map_err(|_| AppError::invalid("arguments do not match the op"))
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| AppError::internal(format!("encode result: {e}")))
}

/// The main process validated the path already; check again here, because
/// the worker is the one about to open it.
fn input(path: &str) -> Result<PathBuf> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(AppError::new(
            ErrorCode::PathRejected,
            "input path must be absolute",
        ));
    }
    Ok(sheaf_store::allow_input(p)?.path)
}

fn sniff(path: &Path) -> Result<SourceKind> {
    let mut head = [0u8; 1024];
    let mut file = std::fs::File::open(path).map_err(|e| AppError::io("open input", &e))?;
    let n = file
        .read(&mut head)
        .map_err(|e| AppError::io("read input", &e))?;
    if head[..n].windows(5).any(|w| w == b"%PDF-") {
        Ok(SourceKind::Pdf)
    } else {
        Ok(SourceKind::Image)
    }
}

fn blank_size(page: &PageSpec) -> Result<(f32, f32)> {
    let (w, h) = page
        .blank_pt
        .ok_or_else(|| AppError::invalid("page has neither source nor size"))?;
    if BLANK_SIDE_PT.contains(&w) && BLANK_SIDE_PT.contains(&h) {
        Ok((w, h))
    } else {
        Err(AppError::invalid("blank page size out of range"))
    }
}

/// The page's pixels with crop and filter applied, before `rot`. `box_px`
/// bounds the size of a PDF page; images come at their native size.
fn pixels(
    engine: Option<&Engine>,
    source: &SourceRef,
    page: &PageSpec,
    box_px: (u32, u32),
) -> Result<RgbaImage> {
    let path = input(&source.path)?;
    let img = match source.kind {
        SourceKind::Pdf => engine
            .ok_or_else(|| AppError::internal("engine not loaded"))?
            .render(&path, page.index, box_px)?,
        SourceKind::Image => {
            if page.index != 0 {
                return Err(AppError::invalid("page index out of range"));
            }
            sheaf_image::decode_file(&path)?
        }
    };
    let img = match page.crop {
        Some(rect) => {
            rect.validate()?;
            sheaf_image::crop_norm(&img, rect)
        }
        None => img,
    };
    page.filter.validate()?;
    Ok(sheaf_image::apply_filter(img, &page.filter))
}

impl Worker {
    pub fn new(session_dir: &Path, pdfium_dir: PathBuf) -> Result<Worker> {
        Ok(Worker {
            session: Session::attach(session_dir)?,
            pdfium_dir,
            engine: None,
        })
    }

    fn engine(&mut self) -> Result<&Engine> {
        if self.engine.is_none() {
            self.engine = Some(Engine::load(&self.pdfium_dir)?);
        }
        self.engine
            .as_ref()
            .ok_or_else(|| AppError::internal("engine not loaded"))
    }

    pub fn handle(&mut self, req: &Request, progress: &mut dyn FnMut(f32)) -> Result<Value> {
        match req.op.as_str() {
            ops::PING => Ok(json!({"pid": std::process::id()})),
            ops::IMPORT_PROBE => to_value(&self.probe(&parse(&req.args)?)?),
            ops::THUMB | ops::RENDER => to_value(&self.render(&parse(&req.args)?)?),
            ops::BUILD => to_value(&self.build(&parse(&req.args)?, progress)?),
            _ => Err(AppError::invalid("unknown op")),
        }
    }

    fn probe(&mut self, args: &ProbeArgs) -> Result<ProbeResult> {
        let path = input(&args.path)?;
        match sniff(&path)? {
            SourceKind::Pdf => self.engine()?.probe(&path),
            SourceKind::Image => {
                let (w, h) = sheaf_image::dimensions(&path)?;
                let pt = |px: u32| px as f32 * 72.0 / RASTER_DPI as f32;
                Ok(ProbeResult {
                    kind: SourceKind::Image,
                    page_count: 1,
                    encrypted: false,
                    has_text: vec![false],
                    sizes_pt: vec![(pt(w), pt(h))],
                })
            }
        }
    }

    fn render(&mut self, args: &RenderArgs) -> Result<RenderResult> {
        let out = self.session.resolve(&args.out)?;
        let (max_w, max_h) = (
            args.max_px.0.clamp(1, MAX_PAGE_SIDE),
            args.max_px.1.clamp(1, MAX_PAGE_SIDE),
        );
        let page = &args.page;
        let turned = matches!(page.rot, Rot::R90 | Rot::R270);
        // Fit is judged after rotation, so the pre-rotation box is swapped.
        let pre = if turned {
            (max_h, max_w)
        } else {
            (max_w, max_h)
        };
        let img = match &page.source {
            Some(source) => {
                let engine = match source.kind {
                    SourceKind::Pdf => Some(self.engine()?),
                    SourceKind::Image => None,
                };
                pixels(engine, source, page, pre)?
            }
            None => {
                let (w_pt, h_pt) = blank_size(page)?;
                let scale = (pre.0 as f32 / w_pt)
                    .min(pre.1 as f32 / h_pt)
                    .min(RASTER_DPI as f32 / 72.0);
                sheaf_image::white((w_pt * scale) as u32, (h_pt * scale) as u32)
            }
        };
        let img = sheaf_image::fit_within(sheaf_image::rotate_quarter(img, page.rot), max_w, max_h);
        sheaf_store::write_atomic(&out, &sheaf_image::encode_png(&img)?)?;
        Ok(RenderResult {
            out: args.out.clone(),
            width: img.width(),
            height: img.height(),
        })
    }

    fn build(&mut self, args: &BuildArgs, progress: &mut dyn FnMut(f32)) -> Result<BuildResult> {
        let out = self.session.resolve(&args.out)?;
        if args.pages.is_empty() {
            return Err(AppError::invalid("nothing to export"));
        }
        let tmp = out.with_extension("part");
        let engine = self.engine()?;
        let mut assemble = || -> Result<sheaf_pdf::BuildStats> {
            let mut builder = engine.builder()?;
            let total = args.pages.len() as f32;
            for (i, page) in args.pages.iter().enumerate() {
                match &page.source {
                    None => {
                        let (w, h) = blank_size(page)?;
                        builder.blank_page(w, h, page.rot)?;
                    }
                    // Untouched PDF pages are imported, never re-encoded (PERF-LOSSLESS).
                    Some(src)
                        if src.kind == SourceKind::Pdf
                            && page.crop.is_none()
                            && page.filter.is_identity() =>
                    {
                        builder.copy_page(&input(&src.path)?, page.index, page.rot)?;
                    }
                    // Images and pages with pixel edits are rasterized one at a
                    // time, so memory stays flat however long the document is.
                    // Rotation stays a /Rotate entry even here.
                    Some(src) => {
                        let img = pixels(Some(engine), src, page, (MAX_PAGE_SIDE, MAX_PAGE_SIDE))?;
                        builder.image_page(&img, RASTER_DPI, page.rot)?;
                    }
                }
                progress((i + 1) as f32 / total);
            }
            builder.save(&tmp)
        };
        let stats = assemble().inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        std::fs::rename(&tmp, &out).map_err(|e| AppError::io("finish output", &e))?;
        Ok(BuildResult {
            out: args.out.clone(),
            page_count: stats.pages,
            copied: stats.copied,
            rasterized: stats.rasterized,
        })
    }
}
