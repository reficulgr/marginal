//! Library screen: folder browser grid with thumbnails.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, ScrollArea, Sense, Ui, Vec2};

use crate::model::*;
use crate::store::SharedStore;

const TILE: Vec2 = vec2(180.0, 250.0);
const THUMB_MAX: Vec2 = vec2(160.0, 200.0);

pub struct Library {
    pub store: SharedStore,
    pub folder: Option<String>,
    tex_cache: std::collections::HashMap<i64, egui::TextureHandle>,
    pub error: Option<String>,
    /// set on click in UI; App consumes and opens the reader
    pub open_request: Option<i64>,
    /// set when the toolbar asks for a rescan; App consumes and spawns a scan
    pub rescan_request: Option<String>,
    /// set when "choose folder" returns; App consumes, stores meta, spawns scan
    pub folder_request: Option<String>,
}

impl Library {
    pub fn new(store: SharedStore) -> Library {
        Library {
            store,
            folder: None,
            tex_cache: Default::default(),
            error: None,
            open_request: None,
            rescan_request: None,
            folder_request: None,
        }
    }

    pub fn set_folder(&mut self, folder: String) {
        self.folder = Some(folder);
    }

    /// Decode + cache thumbnails from DB blobs; cheap after first frame.
    fn sync_thumbs(&mut self, ctx: &egui::Context) -> Vec<BookMeta> {
        let books = crate::store::with_store(&self.store, |s| s.books()).unwrap_or_default();
        for b in &books {
            if self.tex_cache.contains_key(&b.id) {
                continue;
            }
            if let Some(Some(png)) =
                crate::store::with_store(&self.store, |s| s.thumb(b.id)).ok()
            {
                if let Ok(img) = image::load_from_memory(&png) {
                    let rgba = img.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let ci = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                    let tex =
                        ctx.load_texture(format!("marginal-thumb-{}", b.id), ci, egui::TextureOptions::LINEAR);
                    self.tex_cache.insert(b.id, tex);
                }
            }
        }
        books
    }

    pub fn show(&mut self, ui: &mut Ui, status: &crate::app::ScanStatus) {
        // toolbar
        egui::Panel::top("lib_toolbar")
            .frame(crate::theme::panel_frame())
            .show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("📚 Marginal");
                ui.separator();
                if ui.button("📁 Choose folder…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.folder_request = Some(dir.to_string_lossy().into_owned());
                    }
                }
                if let Some(f) = self.folder.clone() {
                    if ui.button("⟳ Scan").clicked() {
                        self.rescan_request = Some(f.clone());
                    }
                    ui.add_sized([460.0, 18.0], egui::Label::new(egui::RichText::new(&f).weak()).truncate());
                } else {
                    ui.label(egui::RichText::new("Pick a folder of books to begin").weak());
                }
            });
            ui.add_space(6.0);
        });

        // error window
        if let Some(e) = self.error.clone() {
            let mut open = true;
            egui::Window::new("Problems")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                        ui.monospace(&e);
                    });
                });
            if !open {
                self.error = None;
            }
        }

        // status strip
        egui::Panel::bottom("lib_status")
            .frame(crate::theme::panel_frame())
            .show(ui, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                match status {
                    crate::app::ScanStatus::Idle => {
                        let count = crate::store::with_store(&self.store, |s| s.books())
                            .map(|b| b.len())
                            .unwrap_or(0);
                        ui.label(format!("{count} books in library"));
                    }
                    crate::app::ScanStatus::Scanning { done, total, current } => {
                        ui.spinner();
                        ui.label(format!("scanning {done}/{total} — {current}"));
                    }
                    crate::app::ScanStatus::Thumbs => {
                        ui.spinner();
                        ui.label("rendering thumbnails…");
                    }
                    crate::app::ScanStatus::Done(errors) => {
                        let count = crate::store::with_store(&self.store, |s| s.books())
                            .map(|b| b.len())
                            .unwrap_or(0);
                        ui.label(format!("{count} books indexed"));
                        if !errors.is_empty() && ui.button(format!("⚠ {}", errors.len())).clicked() {
                            self.error = Some(errors.join("\n"));
                        }
                    }
                }
            });
            ui.add_space(3.0);
        });

        // grid
        egui::CentralPanel::default().show(ui, |ui| {
            crate::theme::paint_canvas(ui);

            let books = self.sync_thumbs(ui.ctx());
            if books.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(80.0);
                        ui.label(
                            egui::RichText::new("Pick a folder 📁 to build your library").heading(),
                        );
                        ui.label(
                            "PDF support is live; EPUB/MOBI/DJVU files are indexed and slated for later.",
                        );
                    });
                });
                return;
            }

            ScrollArea::both().show(ui, |ui| {
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    for book in &books {
                        self.tile(ui, book);
                    }
                });
            });
        });
    }

    fn tile(&mut self, ui: &mut Ui, book: &BookMeta) {
        let (rect, resp) = ui.allocate_exact_size(TILE, Sense::click());

        // background
        let bg = if resp.hovered() {
            Color32::from_rgb(62, 66, 74)
        } else {
            Color32::from_rgb(52, 56, 63)
        };
        ui.painter().rect_filled(rect, CornerRadius::same(8), bg);
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(8),
            egui::Stroke::new(1.0, Color32::from_rgb(80, 84, 92)),
            egui::StrokeKind::Middle,
        );

        // thumbnail
        let thumb_rect = Rect::from_min_size(
            pos2(rect.left() + (TILE.x - THUMB_MAX.x) / 2.0, rect.top() + 10.0),
            THUMB_MAX,
        );
        if let Some(tex) = self.tex_cache.get(&book.id) {
            let size = tex.size_vec2();
            let scale = ((thumb_rect.width() / size.x as f32)
                .min(thumb_rect.height() / size.y as f32))
            .min(1.0);
            let dst = vec2(size.x as f32 * scale, size.y as f32 * scale);
            let img_rect = Rect::from_min_size(
                pos2(
                    thumb_rect.left() + (thumb_rect.width() - dst.x) / 2.0,
                    thumb_rect.top(),
                ),
                dst,
            );
            ui.painter().rect_filled(
                img_rect.translate(vec2(3.0, 4.0)),
                CornerRadius::same(2),
                Color32::from_black_alpha(40),
            );
            ui.painter().image(
                tex.id(),
                img_rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            ui.painter()
                .rect_filled(thumb_rect, CornerRadius::same(2), Color32::from_rgb(64, 68, 76));
            ui.painter().text(
                thumb_rect.center(),
                Align2::CENTER_CENTER,
                book.format.label(),
                FontId::proportional(20.0),
                Color32::from_rgb(130, 134, 145),
            );
        }

        // title + meta
        ui.painter().text(
            pos2(rect.left() + 8.0, rect.top() + THUMB_MAX.y + 18.0),
            Align2::LEFT_TOP,
            title_label(&book.title),
            FontId::proportional(13.0),
            Color32::from_rgb(225, 226, 230),
        );
        let meta_line = match book.page_count {
            Some(n) => format!("{} · {}p", book.format.label(), n),
            None => book.format.label().to_string(),
        };
        ui.painter().text(
            pos2(rect.left() + 8.0, rect.bottom() - 22.0),
            Align2::LEFT_BOTTOM,
            meta_line,
            FontId::proportional(11.0),
            Color32::from_rgb(150, 153, 160),
        );

        if !book.format.renderable() {
            ui.painter().text(
                pos2(rect.right() - 8.0, rect.top() + 16.0),
                Align2::RIGHT_TOP,
                "soon",
                FontId::proportional(10.0),
                Color32::from_rgb(190, 150, 70),
            );
        }

        if resp.clicked() && !book.format.renderable() {
            self.error = Some(format!("{} files will be readable in a later milestone", book.format.label()));
        }
        if resp.clicked() && book.format.renderable() {
            self.open_request = Some(book.id);
        }
    }
}

fn title_label(title: &str) -> String {
    let t = title.replace('_', " ");
    if t.chars().count() <= 28 {
        t
    } else {
        t.chars().take(27).collect::<String>() + "…"
    }
}
