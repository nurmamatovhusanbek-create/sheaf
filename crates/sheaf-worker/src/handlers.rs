use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sheaf_core::consts::{MAX_PAGE_SIDE, RASTER_DPI};
use sheaf_core::proto::{
    ops, BuildArgs, BuildResult, OcrArgs, OcrPageResult, PageSpec, PageTextArgs, PageTextResult,
    ProbeArgs, ProbeResult, RenderArgs, RenderResult, Request, SourceRef,
};
use sheaf_core::{AppError, ErrorCode, Result, Rot, SourceKind};
use sheaf_image::RgbaImage;
use sheaf_ocr::{cache_key, choose_langs, CacheEntry, Meta, OcrEngine, RecOpts, Tesseract};
use sheaf_pdf::{Engine, TextLayer};
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
    /// Tesseract executable and tessdata dir, when the app was started with them.
    tess: Option<(PathBuf, PathBuf)>,
    tess_version: Option<String>,
}

/// Recognition settings that are part of the cache key (DATA-CACHEKEY).
const OCR_PARAMS: &str = "oem1;psm3;fast";

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

/// The page's pixels with crop and filter applied, before `rot`, and their
/// resolution. `box_px` bounds the size of a PDF page; images come at their
/// native size and are taken to be [`RASTER_DPI`].
fn pixels(
    engine: Option<&Engine>,
    source: &SourceRef,
    page: &PageSpec,
    box_px: (u32, u32),
) -> Result<(RgbaImage, u32)> {
    let path = input(&source.path)?;
    let (img, dpi) = match source.kind {
        SourceKind::Pdf => {
            let engine = engine.ok_or_else(|| AppError::internal("engine not loaded"))?;
            let img = engine.render(&path, page.index, box_px)?;
            // Usually RASTER_DPI; lower for a thumbnail box or an oversized page.
            let (w_pt, _) = engine.page_size_pt(&path, page.index)?;
            let dpi = (img.width() as f32 * 72.0 / w_pt.max(1.0)).round() as u32;
            (img, dpi.max(1))
        }
        SourceKind::Image => {
            if page.index != 0 {
                return Err(AppError::invalid("page index out of range"));
            }
            (sheaf_image::decode_file(&path)?, RASTER_DPI)
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
    Ok((sheaf_image::apply_filter(img, &page.filter), dpi))
}

/// True when the page can be imported from its PDF as is (PERF-LOSSLESS).
fn is_untouched_pdf(source: &SourceRef, page: &PageSpec) -> bool {
    source.kind == SourceKind::Pdf
        && page.crop.is_none()
        && page.filter.is_identity()
        && !page.rasterize
}

impl Worker {
    pub fn new(
        session_dir: &Path,
        pdfium_dir: PathBuf,
        tess: Option<(PathBuf, PathBuf)>,
    ) -> Result<Worker> {
        Ok(Worker {
            session: Session::attach(session_dir)?,
            pdfium_dir,
            engine: None,
            tess,
            tess_version: None,
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
            ops::OCR_PAGE => to_value(&self.ocr_page(&parse(&req.args)?)?),
            ops::PAGE_TEXT => {
                let args: PageTextArgs = parse(&req.args)?;
                let text =
                    self.engine()?
                        .page_text(&input(&args.path)?, args.index, args.within)?;
                to_value(&PageTextResult { text })
            }
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
                pixels(engine, source, page, pre)?.0
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
        // Text layers come from the OCR cache; resolve them before the engine is borrowed.
        let mut layers: Vec<Option<(PathBuf, Rot)>> = Vec::with_capacity(args.pages.len());
        for page in &args.pages {
            layers.push(match &page.text_layer {
                Some(key) => {
                    let entry = CacheEntry::at(&self.session, key)?;
                    let meta = entry
                        .meta()
                        .ok_or_else(|| AppError::not_found("OCR result"))?;
                    Some((entry.text_pdf(), Rot::from_degrees(i32::from(meta.rotate))?))
                }
                None => None,
            });
        }
        fn as_layer(l: &(PathBuf, Rot)) -> TextLayer<'_> {
            TextLayer {
                pdf: &l.0,
                rotate: l.1,
            }
        }
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
                    Some(src) if is_untouched_pdf(src, page) => {
                        builder.copy_page(
                            &input(&src.path)?,
                            page.index,
                            page.rot,
                            layers[i].as_ref().map(as_layer),
                        )?;
                    }
                    // Images and pages with pixel edits are rasterized one at a
                    // time, so memory stays flat however long the document is.
                    // Rotation stays a /Rotate entry even here.
                    Some(src) => {
                        let (img, dpi) =
                            pixels(Some(engine), src, page, (MAX_PAGE_SIDE, MAX_PAGE_SIDE))?;
                        builder.image_page(
                            &img,
                            dpi,
                            page.rot,
                            layers[i].as_ref().map(as_layer),
                        )?;
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
            text_layers: stats.text_layers,
        })
    }

    /// OCR-PIPELINE for one page.
    fn ocr_page(&mut self, args: &OcrArgs) -> Result<OcrPageResult> {
        let page = &args.page;
        let source = page
            .source
            .as_ref()
            .ok_or_else(|| AppError::invalid("a blank page has nothing to recognize"))?;
        if let Some(langs) = &args.langs {
            sheaf_ocr::langs::validate(langs)?;
        }
        let skipped = OcrPageResult {
            skipped_has_text: true,
            key: None,
            cached: false,
            engine: String::new(),
            langs: Vec::new(),
            mean_conf: 0.0,
            words: 0,
            rotate: 0,
        };
        // Step 1: has text? Only an untouched PDF page can still carry its text layer.
        if !args.force
            && is_untouched_pdf(source, page)
            && self
                .engine()?
                .page_has_text(&input(&source.path)?, page.index)?
        {
            return Ok(skipped);
        }
        let (exe, tessdata) = self.tess.clone().ok_or_else(|| {
            AppError::new(ErrorCode::EngineMissing, "OCR engine is not configured")
        })?;
        let tess = Tesseract::new(&exe, &tessdata, self.session.dir());
        if self.tess_version.is_none() {
            self.tess_version = Some(tess.version());
        }
        let version = self.tess_version.clone().unwrap_or_default();

        // Step 2: render and clean. These pixels are what the engine sees and what the key hashes.
        let engine = match source.kind {
            SourceKind::Pdf => Some(self.engine()?),
            SourceKind::Image => None,
        };
        let (img, dpi) = pixels(engine, source, page, (MAX_PAGE_SIDE, MAX_PAGE_SIDE))?;
        let image_px = img.dimensions();

        // Step 3: cache check.
        let key = cache_key(
            img.as_raw(),
            image_px,
            tess.id(),
            &version,
            args.langs.as_deref(),
            OCR_PARAMS,
        );
        let entry = CacheEntry::at(&self.session, &key)?;
        let report = |meta: &Meta, cached: bool| OcrPageResult {
            skipped_has_text: false,
            key: Some(key.clone()),
            cached,
            engine: format!("{} ({})", meta.engine, meta.version),
            langs: meta.langs.clone(),
            mean_conf: meta.mean_conf,
            words: meta.words,
            rotate: meta.rotate,
        };
        if let Some(meta) = entry.meta() {
            return Ok(report(&meta, true));
        }

        // Paths handed to Tesseract are relative to its working directory (the session dir).
        let work_rel = format!("cache/{key}.work-{}", std::process::id());
        let work = self.session.resolve(&work_rel)?;
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).map_err(|e| AppError::io("create OCR work dir", &e))?;
        let run = || -> Result<Meta> {
            let img_rel = PathBuf::from(format!("{work_rel}/in.ppm"));
            let write = |img: &RgbaImage| {
                std::fs::write(work.join("in.ppm"), sheaf_image::encode_ppm(img))
                    .map_err(|e| AppError::io("write OCR input", &e))
            };
            write(&img)?;
            // Steps 4 and 5: orientation, script, languages. Fixed languages skip detection.
            let osd = match &args.langs {
                Some(_) => Default::default(),
                None => tess.detect(&img_rel)?,
            };
            let turn = Rot::from_degrees(i32::from(osd.rotate))?;
            if turn != Rot::R0 {
                write(&sheaf_image::rotate_quarter(img, turn))?;
            }
            let langs = args
                .langs
                .clone()
                .unwrap_or_else(|| choose_langs(osd.script));
            // Step 6: recognize.
            let opts = RecOpts {
                langs,
                dpi,
                text_pdf: Some(work.join("page.pdf")),
                tsv: Some(work.join("result.tsv")),
            };
            let result = tess.recognize(&img_rel, &opts)?;
            let _ = std::fs::remove_file(work.join("in.ppm"));
            Ok(Meta {
                engine: tess.id().to_string(),
                version: version.clone(),
                langs: result.langs_used,
                script: osd.script,
                rotate: osd.rotate,
                mean_conf: result.mean_conf,
                words: result.words.len() as u32,
                image_px,
                dpi,
            })
        };
        // Step 7: store. (Step 8, indexing, happens in the main process.)
        let meta = run().inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&work);
        })?;
        entry.commit(&work, &meta)?;
        Ok(report(&meta, false))
    }
}
