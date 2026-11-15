use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sheaf_core::consts::{MAX_PAGE_SIDE, RASTER_DPI};
use sheaf_core::proto::{
    ops, BuildArgs, BuildResult, ExportImagesArgs, ExtractImagesArgs, FactsArgs, FactsResult, FilesResult, ImageFormat,
    OcrArgs, OcrPageResult, PageSpec, PageTextArgs, PageTextResult, ProbeArgs, ProbeResult, RenderArgs, RenderResult,
    RepairArgs, RepairResult, Request, SourceRef,
};
use sheaf_core::{AppError, ErrorCode, Result, Rot, SourceKind};
use sheaf_image::RgbaImage;
use sheaf_ocr::{cache_key, choose_langs, CacheEntry, Meta, OcrEngine, RecOpts, Tesseract};
use sheaf_pdf::{lo, Engine, Src, TextLayer};
use sheaf_store::Session;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Blank pages: about 1 cm to 5 m a side, in points.
const BLANK_SIDE_PT: std::ops::RangeInclusive<f32> = 28.0..=14_400.0;

/// Paths to the bundled pieces a worker may be started with.
#[derive(Debug, Clone, Default)]
pub struct Bundled {
    /// Tesseract executable and tessdata dir.
    pub tess: Option<(PathBuf, PathBuf)>,
    /// TrueType font for stamps (TOOL-WATERMARK, TOOL-PAGENUM).
    pub font: Option<PathBuf>,
}

pub struct Worker {
    session: Session,
    pdfium_dir: PathBuf,
    /// Loaded on first use: image-only jobs never pay for PDFium.
    engine: Option<Engine>,
    bundled: Bundled,
    tess_version: Option<String>,
}

/// Recognition settings that are part of the cache key (DATA-CACHEKEY).
const OCR_PARAMS: &str = "oem1;psm3;fast";

fn parse<T: DeserializeOwned>(args: &Value) -> Result<T> {
    serde_json::from_value(args.clone()).map_err(|_| AppError::invalid("arguments do not match the op"))
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| AppError::internal(format!("encode result: {e}")))
}

/// The main process validated the path already; check again here, because
/// the worker is the one about to open it.
fn input(path: &str) -> Result<PathBuf> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(AppError::new(ErrorCode::PathRejected, "input path must be absolute"));
    }
    Ok(sheaf_store::allow_input(p)?.path)
}

/// A source PDF resolved for PDFium: checked path plus its password.
struct Opened<'a> {
    path: PathBuf,
    password: Option<&'a str>,
}

impl<'a> Opened<'a> {
    fn of(source: &'a SourceRef) -> Result<Self> {
        Ok(Opened {
            path: input(&source.path)?,
            password: source.password.as_ref().map(|p| p.expose()),
        })
    }
    fn src(&self) -> Src<'_> {
        Src {
            path: &self.path,
            password: self.password,
        }
    }
}

fn sniff(path: &Path) -> Result<SourceKind> {
    let mut head = [0u8; 1024];
    let mut file = std::fs::File::open(path).map_err(|e| AppError::io("open input", &e))?;
    let n = file.read(&mut head).map_err(|e| AppError::io("read input", &e))?;
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
    let opened = Opened::of(source)?;
    let (img, dpi) = match source.kind {
        SourceKind::Pdf => {
            let engine = engine.ok_or_else(|| AppError::internal("engine not loaded"))?;
            let img = engine.render(opened.src(), page.index, box_px)?;
            // Usually RASTER_DPI; lower for a thumbnail box or an oversized page.
            let (w_pt, _) = engine.page_size_pt(opened.src(), page.index)?;
            let dpi = (img.width() as f32 * 72.0 / w_pt.max(1.0)).round() as u32;
            (img, dpi.max(1))
        }
        SourceKind::Image => (sheaf_image::decode_page(&opened.path, page.index)?, RASTER_DPI),
    };
    let img = match page.crop {
        // In "hide only" mode the crop is a CropBox, applied at build time, not to pixels.
        Some(rect) if !(page.crop_hide_only && source.kind == SourceKind::Pdf) => {
            rect.validate()?;
            sheaf_image::crop_norm(&img, rect)
        }
        _ => img,
    };
    page.filter.validate()?;
    Ok((sheaf_image::apply_filter(img, &page.filter), dpi))
}

/// True when the page can be imported from its PDF as is (PERF-LOSSLESS).
/// A "hide only" crop keeps the page untouched: it becomes a CropBox.
fn is_untouched_pdf(source: &SourceRef, page: &PageSpec) -> bool {
    source.kind == SourceKind::Pdf
        && (page.crop.is_none() || page.crop_hide_only)
        && page.filter.is_identity()
        && !page.rasterize
}

/// The page as the user sees it: pixels, then `rot`.
fn displayed(engine: Option<&Engine>, page: &PageSpec, box_px: (u32, u32)) -> Result<RgbaImage> {
    let turned = matches!(page.rot, Rot::R90 | Rot::R270);
    // Fit is judged after rotation, so the pre-rotation box is swapped.
    let pre = if turned { (box_px.1, box_px.0) } else { box_px };
    let img = match &page.source {
        Some(source) => pixels(engine, source, page, pre)?.0,
        None => {
            let (w_pt, h_pt) = blank_size(page)?;
            let scale = (pre.0 as f32 / w_pt)
                .min(pre.1 as f32 / h_pt)
                .min(RASTER_DPI as f32 / 72.0);
            sheaf_image::white((w_pt * scale) as u32, (h_pt * scale) as u32)
        }
    };
    Ok(sheaf_image::rotate_quarter(img, page.rot))
}

fn needs_pdfium(page: &PageSpec) -> bool {
    page.source.as_ref().is_some_and(|s| s.kind == SourceKind::Pdf)
}

impl Worker {
    pub fn new(session_dir: &Path, pdfium_dir: PathBuf, bundled: Bundled) -> Result<Worker> {
        Ok(Worker {
            session: Session::attach(session_dir)?,
            pdfium_dir,
            engine: None,
            bundled,
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

    /// PDFium if `page` needs it, loaded on demand.
    fn engine_for(&mut self, page: &PageSpec) -> Result<Option<&Engine>> {
        match needs_pdfium(page) {
            true => self.engine().map(Some),
            false => Ok(None),
        }
    }

    /// A fresh directory inside the session dir for a job's output files.
    fn out_dir(&self, rel: &str) -> Result<PathBuf> {
        let dir = self.session.resolve(rel)?;
        std::fs::create_dir_all(&dir).map_err(|e| AppError::io("create output dir", &e))?;
        Ok(dir)
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
                let path = input(&args.path)?;
                let text = self.engine()?.page_text(Src::plain(&path), args.index, args.within)?;
                to_value(&PageTextResult { text })
            }
            ops::PAGE_FACTS => to_value(&self.page_facts(&parse(&req.args)?)?),
            ops::EXPORT_IMAGES => to_value(&self.export_images(&parse(&req.args)?, progress)?),
            ops::EXTRACT_IMAGES => to_value(&self.extract_images(&parse(&req.args)?)?),
            ops::REPAIR => to_value(&self.repair(&parse(&req.args)?)?),
            _ => Err(AppError::invalid("unknown op")),
        }
    }

    fn probe(&mut self, args: &ProbeArgs) -> Result<ProbeResult> {
        let path = input(&args.path)?;
        match sniff(&path)? {
            SourceKind::Pdf => {
                let password = args.password.as_ref().map(|p| p.expose());
                self.engine()?.probe(Src { path: &path, password })
            }
            SourceKind::Image => {
                let (w, h) = sheaf_image::dimensions(&path)?;
                let pages = sheaf_image::page_count(&path)?;
                let pt = |px: u32| px as f32 * 72.0 / RASTER_DPI as f32;
                Ok(ProbeResult {
                    kind: SourceKind::Image,
                    page_count: pages,
                    encrypted: false,
                    restricted: false,
                    has_forms: false,
                    has_text: vec![false; pages as usize],
                    // Later TIFF pages may differ; the first page's size stands in for them.
                    sizes_pt: vec![(pt(w), pt(h)); pages as usize],
                })
            }
        }
    }

    /// TOOL-REPAIR: rebuild the structure, then open the result with the renderer and say how
    /// many pages it really has. Whether the original was damaged is judged the same way.
    fn repair(&mut self, args: &RepairArgs) -> Result<RepairResult> {
        let path = input(&args.path)?;
        if sniff(&path)? != SourceKind::Pdf {
            return Err(AppError::new(ErrorCode::Unsupported, "only PDF files can be repaired"));
        }
        let out = self.session.resolve(&args.out)?;
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AppError::io("create output dir", &e))?;
        }
        let before = self.engine()?.probe(Src::plain(&path)).ok().map(|p| p.page_count);
        let found = sheaf_pdf::lo::repair(&path, &out)?;
        let after = self.engine()?.probe(Src::plain(&out))?.page_count;
        if after == 0 {
            let _ = std::fs::remove_file(&out);
            return Err(AppError::new(ErrorCode::ParseError, "no pages could be recovered"));
        }
        Ok(RepairResult {
            out: args.out.clone(),
            pages: after,
            was_damaged: before != Some(after) || before != Some(found),
        })
    }

    fn render(&mut self, args: &RenderArgs) -> Result<RenderResult> {
        let out = self.session.resolve(&args.out)?;
        let max = (
            args.max_px.0.clamp(1, MAX_PAGE_SIDE),
            args.max_px.1.clamp(1, MAX_PAGE_SIDE),
        );
        let engine = self.engine_for(&args.page)?;
        let img = sheaf_image::fit_within(displayed(engine, &args.page, max)?, max.0, max.1);
        sheaf_store::write_atomic(&out, &sheaf_image::encode_png(&img)?)?;
        Ok(RenderResult {
            out: args.out.clone(),
            width: img.width(),
            height: img.height(),
        })
    }

    /// What the split planner asks about a page: is it blank, what does it say?
    fn page_facts(&mut self, args: &FactsArgs) -> Result<FactsResult> {
        let page = &args.page;
        let mut facts = FactsResult::default();
        if args.blank {
            facts.blank = match &page.source {
                None => true,
                Some(_) => {
                    // A low-resolution look is enough to tell paper from content.
                    let engine = self.engine_for(page)?;
                    let img = displayed(engine, page, (1000, 1000))?;
                    sheaf_image::is_blank(&img, sheaf_image::BLANK_SENSITIVITY)
                }
            };
        }
        if args.text {
            facts.text = match (&page.text_layer, &page.source) {
                // Recognized text first: it describes the pixels as they are now.
                (Some(key), _) => CacheEntry::at(&self.session, key)?.result()?.text(),
                (None, Some(source)) if is_untouched_pdf(source, page) => {
                    let opened = Opened::of(source)?;
                    self.engine()?.page_text(opened.src(), page.index, None)?
                }
                _ => String::new(),
            };
        }
        Ok(facts)
    }

    /// TOOL-PDF2IMG: one image file per page, as displayed.
    fn export_images(&mut self, args: &ExportImagesArgs, progress: &mut dyn FnMut(f32)) -> Result<FilesResult> {
        if args.pages.is_empty() || args.pages.len() > 100_000 {
            return Err(AppError::invalid("nothing to export"));
        }
        if !(36..=RASTER_DPI * 2).contains(&args.dpi) {
            return Err(AppError::invalid("resolution out of range"));
        }
        let dir = self.out_dir(&args.out_dir)?;
        let digits = args.pages.len().to_string().len().max(3);
        let mut files = Vec::with_capacity(args.pages.len());
        for (i, page) in args.pages.iter().enumerate() {
            let engine = self.engine_for(page)?;
            let img = displayed(engine, page, (MAX_PAGE_SIDE, MAX_PAGE_SIDE))?;
            // Pages come out of the renderer at RASTER_DPI (images are taken to be that too).
            let img = sheaf_image::downsample(img, args.dpi, RASTER_DPI);
            let (bytes, ext) = match args.format {
                ImageFormat::Png => (sheaf_image::encode_png(&img)?, "png"),
                ImageFormat::Jpg => (sheaf_image::encode_jpeg(&img, args.quality.clamp(1, 100))?, "jpg"),
            };
            let name = format!("page-{:0digits$}.{ext}", i + 1);
            sheaf_store::write_atomic(&dir.join(&name), &bytes)?;
            files.push(format!("{}/{name}", args.out_dir));
            progress((i + 1) as f32 / args.pages.len() as f32);
        }
        Ok(FilesResult { files, skipped: 0 })
    }

    /// TOOL-PDF2IMG, alternative mode: the images embedded in a page, unchanged where possible.
    fn extract_images(&mut self, args: &ExtractImagesArgs) -> Result<FilesResult> {
        if args.source.kind != SourceKind::Pdf {
            return Err(AppError::invalid("only PDF pages have embedded images"));
        }
        let opened = Opened::of(&args.source)?;
        let dir = self.out_dir(&args.out_dir)?;
        let stem = format!("page-{:03}", args.index + 1);
        let (paths, skipped) = lo::extract_images(&opened.path, opened.password, args.index, &dir, &stem)?;
        let files = paths
            .iter()
            .filter_map(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| format!("{}/{n}", args.out_dir))
            })
            .collect();
        Ok(FilesResult { files, skipped })
    }

    fn build(&mut self, args: &BuildArgs, progress: &mut dyn FnMut(f32)) -> Result<BuildResult> {
        let out = self.session.resolve(&args.out)?;
        if args.pages.is_empty() {
            return Err(AppError::invalid("nothing to export"));
        }
        for stamp in &args.stamps {
            stamp.validate()?;
        }
        let tmp = out.with_extension("part");
        // Text layers come from the OCR cache; resolve them before the engine is borrowed.
        let mut layers: Vec<Option<(PathBuf, Rot)>> = Vec::with_capacity(args.pages.len());
        for page in &args.pages {
            layers.push(match &page.text_layer {
                Some(key) => {
                    let entry = CacheEntry::at(&self.session, key)?;
                    let meta = entry.meta().ok_or_else(|| AppError::not_found("OCR result"))?;
                    Some((entry.text_pdf(), Rot::from_degrees(i32::from(meta.rotate))?))
                }
                None => None,
            });
        }
        fn as_layer(l: &(PathBuf, Rot)) -> TextLayer<'_> {
            TextLayer { pdf: &l.0, rotate: l.1 }
        }
        let font = self.bundled.font.clone();
        let session_dir = self.session.dir().to_path_buf();
        let engine = self.engine()?;
        let mut assemble = || -> Result<sheaf_pdf::BuildStats> {
            let mut builder = engine.builder()?;
            let total = args.pages.len() as f32;
            for (i, page) in args.pages.iter().enumerate() {
                let layer = layers[i].as_ref().map(as_layer);
                match &page.source {
                    None => {
                        let (w, h) = blank_size(page)?;
                        builder.blank_page(w, h, page.rot)?;
                    }
                    // Untouched PDF pages are imported, never re-encoded (PERF-LOSSLESS).
                    Some(src) if is_untouched_pdf(src, page) => {
                        let opened = Opened::of(src)?;
                        let hide = page.crop.filter(|_| page.crop_hide_only);
                        builder.copy_page(opened.src(), page.index, page.rot, layer, hide)?;
                        if page.flatten_forms {
                            builder.flatten_last()?;
                        }
                    }
                    Some(src) => {
                        let path = input(&src.path)?;
                        // A JPEG with no edits goes in byte for byte (TOOL-IMG2PDF).
                        let untouched = page.crop.is_none() && page.filter.is_identity() && page.index == 0;
                        match sheaf_image::jpeg_passthrough(&path)
                            .filter(|_| src.kind == SourceKind::Image && untouched)
                        {
                            Some((px, exif)) => {
                                builder.jpeg_page(&path, px, RASTER_DPI, exif, page.rot, layer, args.image_fit)?;
                            }
                            // Everything else is rasterized, one page at a time, so
                            // memory stays flat however long the document is.
                            // Rotation stays a /Rotate entry even here.
                            None => {
                                let (img, dpi) = pixels(Some(engine), src, page, (MAX_PAGE_SIDE, MAX_PAGE_SIDE))?;
                                let fit = match src.kind {
                                    SourceKind::Image => args.image_fit,
                                    SourceKind::Pdf => Default::default(),
                                };
                                builder.image_page(&img, dpi, page.rot, layer, fit)?;
                            }
                        }
                    }
                }
                progress((i + 1) as f32 / total);
            }
            if !args.stamps.is_empty() {
                let font = font
                    .as_deref()
                    .ok_or_else(|| AppError::new(ErrorCode::EngineMissing, "stamp font is not configured"))?;
                // Stamp images live in the session dir, where the main process staged them.
                let image_of = |rel: &str| -> Result<RgbaImage> {
                    let plain = !rel.is_empty()
                        && !rel.contains(['\\', ':', '\0'])
                        && rel.split('/').all(|s| !s.is_empty() && s != "." && s != "..");
                    if !plain {
                        return Err(AppError::new(
                            ErrorCode::PathRejected,
                            "stamp image must be inside the session dir",
                        ));
                    }
                    sheaf_image::decode_file(&session_dir.join(rel))
                };
                builder.stamp(&args.stamps, font, &image_of)?;
            }
            builder.save(&tmp)
        };
        // FLOW-EXPORT: build, then optional compress, then optional protect.
        let cleanup = |paths: &[&Path]| {
            for p in paths {
                let _ = std::fs::remove_file(p);
            }
        };
        let packed = out.with_extension("packed");
        let locked = out.with_extension("locked");
        let finish = |stats: sheaf_pdf::BuildStats| -> Result<BuildResult> {
            let mut current = tmp.clone();
            let mut result = BuildResult {
                out: args.out.clone(),
                page_count: stats.pages,
                copied: stats.copied,
                rasterized: stats.rasterized,
                text_layers: stats.text_layers,
                jpeg_passthrough: stats.jpeg_passthrough,
                stamps: stats.stamps,
                flattened: stats.flattened,
                bytes: 0,
                bytes_before_compress: None,
                compress_kept_original: false,
                encryption: None,
            };
            if let Some(level) = args.compress {
                let report = lo::compress(&current, &packed, level)?;
                result.bytes_before_compress = Some(report.bytes_before);
                result.compress_kept_original = report.kept_original;
                current = packed.clone();
            }
            if let Some(protect) = &args.protect {
                result.encryption = Some(lo::encrypt(&current, &locked, protect)?);
                current = locked.clone();
            }
            result.bytes = std::fs::metadata(&current)
                .map_err(|e| AppError::io("stat output", &e))?
                .len();
            std::fs::rename(&current, &out).map_err(|e| AppError::io("finish output", &e))?;
            Ok(result)
        };
        let result = assemble().and_then(finish);
        cleanup(&[&tmp, &packed, &locked]);
        result
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
        if !args.force && is_untouched_pdf(source, page) {
            let opened = Opened::of(source)?;
            if self.engine()?.page_has_text(opened.src(), page.index)? {
                return Ok(skipped);
            }
        }
        let (exe, tessdata) = self
            .bundled
            .tess
            .clone()
            .ok_or_else(|| AppError::new(ErrorCode::EngineMissing, "OCR engine is not configured"))?;
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
            let langs = args.langs.clone().unwrap_or_else(|| choose_langs(osd.script));
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
