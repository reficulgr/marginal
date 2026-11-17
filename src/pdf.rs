//! PDF rendering via pdfium-render. The pdfium shared library is resolved
//! from (1) $GLOSSA_PDFIUM_LIB, (2) <data_dir>/pdfium/libpdfium.so,
//! (3) ./vendor/pdfium/libpdfium.so, (4) system library search path.

use anyhow::{anyhow, bail, Context, Result};
use pdfium_render::prelude::*;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Once, OnceLock};

use crate::store::data_dir;

static INIT: Once = Once::new();
static PDFIUM: OnceLock<Pdfium> = OnceLock::new();

fn candidate_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("GLOSSA_PDFIUM_LIB") {
        out.push(PathBuf::from(p));
    }
    out.push(data_dir().join("pdfium").join("libpdfium.so"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join("libpdfium.so"));
        }
    }
    out.push(PathBuf::from("vendor/pdfium/libpdfium.so"));
    out
}

/// Bind pdfium exactly once, process-wide. Returns `&'static Pdfium` so
/// documents opened from it can be stored anywhere without lifetime games.
pub fn engine() -> Result<&'static Pdfium> {
    INIT.call_once(|| {
        for cand in candidate_paths() {
            if cand.is_file() {
                if let Ok(bindings) = Pdfium::bind_to_library(&cand) {
                    let _ = PDFIUM.set(Pdfium::new(bindings));
                    return;
                }
            }
        }
        // Fall back to a system-wide libpdfium.so if present.
        if let Ok(bindings) = Pdfium::bind_to_system_library() {
            let _ = PDFIUM.set(Pdfium::new(bindings));
        }
    });

    PDFIUM.get().ok_or_else(|| {
        anyhow!(
            "pdfium library not found.\n\
             Run `scripts/fetch-pdfium.sh` to install it into {},\n\
             or set GLOSSA_PDFIUM_LIB to the path of libpdfium.so.",
            data_dir().join("pdfium").display()
        )
    })
}

#[derive(Debug)]
pub struct PageImage {
    pub width: u32,
    pub height: u32,
    /// RGBA8, premultiplied not applied — straight alpha, 4 bytes per pixel.
    pub rgba: Vec<u8>,
}

pub struct OpenDoc {
    doc: PdfDocument<'static>,
    #[allow(dead_code)]
    path: PathBuf,
}

impl OpenDoc {
    pub fn open(path: &Path) -> Result<OpenDoc> {
        let pdfium = engine()?;
        let doc = pdfium
            .load_pdf_from_file(path, None)
            .with_context(|| format!("opening PDF {}", path.display()))?;
        Ok(OpenDoc {
            doc,
            path: path.to_path_buf(),
        })
    }

    pub fn page_count(&self) -> u32 {
        self.doc.pages().len() as u32
    }

    /// Embedded outline ("bookmarks" tree): top-level entries only,
    /// as (1-based page number, title).
    pub fn outline(&self) -> Vec<(u32, String)> {
        let count = self.doc.pages().len() as i32;
        let mut out = Vec::new();
        for bm in self.doc.bookmarks().iter() {
            let title = bm.title().unwrap_or_default().trim().to_string();
            if title.is_empty() {
                continue;
            }
            let Some(dest) = bm.destination() else { continue };
            let Ok(idx) = dest.page_index() else { continue };
            let page = (idx.clamp(0, count - 1) + 1) as u32;
            out.push((page, title));
        }
        out
    }

    /// Page size in PDF points (1 pt = 1/72").
    pub fn page_size(&self, page_index: u32) -> Result<(f32, f32)> {
        let pages = self.doc.pages();
        let page = pages.get(page_index as i32)?;
        Ok((page.width().value, page.height().value))
    }

    /// Render one page (0-based) at the given pixel width, keeping aspect.
    pub fn render(&self, page_index: u32, width_px: u32) -> Result<PageImage> {
        let (w_pts, h_pts) = self.page_size(page_index)?;
        if w_pts <= 0.0 || h_pts <= 0.0 {
            bail!("page {} has zero size", page_index + 1);
        }
        let w_px = (width_px.max(8)) as i32;
        let h_px = (w_px as f32 * h_pts / w_pts).round().max(8.0) as i32;

        let config = PdfRenderConfig::new()
            .set_target_width(w_px)
            .set_target_height(h_px);

        let pages = self.doc.pages();
        let page = pages.get(page_index as i32)?;
        let bitmap = page
            .render_with_config(&config)
            .context("rendering page")?;

        let width = bitmap.width().max(1) as u32;
        let height = bitmap.height().max(1) as u32;
        Ok(PageImage {
            width,
            height,
            rgba: bitmap.as_rgba_bytes(),
        })
    }
}

/// Thumbnail + page count in one pass (for the scan worker).
pub fn thumbnail_png(path: &Path, width_px: u32) -> Result<(Vec<u8>, u32)> {
    let doc = OpenDoc::open(path)?;
    let count = doc.page_count();
    let img = doc.render(0, width_px)?;
    let png = rgba_to_png(&img)?;
    Ok((png, count))
}

pub fn rgba_to_png(img: &PageImage) -> Result<Vec<u8>> {
    let im = image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone())
        .ok_or_else(|| anyhow!("rendered buffer size mismatch"))?;
    let mut buf = Cursor::new(Vec::new());
    im.write_to(&mut buf, image::ImageFormat::Png)?;
    Ok(buf.into_inner())
}
