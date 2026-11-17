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
    /// pan offset of the whole desk (space+drag, wheel) in screen px
    pan: Vec2,

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
        let mut reader = Reader {
            store,
            book,
            doc,
            page: 1,
            page_count,
            zoom: 1.0,
            page_tex_cache: Default::default(),
            annotations,
            pan: Vec2::ZERO,
            armed: None,
            editing: None,
            link_dialog: None,
            show_links_panel: false,
            error: None,
            toast_out: None,
            toast_at: 0.0,
            pending_jump: None,
        };
        reader.migrate_postit_cards();
        reader
    }

    pub fn go_to_page(&mut self, page: u32) {
        self.page = page.clamp(1, self.page_count.max(1));
    }

    /// Give legacy post-its (anchor-only) a hung card position, and stack
    /// cards on the same side so they don't overlap.
    fn migrate_postit_cards(&mut self) {
        let indexes: Vec<usize> = self
            .annotations
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                matches!(&a.anchor, Anchor::PostIt { card: None, .. })
            })
            .map(|(i, _)| i)
            .collect();
        for i in indexes {
            self.annotations[i].anchor = self.hang_off_card_pos(i);
            self.save_annotation(self.annotations[i].clone());
        }
    }

    /// Card position hanging outside the page for post-it [i] (by index):
    /// side picked by pin x, vertically centered near the pin row, nudged
    /// downward past other cards on the same side.
    fn hang_off_card_pos(&self, index: usize) -> Anchor {
        let ann = &self.annotations[index];
        let Anchor::PostIt { x, y, .. } = ann.anchor else {
            return ann.anchor.clone();
        };
        let left = x <= 0.5;
        let cx = if left { -0.18 } else { 1.02 };
        let mut cy = (y - 0.05).max(-0.25);
        for _ in 0..12 {
            let overlapping = self.annotations.iter().any(|other| {
                if let Anchor::PostIt {
                    card: Some([_, oy]),
                    x: ox,
                    ..
                } = other.anchor
                {
                    (ox <= 0.5) == left && (oy - cy).abs() < 0.14
                } else {
                    false
                }
            });
            if !overlapping {
                break;
            }
            cy += 0.16;
        }
        Anchor::PostIt {
            x,
            y,
            card: Some([cx, cy]),
        }
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
        egui::Panel::top("reader_toolbar")
            .frame(crate::theme::panel_frame())
            .show(ui, |ui| {
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
                .frame(crate::theme::panel_frame())
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
        // charcoal + dot grid backdrop, behind everything
        crate::theme::paint_canvas(ui);

        // zoom (ctrl+wheel)
        let zd = ui.input(|i| i.zoom_delta());
        if zd != 1.0 {
            self.zoom = (self.zoom * zd).clamp(0.35, 4.0);
        }

        // panning: space+drag anywhere, wheel pans too
        let editing_active = self.editing.is_some();
        let (space_down, drag_delta, primary_down) = ui.input(|i| {
            (
                i.key_down(Key::Space),
                i.pointer.delta(),
                i.pointer.primary_down(),
            )
        });
        if !editing_active {
            if space_down && primary_down && drag_delta != Vec2::ZERO {
                self.pan += drag_delta;
            }
            if zd == 1.0 {
                let wheel = ui.input(|i| i.smooth_scroll_delta());
                if wheel != Vec2::ZERO {
                    self.pan += wheel;
                }
            }
        }
        // hand cursor while space is active (not while typing)
        if !editing_active && space_down {
            ui.ctx().set_cursor_icon(if primary_down {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }

        // keyboard page nav
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

        // the page floats at the top-center of the desk, shifted by pan
        let avail = ui.max_rect();
        let page_rect = Rect::from_min_size(
            pos2(
                avail.center().x - page_size.x * 0.5 + self.pan.x,
                avail.top() + 28.0 + self.pan.y,
            ),
            page_size,
        );

        // ----- page -----
        let page_id = egui::Id::unique(("reader-page", self.book.id));
        ui.advance_cursor_after_rect(page_rect);
        let resp = ui.interact(page_rect, page_id, Sense::click());

        ui.painter().rect_filled(
            page_rect.translate(vec2(6.0, 8.0)),
            CornerRadius::same(2),
            Color32::from_black_alpha(90),
        );
        ui.painter().rect_filled(page_rect, CornerRadius::same(2), Color32::WHITE);
        ui.painter().image(
            tex.id(),
            page_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        // armed creation hint + click (clicks are suppressed while space-panning)
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
        let space_down_now = space_down;
        if resp.clicked() && !space_down_now {
            if let Some(p) = resp.hover_pos() {
                self.on_page_clicked(p, page_rect);
            }
        }

        // ----- post-its: pins + hanging cards -----
        for ann in &page_annotations {
            if let Anchor::PostIt { .. } = ann.anchor {
                self.draw_postit(ui, ann, page_rect);
            }
        }

        // ----- marginalia: chalk hanging beside the page -----
        if has_margin_notes {
            let strip_rect = Rect::from_min_size(
                pos2(page_rect.right(), page_rect.top()),
                vec2(MARGIN_STRIP_W, page_size.y),
            );
            for ann in &page_annotations {
                if let Anchor::Margin { .. } = ann.anchor {
                    self.draw_margin_note(ui, ann, strip_rect, page_rect);
                }
            }
        }

        // connectors + markers drawn last (topmost)
        self.draw_link_overlays(ui, &page_annotations, page_rect);

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
            ArmMode::PostIt => (Anchor::PostIt { x: nx, y: ny, card: None }, "yellow"),
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
        // post-its: hang the card off the pin, stacked to avoid clutter
        if matches!(ann.anchor, Anchor::PostIt { .. }) {
            let i = self.annotations.len() - 1;
            self.annotations[i].anchor = self.hang_off_card_pos(i);
            self.save_annotation(self.annotations[i].clone());
        }
        self.armed = None;
        self.editing = Some(EditingNote {
            id: ann.id,
            buf: String::new(),
        });
    }

    // ==================================================================
    // Post-its
    // ==================================================================

    /// A post-it = a red pin on the page + a yellow card hanging on the desk,
    /// joined by a faint dashed thread. The pin moves on the page; the card
    /// floats anywhere (the pan makes even far-off cards reachable).
    fn draw_postit(&mut self, ui: &mut Ui, ann: &Annotation, page_rect: Rect) {
        let Anchor::PostIt {
            x,
            y,
            card: Some([cx, cy]),
        } = ann.anchor
        else {
            return;
        };
        let card_fill = note_color_ui(&ann.color);
        let thread_color = crate::theme::postit_thread();

        let pin_pos = pos2(
            page_rect.min.x + x * page_rect.width(),
            page_rect.min.y + y * page_rect.height(),
        );
        let card = Rect::from_min_size(
            pos2(
                page_rect.min.x + cx * page_rect.width(),
                page_rect.min.y + cy * page_rect.height(),
            ),
            POSTIT_SIZE,
        );

        // ----- red pin, draggable anywhere on the page -----
        let pin_id = egui::Id::unique(("postit-pin", ann.id.clone()));
        let pin_rect = Rect::from_center_size(pin_pos, vec2(14.0, 14.0));
        ui.advance_cursor_after_rect(pin_rect);
        let pin_resp = ui.interact(pin_rect, pin_id, Sense::click_and_drag());

        // ----- pencil thread, quite transparent -----
        if !card.contains(pin_pos) {
            let attach = edge_point_toward(card, pin_pos);
            let seed = egui::util::hash((ann.id.clone(), "thread"));
            links::hand_dashed_line(&ui.painter().clone(), pin_pos, attach, seed, thread_color);
        }
        let pin_radius = if pin_resp.hovered() || pin_resp.dragged() { 6.5 } else { 4.5 };
        ui.painter().circle_filled(pin_pos, pin_radius, crate::theme::PIN_RED);
        if pin_resp.hovered() || pin_resp.dragged() {
            ui.painter().text(
                pin_pos + vec2(8.0, -12.0),
                Align2::LEFT_BOTTOM,
                "drag pin",
                FontId::proportional(10.0),
                Color32::from_rgb(150, 150, 150),
            );
        }

        if self.editing.as_ref().map(|e| e.id == ann.id).unwrap_or(false) {
            let mut editing = self.editing.take().unwrap();
            let edit_id = egui::Id::unique(("edit-note", editing.id.clone()));
            ui.painter().rect_filled(card, CornerRadius::same(3), card_fill);
            ui.painter().rect_stroke(
                card,
                CornerRadius::same(3),
                Stroke::new(2.0, crate::theme::PIN_RED),
                egui::StrokeKind::Middle,
            );
            let mut buf = editing.buf.clone();
            let dark_ink = Color32::from_rgb(45, 45, 45);
            let mut layouter =
                |ui: &Ui, text: &dyn egui::TextBuffer, width: f32| -> std::sync::Arc<egui::Galley> {
                    crate::theme::layout_note(ui, text.as_str(), width, dark_ink)
                };
            let te = egui::TextEdit::multiline(&mut buf)
                .desired_width(card.width() - 14.0)
                .frame(egui::Frame::NONE)
                .layouter(&mut layouter)
                .id(edit_id);
            let resp = ui.put(card.shrink2(vec2(7.0, 5.0)), te);
            editing.buf = buf;
            ui.memory_mut(|m| m.request_focus(edit_id));
            ui.painter().text(
                pos2(card.left(), card.bottom() + 12.0),
                Align2::LEFT_TOP,
                "Esc or click away to save",
                FontId::proportional(10.0),
                Color32::from_rgb(140, 140, 145),
            );
            if editor_should_commit(ui, &resp, card) {
                let mut saved = self.fetch_annotation(&ann.id).unwrap_or_else(|| ann.clone());
                saved.body = editing.buf.clone();
                saved.updated_at = now_secs();
                self.save_annotation(saved);
            } else {
                self.editing = Some(editing);
            }
            return;
        }

        // ----- the card itself -----
        let card_id = egui::Id::unique(("postit", ann.id.clone()));
        ui.advance_cursor_after_rect(card);
        let resp = ui.interact(card, card_id, Sense::click_and_drag());
        let r = card;

        ui.painter()
            .rect_filled(r.translate(vec2(2.0, 3.0)), CornerRadius::same(3), Color32::from_black_alpha(38));
        ui.painter().rect_filled(r, CornerRadius::same(3), card_fill);
        ui.painter()
            .rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0, border_color_ui(&ann.color)), egui::StrokeKind::Middle);

        if !ann.body.trim().is_empty() {
            let g = crate::theme::layout_note(
                ui,
                ann.body.as_str(),
                r.width() - 12.0,
                Color32::from_rgb(40, 40, 40),
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

        // hover bar (stays up while the pointer is over the bar itself)
        self.note_hover_bar(ui, ann, r, resp.hovered());

        // drag the card anywhere on the desk
        if resp.dragged() {
            let delta = resp.drag_delta();
            let mut moved = self.fetch_annotation(&ann.id).unwrap_or_else(|| ann.clone());
            if let Anchor::PostIt {
                card: Some([cx, cy]),
                ..
            } = &mut moved.anchor
            {
                *cx += delta.x / page_rect.width();
                *cy += delta.y / page_rect.height();
            }
            self.save_annotation(moved);
        }
        // drag the pin to re-target (stays on the page)
        if pin_resp.dragged() {
            let delta = pin_resp.drag_delta();
            let mut moved = self.fetch_annotation(&ann.id).unwrap_or_else(|| ann.clone());
            if let Anchor::PostIt { x, y, .. } = &mut moved.anchor {
                *x = (*x + delta.x / page_rect.width()).clamp(0.0, 1.0);
                *y = (*y + delta.y / page_rect.height()).clamp(0.0, 1.0);
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
        // marginalia = white chalk handwriting floating on the desk.
        // No card. The note's color only shows in the connector + anchor dot.
        let ink = crate::theme::NOTE_CHALK;
        let thread = note_color_ui(&ann.color);
        let Anchor::Margin { y } = ann.anchor else { return };

        let w = strip.width();
        let y_screen = strip.top() + y * page_rect.height();

        // measure the text so the interaction area fits the handwriting
        let probe_text = if ann.body.is_empty() {
            "…".to_string()
        } else {
            ann.body.clone()
        };
        let probe = crate::theme::layout_note(ui, probe_text.as_str(), w - 12.0, ink);
        let h = (probe.size().y + 8.0).max(42.0);
        let rect = Rect::from_min_size(pos2(strip.left() + 4.0, y_screen), vec2(w - 12.0, h));

        // the anchor dot on the page edge is itself draggable: grabbing it
        // re-targets which passage this marginalia points at
        let anchor_pt = pos2(page_rect.left(), y_screen);
        let dot_id = egui::Id::unique(("margin-anchor", ann.id.clone()));
        let dot_rect = Rect::from_center_size(anchor_pt, vec2(14.0, 14.0));
        ui.advance_cursor_after_rect(dot_rect);
        let dot_resp = ui.interact(dot_rect, dot_id, Sense::click_and_drag());

        // connector thread from page-edge anchor to the text — a pencil
        // dotted trail, wobbling deterministically per note
        let to = pos2(rect.left() - 6.0, rect.top() + 12.0);
        let seed = egui::util::hash(ann.id.clone());
        links::hand_dashed_line(&ui.painter().clone(), anchor_pt, to, seed, crate::theme::CONNECTOR);
        let dot_radius = if dot_resp.hovered() || dot_resp.dragged() { 6.0 } else { 4.0 };
        ui.painter().circle_filled(anchor_pt, dot_radius, thread);
        if dot_resp.hovered() || dot_resp.dragged() {
            ui.painter().text(
                pos2(anchor_pt.x - 8.0, anchor_pt.y - 14.0),
                Align2::RIGHT_CENTER,
                "drag to re-target",
                FontId::proportional(10.0),
                Color32::from_rgb(140, 140, 145),
            );
        }

        if self.editing.as_ref().map(|e| e.id == ann.id).unwrap_or(false) {
            let mut editing = self.editing.take().unwrap();
            let edit_id = egui::Id::unique(("edit-note", editing.id.clone()));
            // only a ghost outline marks the editing area — the words float
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(3),
                Stroke::new(1.0, Color32::from_white_alpha(45)),
                egui::StrokeKind::Middle,
            );
            let mut buf = editing.buf.clone();
            let mut layouter =
                |ui: &Ui, text: &dyn egui::TextBuffer, width: f32| -> std::sync::Arc<egui::Galley> {
                    crate::theme::layout_note(ui, text.as_str(), width, ink)
                };
            let te = egui::TextEdit::multiline(&mut buf)
                .desired_width(rect.width())
                .frame(egui::Frame::NONE)
                .layouter(&mut layouter)
                .id(edit_id);
            let resp = ui.put(rect, te);
            editing.buf = buf;
            ui.memory_mut(|m| m.request_focus(edit_id));
            ui.painter().text(
                pos2(rect.left(), rect.bottom() + 12.0),
                Align2::LEFT_TOP,
                "Esc or click away to save",
                FontId::proportional(10.0),
                Color32::from_rgb(140, 140, 145),
            );
            if editor_should_commit(ui, &resp, rect) {
                let mut saved = self.fetch_annotation(&ann.id).unwrap_or_else(|| ann.clone());
                saved.body = editing.buf.clone();
                saved.updated_at = now_secs();
                self.save_annotation(saved);
            } else {
                self.editing = Some(editing);
            }
            return;
        }

        // interaction at the text's exact position, with a stable id
        let card_id = egui::Id::unique(("margin", ann.id.clone()));
        ui.advance_cursor_after_rect(rect);
        let resp = ui.interact(rect, card_id, Sense::click_and_drag());
        let r = rect;

        if ann.body.trim().is_empty() {
            ui.painter().text(
                r.left_bottom() + vec2(2.0, -2.0),
                Align2::LEFT_BOTTOM,
                "double-click to write",
                FontId::proportional(11.0),
                Color32::from_rgb(90, 95, 104),
            );
        } else {
            let g = crate::theme::layout_note(ui, ann.body.as_str(), r.width(), ink);
            ui.painter().galley(r.min, g, ink);
        }

        // hover bar (stays up while the pointer is over the bar itself)
        self.note_hover_bar(ui, ann, r, resp.hovered());

        // vertical drag re-anchors the note height (card OR its anchor dot)
        if resp.dragged() {
            let mut moved = self.fetch_annotation(&ann.id).unwrap_or_else(|| ann.clone());
            if let Anchor::Margin { y } = &mut moved.anchor {
                *y += resp.drag_delta().y / page_rect.height();
            }
            self.save_annotation(moved);
        }
        if dot_resp.dragged() {
            let mut moved = self.fetch_annotation(&ann.id).unwrap_or_else(|| ann.clone());
            if let Anchor::Margin { y } = &mut moved.anchor {
                *y += dot_resp.drag_delta().y / page_rect.height();
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

    fn note_hover_bar(&mut self, ui: &mut Ui, ann: &Annotation, rect: Rect, card_hovered: bool) {
        let bar_y = (rect.min.y - BAR_H - 4.0).max(4.0);
        let bar = Rect::from_min_size(pos2(rect.min.x, bar_y), vec2(rect.width(), BAR_H));

        // keep the bar visible while the pointer is anywhere on card or bar
        let bar_hovered = ui.rect_contains_pointer(bar) || ui.rect_contains_pointer(rect);
        if !(card_hovered || bar_hovered) {
            return;
        }
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
                    let seed = egui::util::hash((src.clone(), dst.clone()));
                    links::hand_dashed_line(
                        &ui.painter().clone(),
                        card_focus(src_ann, page_rect),
                        card_focus(dst_ann, page_rect),
                        seed,
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

/// Inline note editors commit on: Esc, focus loss, or a click outside the card.
fn editor_should_commit(ui: &Ui, resp: &egui::Response, rect: Rect) -> bool {
    let escaped = ui.input(|i| i.key_pressed(Key::Escape));
    if escaped {
        return true;
    }
    if resp.lost_focus() {
        return true;
    }
    let (released, pos) = ui.input(|i| (i.pointer.primary_released(), i.pointer.latest_pos()));
    released && matches!(pos, Some(p) if !rect.expand(12.0).contains(p))
}

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
        Anchor::PostIt { x, y, .. } => pos2(
            page_rect.min.x + x * page_rect.width(),
            page_rect.min.y + y * page_rect.height(),
        ),
        Anchor::Margin { y } => pos2(
            page_rect.min.x - 2.0,
            page_rect.min.y + y * page_rect.height(),
        ),
    }
}

/// Where a line from `from` to the center of `rect` first touches `rect`
/// (the thread should end on the card's border, not inside it).
fn edge_point_toward(rect: Rect, from: Pos2) -> Pos2 {
    if rect.contains(from) {
        return from;
    }
    let center = rect.center();
    let seg = center - from;
    let mut lo = 0.0f32;
    let mut hi = 1.0f32;
    for _ in 0..14 {
        let mid = (lo + hi) * 0.5;
        let q = from + seg * mid;
        if rect.contains(q) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    from + seg * hi
}
