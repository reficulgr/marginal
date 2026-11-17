//! Reader screen: page canvas + annotation layer.
//!
//! Layout model: the page is drawn at some scale; annotations live in
//! *normalized* page coordinates (0..1) so they survive zoom changes.
//! Post-its overlay the page; marginalia hang in a margin strip beside it,
//! with a connector line drawn to their anchor point on the page edge.

use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontId, Key, Pos2, Rect, ScrollArea, Sense, Stroke,
    Ui, Vec2,
};

use crate::links::{self, LinkDialog};
use crate::model::*;
use crate::pdf::{OpenDoc, PageImage};
use crate::store::SharedStore;

const POSTIT_SIZE: Vec2 = vec2(150.0, 110.0);
const MARGIN_STRIP_W: f32 = 250.0;
const BAR_H: f32 = 18.0;

#[derive(Clone, Copy, PartialEq)]
pub enum ArmMode {
    PostIt,
    Margin,
}

pub struct EditingNote {
    pub id: AnnotationId,
    pub buf: String,
}

pub struct PageTex {
    pub tex: egui::TextureHandle,
}

pub struct Reader {
    pub store: SharedStore,
    pub book: BookMeta,
    pub doc: Option<OpenDoc>,
    pub page: u32, // 1-based
    pub page_count: u32,
    pub zoom: f32,

    page_tex_cache: std::collections::HashMap<u32, PageTex>,
    annotations: Vec<Annotation>,

    /// armed "click on page to add" mode
    armed: Option<ArmMode>,
    editing: Option<EditingNote>,
    link_dialog: Option<LinkDialog>,
    show_links_panel: bool,
    error: Option<String>,
    pub toast_out: Option<String>,
    toast_at: f64,
    /// App-level nav request: (book_id, page) — consumed by App each frame.
    pub pending_jump: Option<(i64, u32)>,
}

impl Reader {
    pub fn open(store: SharedStore, book: BookMeta) -> Reader {
        let doc = OpenDoc::open(std::path::Path::new(&book.path)).ok();
        let page_count = doc.as_ref().map(|d| d.page_count()).unwrap_or(1).max(1);
        let annotations = crate::store::with_store(&store, |s| s.annotations_for_book(book.id))
            .unwrap_or_default();
        crate::store::with_store(&store, |s| s.touch_opened(book.id)).ok();
        Reader {
            store,
            book,
            doc,
            page: 1,
            page_count,
            zoom: 1.0,
            page_tex_cache: Default::default(),
            annotations,
            armed: None,
            editing: None,
            link_dialog: None,
            show_links_panel: false,
            error: None,
            toast_out: None,
            toast_at: 0.0,
            pending_jump: None,
        }
    }

    pub fn go_to_page(&mut self, page: u32) {
        self.page = page.clamp(1, self.page_count.max(1));
    }

    fn fetch_annotation(&self, id: &str) -> Option<Annotation> {
        self.annotations.iter().find(|a| a.id == id).cloned()
    }

    fn save_annotation(&mut self, ann: Annotation) {
        crate::store::with_store(&self.store, |s| {
            s.update_annotation(
                &ann.id,
                &ann.body,
                &ann.color,
                ann.page,
                &ann.anchor,
                ann.updated_at,
            )
        })
        .ok();
        if let Some(slot) = self.annotations.iter_mut().find(|a| a.id == ann.id) {
            *slot = ann;
        }
    }

    // ==================================================================
    // Frame entry
    // ==================================================================

    pub fn show(&mut self, ui: &mut Ui, on_back: &mut dyn FnMut()) {
        // error toast (bottom strip)
        if let Some(err) = self.error.clone() {
            egui::Panel::bottom("reader_error")
                .frame(egui::Frame::default().fill(Color32::from_rgb(90, 35, 35)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("⚠").color(Color32::WHITE));
                        ui.label(egui::RichText::new(&err).color(Color32::WHITE));
                        if ui.small_button("x").clicked() {
                            self.error = None;
                        }
                    });
                });
        }

        // toolbar
        egui::Panel::top("reader_toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("← Library").clicked() {
                    on_back();
                }
                ui.separator();

                let prev = ui.button("◀").clicked();
                ui.label(format!("{}/{}", self.page, self.page_count));
                let next = ui.button("▶").clicked();
                if prev {
                    self.page = self.page.saturating_sub(1).max(1);
                }
                if next {
                    self.page = (self.page + 1).min(self.page_count.max(1));
                }

                ui.separator();
                if ui.button("–").clicked() {
                    self.zoom = (self.zoom / 1.15).max(0.35);
                }
                if ui.button("＋").clicked() {
                    self.zoom = (self.zoom * 1.15).min(4.0);
                }
                if ui.button("1:1").clicked() {
                    self.zoom = 1.0;
                }

                ui.separator();
                if ui
                    .button(if self.armed == Some(ArmMode::PostIt) {
                        "📌 click page…"
                    } else {
                        "📌 Post-it"
                    })
                    .clicked()
                {
                    self.armed = if self.armed == Some(ArmMode::PostIt) {
                        None
                    } else {
                        Some(ArmMode::PostIt)
                    };
                }
                if ui
                    .button(if self.armed == Some(ArmMode::Margin) {
                        "🗒 click page…"
                    } else {
                        "🗒 Marginalia"
                    })
                    .clicked()
                {
                    self.armed = if self.armed == Some(ArmMode::Margin) {
                        None
                    } else {
                        Some(ArmMode::Margin)
                    };
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(if self.show_links_panel { "✒ Links ×" } else { "✒ Links" })
                        .clicked()
                    {
                        self.show_links_panel = !self.show_links_panel;
                    }
                    ui.label(egui::RichText::new(&self.book.title).weak());
                });
            });
            ui.add_space(4.0);
        });

        // links side panel
        if self.show_links_panel {
            egui::Panel::right("reader_links")
                .resizable(true)
                .default_size(320.0)
                .show(ui, |ui| {
                    self.links_panel(ui);
                });
        }

        // canvas
        egui::CentralPanel::default().show(ui, |ui| {
            self.canvas(ui);
        });
    }

    // ==================================================================
    // Canvas
    // ==================================================================

    fn canvas(&mut self, ui: &mut Ui) {
        // zoom: ctrl+wheel
        let zd = ui.input(|i| i.zoom_delta());
        if zd != 1.0 {
            self.zoom = (self.zoom * zd).clamp(0.35, 4.0);
        }
        // keyboard nav
        let (pgup, pgdn, home, end) = ui.input(|i| {
            (
                i.key_pressed(Key::PageUp) || i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::PageDown) || i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::Home),
                i.key_pressed(Key::End),
            )
        });
        if pgup {
            self.page = self.page.saturating_sub(1).max(1);
        }
        if pgdn {
            self.page = (self.page + 1).min(self.page_count.max(1));
        }
        if home {
            self.page = 1;
        }
        if end {
            self.page = self.page_count.max(1);
        }

        let tex = self.ensure_page_texture(ui.ctx());

        let Some(tex) = tex else {
            ui.centered_and_justified(|ui| {
                let msg: String = if self.doc.is_none() {
                    format!("Could not open PDF:\n{}", self.book.path)
                } else {
                    "Rendering page…".to_string()
                };
                ui.label(msg);
            });
            return;
        };

        let page_pts = self
            .doc
            .as_ref()
            .map(|d| d.page_size(self.page - 1))
            .unwrap_or(Ok((595.0, 842.0)))
            .unwrap_or((595.0, 842.0));

        let avail_h = ui.available_height() - 24.0;
        let scale = (avail_h / page_pts.1).max(0.05) * self.zoom;
        let page_size = vec2(page_pts.0 * scale, page_pts.1 * scale);

        let page_annotations: Vec<Annotation> = self
            .annotations
            .iter()
            .filter(|a| a.page == self.page)
            .cloned()
            .collect();
        let has_margin_notes = page_annotations
            .iter()
            .any(|a| matches!(a.anchor, Anchor::Margin { .. }));
        let strip = if has_margin_notes { MARGIN_STRIP_W } else { 0.0 };

        let mut page_rect_opt: Option<Rect> = None;

        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(12.0);
                let left_space = ((ui.available_width() - page_size.x - strip) / 2.0).max(24.0);
                ui.horizontal(|ui| {
                    ui.add_space(left_space);

                    // ----- page -----
                    let (page_rect, resp) = ui.allocate_exact_size(page_size, Sense::click());
                    page_rect_opt = Some(page_rect);

                    ui.painter().rect_filled(
                        page_rect.translate(vec2(6.0, 8.0)),
                        CornerRadius::same(2),
                        Color32::from_black_alpha(26),
                    );
                    ui.painter().rect_filled(page_rect, CornerRadius::same(2), Color32::WHITE);
                    ui.painter().image(
                        tex.id(),
                        page_rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );

                    // armed creation hint + click
                    if self.armed.is_some() {
                        if let Some(p) = resp.hover_pos() {
                            let hint = match self.armed {
                                Some(ArmMode::PostIt) => "📌 pin here",
                                Some(ArmMode::Margin) => "🗒 place here",
                                None => "",
                            };
                            ui.painter().text(
                                p + vec2(0.0, -14.0),
                                Align2::CENTER_CENTER,
                                hint,
                                FontId::proportional(13.0),
                                Color32::from_rgb(70, 70, 70),
                            );
                        }
                    }
                    if resp.clicked() {
                        if let Some(p) = resp.hover_pos() {
                            self.on_page_clicked(p, page_rect);
                        }
                    }

                    // ----- post-its on the page -----
                    for ann in &page_annotations {
                        if let Anchor::PostIt { x, y } = ann.anchor {
                            let card_min =
                                page_rect.min + vec2(x * page_rect.width(), y * page_rect.height());
                            self.draw_postit(
                                ui,
                                ann,
                                card_min,
                                page_rect,
                            );
                        }
                    }
                });

                // ----- margin strip (vertically aligned with page) -----
                if has_margin_notes {
                    ui.allocate_ui(egui::vec2(strip, page_size.y + 12.0), |ui| {
                        let strip_rect = ui.available_rect_before_wrap();
                        if let Some(pr) = page_rect_opt {
                            for ann in &page_annotations {
                                if let Anchor::Margin { .. } = ann.anchor {
                                    self.draw_margin_note(ui, ann, strip_rect, pr);
                                }
                            }
                        }
                    });
                }
            });

        // connectors + markers drawn last (topmost)
        if let Some(pr) = page_rect_opt {
            self.draw_link_overlays(ui, &page_annotations, pr);
        }

        // link picker dialog
        if let Some(mut dialog) = self.link_dialog.take() {
            links::show_dialog(ui.ctx(), self.store.clone(), &mut dialog);
            if !dialog.open {
                if let Some((src, target)) = dialog.result.take() {
                    crate::store::with_store(&self.store, |s| s.insert_link(&src, &target)).ok();
                }
            } else {
                self.link_dialog = Some(dialog);
            }
        }
    }

    fn ensure_page_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        if let Some(t) = self.page_tex_cache.get(&self.page) {
            return Some(t.tex.clone());
        }
        let doc = self.doc.as_ref()?;
        let quality: u32 = (ctx.pixels_per_point() * 1.7)
            .max(1100.0)
            .min(2000.0) as u32;
        let img: PageImage = match doc.render((self.page - 1) as u32, quality) {
            Ok(img) => img,
            Err(e) => {
                self.error = Some(format!("render failed: {e}"));
                return None;
            }
        };
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [img.width as usize, img.height as usize],
            &img.rgba,
        );
        let tex = ctx.load_texture(
            format!("glossa-page-{}-{}", self.book.id, self.page),
            color,
            egui::TextureOptions::LINEAR,
        );
        self.page_tex_cache
            .insert(self.page, PageTex { tex: tex.clone() });
        while self.page_tex_cache.len() > 6 {
            let victim = self
                .page_tex_cache
                .keys()
                .find(|k| **k != self.page)
                .copied();
            match victim {
                Some(k) => {
                    self.page_tex_cache.remove(&k);
                }
                None => break,
            }
        }
        Some(tex)
    }

    fn on_page_clicked(&mut self, p: Pos2, page_rect: Rect) {
        let Some(mode) = self.armed else { return };
        let nx = ((p.x - page_rect.left()) / page_rect.width()).clamp(0.0, 1.0);
        let ny = ((p.y - page_rect.top()) / page_rect.height()).clamp(0.0, 1.0);
        let now = now_secs();
        let (anchor, default_color) = match mode {
            ArmMode::PostIt => (Anchor::PostIt { x: nx, y: ny }, "yellow"),
            ArmMode::Margin => (Anchor::Margin { y: ny }, "blue"),
        };
        let ann = Annotation {
            id: uuid::Uuid::new_v4().to_string(),
            page: self.page,
            anchor,
            body: String::new(),
            color: default_color.into(),
            created_at: now,
            updated_at: now,
        };
        crate::store::with_store(&self.store, |s| s.insert_annotation(self.book.id, &ann)).ok();
        self.annotations.push(ann.clone());
        self.armed = None;
        self.editing = Some(EditingNote {
            id: ann.id,
            buf: String::new(),
        });
    }

    // ==================================================================
    // Post-its
    // ==================================================================

    fn draw_postit(&mut self, ui: &mut Ui, ann: &Annotation, card_min_pos: Pos2, page_rect: Rect) {
        let color = note_color_ui(&ann.color);
        let border = border_color_ui(&ann.color);

        // keep the card on the page
        let clamped_min = pos2(
            card_min_pos
                .x
                .clamp(page_rect.left(), (page_rect.right() - 30.0).max(page_rect.left())),
            card_min_pos
                .y
                .clamp(page_rect.top(), (page_rect.bottom() - 30.0).max(page_rect.top())),
        );
        let rect = Rect::from_min_size(clamped_min, POSTIT_SIZE);

        if self.editing.as_ref().map(|e| e.id == ann.id).unwrap_or(false) {
            let mut editing = self.editing.take().unwrap();
            let edit_id = egui::Id::unique(("edit-note", editing.id.clone()));
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(3),
                Stroke::new(2.0, border),
                egui::StrokeKind::Middle,
            );
            let mut buf = editing.buf.clone();
            let te = egui::TextEdit::multiline(&mut buf)
                .desired_width(rect.width() - 14.0)
                .id_salt(edit_id);
            let resp = ui.put(
                rect.shrink2(vec2(7.0, 5.0)).expand2(vec2(0.0, 10.0)),
                te,
            );
            editing.buf = buf;
            ui.memory_mut(|m| m.request_focus(edit_id));
            let escaped = ui.input(|i| i.key_pressed(Key::Escape));
            let lost = resp.lost_focus()
                && ui.input(|i| i.pointer.primary_released())
                && !ui.rect_contains_pointer(rect);
            if escaped || lost {
                let mut saved = self.fetch_annotation(&ann.id).unwrap_or_else(|| ann.clone());
                saved.body = editing.buf.clone();
                saved.updated_at = now_secs();
                self.save_annotation(saved);
            } else {
                self.editing = Some(editing);
            }
            return;
        }

        let (_, resp) = ui.allocate_exact_size(POSTIT_SIZE, Sense::click_and_drag());
        let r = rect;

        ui.painter()
            .rect_filled(r.translate(vec2(2.0, 3.0)), CornerRadius::same(3), Color32::from_black_alpha(38));
        ui.painter().rect_filled(r, CornerRadius::same(3), color);
        ui.painter()
            .rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0, border), egui::StrokeKind::Middle);

        if !ann.body.trim().is_empty() {
            let g = ui.painter().layout(
                ann.body.clone(),
                FontId::proportional(12.0),
                Color32::from_rgb(40, 40, 40),
                r.width() - 12.0,
            );
            ui.painter().galley(r.min + vec2(6.0, 6.0), g, Color32::from_rgb(40, 40, 40));
        } else {
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                "double-click to write",
                FontId::proportional(11.0),
                Color32::from_rgb(120, 120, 120),
            );
        }

        if resp.hovered() {
            self.note_hover_bar(ui, ann, r);
        }

        // drag to move
        if resp.dragged() {
            let delta = resp.drag_delta();
            let mut moved = self.fetch_annotation(&ann.id).unwrap_or_else(|| ann.clone());
            if let Anchor::PostIt { x, y } = &mut moved.anchor {
                *x += delta.x / page_rect.width();
                *y += delta.y / page_rect.height();
            }
            self.save_annotation(moved);
        }
        if resp.double_clicked() {
            self.editing = Some(EditingNote {
                id: ann.id.clone(),
                buf: ann.body.clone(),
            });
        }
    }

    // ==================================================================
    // Marginalia
    // ==================================================================

    fn draw_margin_note(&mut self, ui: &mut Ui, ann: &Annotation, strip: Rect, page_rect: Rect) {
        let color = note_color_ui(&ann.color);
        let border = border_color_ui(&ann.color);
        let Anchor::Margin { y } = ann.anchor else { return };

        let w = strip.width();
        let y_screen = strip.top() + y * page_rect.height();

        // estimate card height from wrap
        let probe_text = if ann.body.is_empty() {
            "…".to_string()
        } else {
            ann.body.clone()
        };
        let probe = ui.painter().layout(
            probe_text,
            FontId::proportional(12.0),
            Color32::from_rgb(40, 40, 40),
            w - 24.0,
        );
        let h = (probe.size().y + 16.0).max(42.0);
        let rect = Rect::from_min_size(pos2(strip.left() + 12.0, y_screen), vec2(w - 24.0, h));

        // connector from page-edge anchor to the card
        let anchor_pt = pos2(page_rect.left(), y_screen);
        let to = pos2(rect.right(), rect.top() + 14.0);
        let stroke = Stroke::new(2.0, border.gamma_multiply(0.85));
        ui.painter().line(bezier(anchor_pt, to), stroke);
        ui.painter().circle_filled(anchor_pt, 4.0, border);

        if self.editing.as_ref().map(|e| e.id == ann.id).unwrap_or(false) {
            let mut editing = self.editing.take().unwrap();
            let edit_id = egui::Id::unique(("edit-note", editing.id.clone()));
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(3),
                Stroke::new(2.0, border),
                egui::StrokeKind::Middle,
            );
            let mut buf = editing.buf.clone();
            let te = egui::TextEdit::multiline(&mut buf)
                .desired_width(rect.width() - 14.0)
                .id_salt(edit_id);
            let resp = ui.put(
                rect.expand2(vec2(0.0, 80.0)).shrink2(vec2(12.0, 6.0)),
                te,
            );
            editing.buf = buf;
            ui.memory_mut(|m| m.request_focus(edit_id));
            let escaped = ui.input(|i| i.key_pressed(Key::Escape));
            let lost = resp.lost_focus() && !ui.rect_contains_pointer(rect);
            if escaped || lost {
                let mut saved = self.fetch_annotation(&ann.id).unwrap_or_else(|| ann.clone());
                saved.body = editing.buf.clone();
                saved.updated_at = now_secs();
                self.save_annotation(saved);
            } else {
                self.editing = Some(editing);
            }
            return;
        }

        let (_, resp) = ui.allocate_exact_size(rect.size(), Sense::click_and_drag());
        let r = rect;

        ui.painter()
            .rect_filled(r.translate(vec2(2.0, 2.0)), CornerRadius::same(3), Color32::from_black_alpha(30));
        ui.painter().rect_filled(r, CornerRadius::same(3), color);
        ui.painter()
            .rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0, border), egui::StrokeKind::Middle);

        if ann.body.trim().is_empty() {
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                "double-click to write",
                FontId::proportional(11.0),
                Color32::from_rgb(120, 120, 120),
            );
        } else {
            let g = ui.painter().layout(
                ann.body.clone(),
                FontId::proportional(12.0),
                Color32::from_rgb(40, 40, 40),
                r.width() - 12.0,
            );
            ui.painter().galley(r.min + vec2(6.0, 7.0), g, Color32::from_rgb(40, 40, 40));
        }

        if resp.hovered() {
            self.note_hover_bar(ui, ann, r);
        }

        // vertical drag re-anchors the note height
        if resp.dragged() {
            let mut moved = self.fetch_annotation(&ann.id).unwrap_or_else(|| ann.clone());
            if let Anchor::Margin { y } = &mut moved.anchor {
                *y += resp.drag_delta().y / page_rect.height();
            }
            self.save_annotation(moved);
        }
        if resp.double_clicked() {
            self.editing = Some(EditingNote {
                id: ann.id.clone(),
                buf: ann.body.clone(),
            });
        }
    }

    // ==================================================================
    // Shared note UI bits
    // ==================================================================

    fn note_hover_bar(&mut self, ui: &mut Ui, ann: &Annotation, rect: Rect) {
        let bar_y = (rect.min.y - BAR_H - 4.0).max(4.0);
        let bar = Rect::from_min_size(pos2(rect.min.x, bar_y), vec2(rect.width(), BAR_H));
        let bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar).invisible());
        let mut bar_ui = bar_ui;
        bar_ui.horizontal(|ui| {
            if ui.small_button("✎").on_hover_text("edit").clicked() {
                self.editing = Some(EditingNote {
                    id: ann.id.clone(),
                    buf: ann.body.clone(),
                });
            }
            if ui.small_button("🔗").on_hover_text("add link").clicked() {
                self.link_dialog = Some(links::LinkDialog::new(
                    &ann.id,
                    self.book.fingerprint.clone(),
                    self.page,
                ));
            }
            if ui.small_button("🎨").on_hover_text("cycle color").clicked() {
                let mut saved = self.fetch_annotation(&ann.id).unwrap_or_else(|| ann.clone());
                saved.color = next_note_color(&saved.color);
                saved.updated_at = now_secs();
                self.save_annotation(saved);
            }
            if ui.small_button("🗑").on_hover_text("delete").clicked() {
                crate::store::with_store(&self.store, |s| s.delete_annotation(&ann.id)).ok();
                self.annotations.retain(|a| a.id != ann.id);
            }
        });
        let n = crate::store::with_store(&self.store, |s| s.links_for(&ann.id))
            .map(|v| v.len())
            .unwrap_or(0);
        if n > 0 {
            ui.painter().text(
                pos2(bar.max.x, bar.min.y + BAR_H * 0.5),
                Align2::RIGHT_CENTER,
                format!("{n} links"),
                FontId::proportional(10.0),
                Color32::from_rgb(90, 90, 90),
            );
        }
    }

    /// Draw note↔note link curves for annotations on this page.
    fn draw_link_overlays(&mut self, ui: &mut Ui, page_annotations: &[Annotation], page_rect: Rect) {
        let links =
            crate::store::with_store(&self.store, |s| s.links_of_book(self.book.id))
                .unwrap_or_default();
        let active_src = self
            .link_dialog
            .as_ref()
            .map(|d| d.from.clone())
            .unwrap_or_default();

        for (src, _link_id, target) in &links {
            let Some(src_ann) = page_annotations.iter().find(|a| &a.id == src) else { continue };
            match target {
                LinkTarget::Annotation(dst) => {
                    let Some(dst_ann) = page_annotations.iter().find(|a| &a.id == dst) else { continue };
                    links::draw_connector(
                        &ui.painter(),
                        card_focus(src_ann, page_rect),
                        card_focus(dst_ann, page_rect),
                        Color32::from_rgb(125, 95, 170),
                    );
                }
                _ if src == &active_src => {
                    // the note being linked elsewhere (page or URL) gets a marker dot
                    ui.painter().circle_filled(
                        card_focus(src_ann, page_rect),
                        7.0,
                        Color32::from_rgba_unmultiplied(125, 95, 170, 140),
                    );
                }
                _ => {}
            }
        }
    }

    // ==================================================================
    // Links side panel
    // ==================================================================

    fn links_panel(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.heading("Links in this book");
        ui.add_space(4.0);

        let annotations = self.annotations.clone();
        let links =
            crate::store::with_store(&self.store, |s| s.links_of_book(self.book.id))
                .unwrap_or_default();

        ScrollArea::vertical().show(ui, |ui| {
            egui::CollapsingHeader::new(egui::RichText::new(format!(
                "Notes ({})",
                annotations.len()
            )))
            .default_open(true)
            .show(ui, |ui| {
                let mut nav_to: Option<u32> = None;
                for ann in &annotations {
                    let count = links.iter().filter(|(src, _, _)| src == &ann.id).count();
                    ui.horizontal(|ui| {
                        ui.painter().rect_filled(
                            Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0)),
                            CornerRadius::same(2),
                            note_color_ui(&ann.color),
                        );
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let label = format!("p.{:>3} — {}", ann.page, note_preview(&ann.body, 40));
                        if ui.button(label).on_hover_text("jump to this note's page").clicked() {
                            nav_to = Some(ann.page);
                        }
                        if count > 0 {
                            ui.weak(format!("↯{count}"));
                        }
                    });
                }
                if let Some(p) = nav_to {
                    self.go_to_page(p);
                }
            });

            ui.add_space(8.0);
            egui::CollapsingHeader::new(egui::RichText::new(format!(
                "Connections ({})",
                links.len()
            )))
            .default_open(true)
            .show(ui, |ui| {
                for (src, link_id, target) in &links {
                    let src_page = annotations
                        .iter()
                        .find(|a| a.id == *src)
                        .map(|a| a.page)
                        .unwrap_or(0);
                    ui.horizontal(|ui| {
                        ui.label(format!("p.{src_page} {}", links::describe(target)));
                        if ui.small_button("→").on_hover_text("go").clicked() {
                            self.navigate_target(target.clone());
                        }
                        if ui.small_button("✕").on_hover_text("delete link").clicked() {
                            crate::store::with_store(&self.store, |s| s.delete_link(link_id)).ok();
                            ui.ctx().request_repaint();
                        }
                    });
                }
                if links.is_empty() {
                    ui.label(egui::RichText::new("No links yet — hover a note, press 🔗.").weak());
                }
            });
            ui.add_space(80.0);
        });

        // footer: sidecar buttons
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("⬆ Export sidecar").clicked() {
                    let res = crate::store::with_store(&self.store, |s| {
                        let book = s.book_by_id(self.book.id)?;
                        Ok(crate::sidecar::export(s, &book)?) as anyhow::Result<std::path::PathBuf>
                    });
                    match res {
                        Ok(path) => {
                            self.toast_at = ui.input(|i| i.time);
                            self.toast_out = Some(format!("exported: {}", path.display()));
                        }
                        Err(e) => self.error = Some(format!("export failed: {e}")),
                    }
                }
                if ui.button("⬇ Import sidecar").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Glossa sidecar", &["json"])
                        .pick_file()
                    {
                        let res =
                            crate::store::with_store(&self.store, |s| crate::sidecar::import(s, &path));
                        match res {
                            Ok((a, u, l)) => {
                                self.reload_annotations();
                                self.toast_at = ui.input(|i| i.time);
                                self.toast_out =
                                    Some(format!("imported: +{a} new, ~{u} updated, +{l} links"));
                            }
                            Err(e) => self.error = Some(format!("import failed: {e}")),
                        }
                    }
                }
            });
            if let Some(t) = self.toast_out.clone() {
                ui.label(egui::RichText::new(t).small().color(Color32::from_rgb(50, 110, 50)));
                let now = ui.input(|i| i.time);
                if now - self.toast_at > 8.0 {
                    self.toast_out = None;
                }
            }
        });
    }

    fn reload_annotations(&mut self) {
        self.annotations =
            crate::store::with_store(&self.store, |s| s.annotations_for_book(self.book.id))
                .unwrap_or_default();
    }

    fn navigate_target(&mut self, target: LinkTarget) {
        match target {
            LinkTarget::Page { book, page } => {
                if book == self.book.fingerprint {
                    self.go_to_page(page);
                } else if let Some(other) =
                    crate::store::with_store(&self.store, |s| s.book_by_fingerprint(&book))
                        .ok()
                        .flatten()
                {
                    self.pending_jump = Some((other.id, page));
                }
            }
            LinkTarget::Annotation(id) => {
                let found = crate::store::with_store(&self.store, |s| {
                    for b in s.books()? {
                        let anns = s.annotations_for_book(b.id)?;
                        if let Some(a) = anns.iter().find(|a| a.id == *id) {
                            return Ok::<_, anyhow::Error>(Some((b, a.page)));
                        }
                    }
                    Ok(None)
                })
                .ok()
                .flatten();
                if let Some((b, p)) = found {
                    if b.id == self.book.id {
                        self.go_to_page(p);
                    } else {
                        self.pending_jump = Some((b.id, p));
                    }
                }
            }
            LinkTarget::Url(url) => {
                let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

pub fn note_color_ui(name: &str) -> Color32 {
    let [r, g, b] = note_color_parts(name);
    Color32::from_rgb(r, g, b)
}

pub fn border_color_ui(name: &str) -> Color32 {
    let [r, g, b] = darker_parts(note_color_parts(name), 0.72);
    Color32::from_rgb(r, g, b)
}

fn note_preview(body: &str, max: usize) -> String {
    let one = body.lines().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        one.chars().take(max).collect::<String>() + "…"
    }
}

fn card_focus(ann: &Annotation, page_rect: Rect) -> Pos2 {
    match ann.anchor {
        Anchor::PostIt { x, y } => pos2(
            page_rect.min.x + x * page_rect.width(),
            page_rect.min.y + y * page_rect.height(),
        ),
        Anchor::Margin { y } => pos2(
            page_rect.min.x - 2.0,
            page_rect.min.y + y * page_rect.height(),
        ),
    }
}

fn bezier(from: Pos2, to: Pos2) -> Vec<Pos2> {
    let mid_y = (from.y + to.y) * 0.5;
    let p1 = pos2(from.x, mid_y);
    let p2 = pos2(to.x, mid_y);
    let mut pts = Vec::with_capacity(25);
    for i in 0..=24 {
        let t = i as f32 / 24.0;
        let mt = 1.0 - t;
        let x = mt * mt * mt * from.x + 3.0 * mt * mt * t * p1.x + 3.0 * mt * t * t * p2.x + t * t * t * to.x;
        let y = mt * mt * mt * from.y + 3.0 * mt * mt * t * p1.y + 3.0 * mt * t * t * p2.y + t * t * t * to.y;
        pts.push(pos2(x, y));
    }
    pts
}
