//! Reader screen: page canvas + annotation layer.
//!
//! Layout model: the page is drawn at some scale; annotations live in
//! *normalized* page coordinates (0..1) so they survive zoom changes.
//! Post-its overlay the page; marginalia hang in a margin strip beside it,
//! with a connector line drawn to their anchor point on the page edge.

use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontId, Key, Pos2, Rect, ScrollArea, Sense, Shape,
    Stroke, Ui, Vec2,
};
use egui::emath::Rot2;
use egui::epaint::TextShape;

use crate::links::{self, LinkDialog};
use crate::model::*;
use crate::pdf::{DocInfo, RenderJob, RenderResult, spawn_render_worker};
use crate::store::SharedStore;

const MARGIN_STRIP_W: f32 = 250.0;
const BAR_H: f32 = 18.0;

#[derive(Clone, Copy, PartialEq)]
pub enum ArmMode {
    PostIt,
    Margin,
    Flag,
}

pub struct EditingNote {
    pub id: AnnotationId,
    pub buf: String,
}

#[derive(Clone)]
pub struct PageTex {
    pub tex: egui::TextureHandle,
    /// pixel width it was rendered at (for the quality ladder upgrade)
    pub quality: u32,
}

pub struct Reader {
    pub store: SharedStore,
    pub book: BookMeta,
    pub doc_info: Option<DocInfo>,
    pub page: u32, // 1-based
    pub page_count: u32,
    pub zoom: f32,

    page_tex_cache: std::collections::HashMap<u32, PageTex>,
    annotations: Vec<Annotation>,
    /// pan offset of the whole desk (space+drag, wheel) in screen px
    pan: Vec2,
    /// book (facing pages) mode
    facing: bool,
    /// last time the page changed (for the scrub quality ladder)
    last_page_jump: Option<std::time::Instant>,
    /// while a ruler drag is active, keep rendering cheap previews
    scrub_until: Option<std::time::Instant>,
    /// paper grain texture, generated once
    grain: std::collections::HashMap<String, egui::TextureHandle>,
    /// blurred shadow sprite, generated once
    shadow: Option<egui::TextureHandle>,
    /// chapter marks from the pdf outline: (1-based page, title)
    chapter_marks: Vec<(u32, String)>,

    /// pages with a render job in flight, and the quality asked
    render_pending: std::collections::HashMap<u32, u32>,
    /// pages the worker could not render — never re-asked
    render_failed: std::collections::HashSet<u32>,
    /// the render worker channel (pdfium lives entirely off the UI thread)
    render_tx: Option<std::sync::mpsc::Sender<RenderJob>>,
    render_rx: Option<std::sync::mpsc::Receiver<RenderResult>>,

    /// armed "click on page to add" mode
    armed: Option<ArmMode>,
    editing: Option<EditingNote>,
    link_dialog: Option<LinkDialog>,
    /// bookmark tabs stuck on the pages' top edges (frosted slip, colored tip)
    flags: Vec<Flag>,
    /// last tip color picked — a real pack: you keep peeling the same flag
    last_flag_color: String,
    /// notes sidebar: every note & marginalia in book order, plus connections
    show_notes_panel: bool,
    /// fresh bottom edge of the toolbar — the film anchors below it
    toolbar_bottom: f32,
    error: Option<String>,
    pub toast_out: Option<String>,
    toast_at: f64,
    /// App-level nav request: (book_id, page) — consumed by App each frame.
    pub pending_jump: Option<(i64, u32)>,
}

    /// One desk marginalia seen by the margin solver.
    #[derive(Clone)]
struct MarginCol {
        id: String,
        dot_y: f32,
        body: String,
        natural_w: f32,
        max_w: f32,
    }

    /// Fit a margin column so chalk boxes never overlap. Remedy one: a box
    /// that overfills the gap to the next dot below reflows wider into the
    /// free desk — fewer rows, shorter box, top still pinned at its dot.
    /// Remedy two: whatever still collides slides down the strip, hanging
    /// from its dot by its thread. Single-row notes are pins: never
    /// widened, never moved, never overlapped — the solve stops at them
    /// and movable notes yield instead. Judged on measured text, fresh
    /// every frame; the original widths live in the notes and are never
    /// written back, so the arrangement reverts by itself once the
    /// crowding goes away.
    fn solve_margin_boxes(
        ui: &egui::Ui,
        content_bottom: f32,
        mut col: Vec<MarginCol>,
    ) -> Vec<(String, f32, f32)> {
        col.sort_by(|a, b| a.dot_y.total_cmp(&b.dot_y));
        let pad = 4.0f32;
        let ink = crate::theme::NOTE_CHALK;
        let n = col.len();

        // remedy one — widen to fit the vertical gap, stepping at row
        // breaks until the box is short enough (or reaches the one-row
        // floor, or the live window edge)
        let mut h = vec![0.0f32; n];
        let mut w = vec![0.0f32; n];
        let mut pin = vec![false; n];
        for i in 0..n {
            let next_y = if i + 1 < n { col[i + 1].dot_y } else { content_bottom };
            let gap = (next_y - col[i].dot_y - pad).max(30.0);
            let g = crate::theme::layout_note(ui, &col[i].body, col[i].natural_w, ink);
            let mut hh = (g.size().y + 8.0).max(42.0);
            let mut rows = g.rows.len();
            let mut ww = col[i].natural_w;
            if rows > 1 && hh > gap {
                while ww < col[i].max_w {
                    ww = (ww + 40.0).min(col[i].max_w);
                    let g2 = crate::theme::layout_note(ui, &col[i].body, ww, ink);
                    hh = (g2.size().y + 8.0).max(42.0);
                    rows = g2.rows.len();
                    if hh <= gap || rows <= 1 {
                        break;
                    }
                }
            }
            h[i] = hh;
            w[i] = ww;
            pin[i] = rows <= 1;
        }

        // the dot of the next single-row pin below — pins bound how far a
        // movable box may reach downward
        let mut next_pin_y = vec![f32::INFINITY; n];
        for i in (0..n).rev() {
            if pin[i] {
                next_pin_y[i] = col[i].dot_y;
            } else if i + 1 < n {
                next_pin_y[i] = next_pin_y[i + 1];
            }
        }

        // remedy two — displacement: walk top to bottom; a movable box
        // hangs from its dot but slides down when crowded, and if it would
        // poke the next pin it defers past it instead (the pin refuses)
        let mut out: Vec<(String, f32, f32)> = Vec::with_capacity(n);
        let mut cursor = f32::NEG_INFINITY;
        let mut deferred: Vec<usize> = Vec::new();
        for i in 0..n {
            if pin[i] {
                out.push((col[i].id.clone(), col[i].dot_y, w[i]));
                cursor = col[i].dot_y + h[i] + pad;
            } else {
                let top = col[i].dot_y.max(cursor);
                if next_pin_y[i] - pad < top + h[i] {
                    deferred.push(i);
                } else {
                    out.push((col[i].id.clone(), top, w[i]));
                    cursor = top + h[i] + pad;
                }
            }
        }
        for i in deferred {
            let top = col[i].dot_y.max(cursor);
            out.push((col[i].id.clone(), top, w[i]));
            cursor = top + h[i] + pad;
        }
        out
    }

impl Reader {
    pub fn open(store: SharedStore, book: BookMeta) -> Reader {
        // rendering happens entirely off the UI thread: one worker per book
        // owns the pdfium handle; the UI paints cached pixels only
        let (render_tx, render_rx) = spawn_render_worker(std::path::PathBuf::from(&book.path));
        // seed the page count from the library scan until the worker reports
        let page_count = book.page_count.unwrap_or(1).max(1);
        let annotations = crate::store::with_store(&store, |s| s.annotations_for_book(book.id))
            .unwrap_or_default();
        let flags = crate::store::with_store(&store, |s| s.flags_for_book(book.id))
            .unwrap_or_default();
        crate::store::with_store(&store, |s| s.touch_opened(book.id)).ok();
        // book-view preference
        let facing_pref = {
            let stored = crate::store::with_store(&store, |s| s.get_meta("facing"))
                .ok()
                .flatten()
                .map(|v| v == "1")
                .unwrap_or(false);
            stored || std::env::var("MARGINAL_FACING").ok().as_deref() == Some("1")
        };
        let reader = Reader {
            facing: facing_pref,
            store,
            book,
            page: 1,
            page_count,
            zoom: 1.0,
            page_tex_cache: Default::default(),
            doc_info: None,
            annotations,
            pan: Vec2::ZERO,
            last_page_jump: None,
            scrub_until: None,
            grain: Default::default(),
            shadow: None,
            chapter_marks: Vec::new(),
            render_pending: Default::default(),
            render_failed: Default::default(),
            render_tx: Some(render_tx),
            render_rx: Some(render_rx),
            armed: None,
            editing: None,
            link_dialog: None,
            flags,
            last_flag_color: "red".into(),
            show_notes_panel: false,
            toolbar_bottom: 0.0,
            error: None,
            toast_out: None,
            toast_at: 0.0,
            pending_jump: None,
        };
        
        reader
    }

    /// Toolbar toggle target: the odd (right-hand) page of the spread that
    /// contains p — or p for lone pages. In single mode, p itself.
    fn anchor_for_mode(&self, p: u32, facing: bool) -> u32 {
        let count = self.page_count.max(1);
        if !facing {
            return p.clamp(1, count);
        }
        let p = p.clamp(1, count);
        if p <= 1 {
            1
        } else if p % 2 == 1 {
            p
        } else if p + 1 <= count {
            p + 1
        } else {
            p // lone even last page
        }
    }

    /// Visible sheets in the current mode: left/right slots with book pairing.
    fn spread_pair(&self) -> (Option<u32>, Option<u32>) {
        let p = self.page;
        let count = self.page_count.max(1);
        if !self.facing {
            return (Some(p), None);
        }
        if p <= 1 {
            (None, Some(1))
        } else if p % 2 == 1 {
            (Some(p - 1), Some(p))
        } else if p + 1 <= count {
            (Some(p), Some(p + 1))
        } else {
            (Some(p), None) // lone even last page sits on the left
        }
    }

    /// Counter label for the current view.
    fn page_label(&self) -> String {
        if !self.facing {
            return format!("{}/{}", self.page, self.page_count);
        }
        match self.spread_pair() {
            (Some(l), Some(r)) => format!("{l}–{r} / {}", self.page_count),
            (None, Some(r)) => format!("{r} / {}", self.page_count),
            (Some(l), None) => format!("{l} / {}", self.page_count),
            (None, None) => String::new(),
        }
    }

    pub fn go_to_page(&mut self, page: u32) {
        self.page = self.anchor_for_mode(page.clamp(1, self.page_count.max(1)), self.facing);
        self.last_page_jump = Some(std::time::Instant::now());
    }

    /// Give legacy post-its (anchor-only) a hung card position, and stack
    /// cards on the same side so they don't overlap.
    /// Auto-hug: positions for every post-it card that has no user-set
    /// position in the CURRENT view mode. The card hangs outside whichever
    /// page edge is CLOSEST to its pin (left / right / top / bottom),
    /// stacking outward along that edge. In spread (book) view the spine
    /// side is excluded — left sheets hang {left, top, bottom},
    /// right sheets {right, top, bottom} — so nothing sits in the gutter.
    fn compute_auto_hangs(
        &self,
        page_anns: &[Annotation],
        page_rect: Rect,
        default_size: [f32; 2],
        outer_left: Option<bool>,
    ) -> std::collections::HashMap<String, [f32; 2]> {
        let mut out = std::collections::HashMap::new();
        let pw = page_rect.width().max(1.0);
        let ph = page_rect.height().max(1.0);
        let mut left_stack: Vec<f32> = Vec::new();
        let mut right_stack: Vec<f32> = Vec::new();
        let mut top_stack: Vec<f32> = Vec::new();
        let mut bottom_stack: Vec<f32> = Vec::new();

        // pass 1: register user-placed cards by which margin they occupy
        for ann in page_anns {
            let Anchor::PostIt { card, card_s, size, .. } = ann.anchor else { continue };
            let user_slot = if self.facing { card_s } else { card };
            let Some([cx, cy]) = user_slot else { continue };
            let [w, h] = size.unwrap_or(default_size);
            let w_frac = (w / pw).max(0.04);
            let h_frac = (h / ph).max(0.05);
            if cy + h_frac <= 0.07 {
                top_stack.push(cx);
            } else if cy >= 0.93 {
                bottom_stack.push(cx);
            } else if cx + w_frac <= 0.07 {
                left_stack.push(cy);
            } else if cx >= 0.93 {
                right_stack.push(cy);
            }
        }

        // pass 2: hang the rest by nearest allowed edge
        for ann in page_anns {
            let Anchor::PostIt {
                x,
                y,
                card,
                card_s,
                size,
            } = ann.anchor
            else {
                continue;
            };
            let user_slot = if self.facing { card_s } else { card };
            match user_slot {
                Some(pos) => {
                    out.insert(ann.id.clone(), pos);
                }
                None => {
                    let [w, h] = size.unwrap_or(default_size);
                    let w = w.max(default_size[0]);
                    let h = h.max(default_size[1]);
                    let w_frac = (w / pw).max(0.04);
                    let h_frac = (h / ph).max(0.05);

                    // nearest edge, spine excluded in book view
                    enum Side {
                        L,
                        R,
                        T,
                        B,
                    }
                    let d_left = x;
                    let d_right = 1.0 - x;
                    let d_top = y;
                    let d_bottom = 1.0 - y;
                    let side = match outer_left {
                        Some(true) => {
                            if d_top <= d_left.min(d_bottom) {
                                Side::T
                            } else if d_bottom <= d_left {
                                Side::B
                            } else {
                                Side::L
                            }
                        }
                        Some(false) => {
                            if d_top <= d_right.min(d_bottom) {
                                Side::T
                            } else if d_bottom <= d_right {
                                Side::B
                            } else {
                                Side::R
                            }
                        }
                        None => {
                            if d_left <= d_right && d_left <= d_top && d_left <= d_bottom {
                                Side::L
                            } else if d_right <= d_top && d_right <= d_bottom {
                                Side::R
                            } else if d_top <= d_bottom {
                                Side::T
                            } else {
                                Side::B
                            }
                        }
                    };

                    match side {
                        Side::L => {
                            let cx = -(w + 18.0) / pw;
                            let cy = stack_along_h(y, &mut left_stack, h_frac);
                            out.insert(ann.id.clone(), [cx, cy]);
                        }
                        Side::R => {
                            let cx = 1.0 + 18.0 / pw;
                            let cy = stack_along_h(y, &mut right_stack, h_frac);
                            out.insert(ann.id.clone(), [cx, cy]);
                        }
                        Side::T => {
                            let cy = -(h + 16.0) / ph;
                            let cx = stack_along_v(x, &mut top_stack, w_frac);
                            out.insert(ann.id.clone(), [cx, cy]);
                        }
                        Side::B => {
                            let cy = 1.0 + 16.0 / ph;
                            let cx = stack_along_v(x, &mut bottom_stack, w_frac);
                            out.insert(ann.id.clone(), [cx, cy]);
                        }
                    }
                }
            }
        }
        out
    }

    /// Minimum card size: ~15 characters wide by 7 lines tall (font-truthful),
    /// including the paper padding.
    fn card_min_size(&self, ui: &Ui) -> [f32; 2] {
        let probe = ui.painter().layout_no_wrap(
            "Hello world 123".to_string(),
            crate::theme::note_font(),
            Color32::TRANSPARENT,
        );
        let min_w = probe.size().x + 2.0 * crate::theme::NOTE_PAD + 4.0;
        let min_h = 7.0 * crate::theme::NOTE_LEADING + 2.0 * crate::theme::NOTE_PAD;
        [min_w.ceil(), min_h]
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
        // swap in page textures the render worker finished off-thread
        self.poll_renders(ui.ctx());

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
        let toolbar = egui::Panel::top("reader_toolbar")
            .frame(crate::theme::panel_frame())
            .show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("← Library").clicked() {
                    on_back();
                }
                ui.separator();

                let facing_label = if self.facing { "📖 ✓" } else { "📖" };
                if ui
                    .button(facing_label)
                    .on_hover_text("facing pages (book view)")
                    .clicked()
                {
                    self.facing = !self.facing;
                    self.page = self.anchor_for_mode(self.page, self.facing);
                    crate::store::with_store(&self.store, |s| {
                        s.set_meta("facing", if self.facing { "1" } else { "0" })
                    })
                    .ok();
                }
                let step = if self.facing { 2 } else { 1 };
                let prev = ui.button("◀").clicked();
                ui.label(self.page_label());
                let next = ui.button("▶").clicked();
                if prev {
                    self.page = self.page.saturating_sub(step).max(1);
                }
                if next {
                    self.page = (self.page + step).min(self.page_count.max(1));
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
                if ui
                    .button(if self.armed == Some(ArmMode::Flag) {
                        "🔖 click page…"
                    } else {
                        "🔖 Tab"
                    })
                    .on_hover_text("bookmark tab: sticks on a page's top edge, tip over the edge")
                    .clicked()
                {
                    self.armed = if self.armed == Some(ArmMode::Flag) {
                        None
                    } else {
                        Some(ArmMode::Flag)
                    };
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(if self.show_notes_panel { "🗒 Notes ×" } else { "🗒 Notes" })
                        .on_hover_text("every note & marginalia, in book order")
                        .clicked()
                    {
                        self.show_notes_panel = !self.show_notes_panel;
                    }
                    ui.label(egui::RichText::new(&self.book.title).weak());
                });
            });
            ui.add_space(4.0);
        });
        // fresh every frame: the film anchors below this edge, since the
        // panel-path layout can hold a stale window size across resizes
        self.toolbar_bottom = toolbar.response.rect.bottom();

        // canvas — full width, always; the strip is pinned UI floating
        // over its right edge. Pages, notes and the dot grid show through
        // its 20% translucency and pan beneath it like anywhere else —
        // but the strip's own contents never scroll or drag: it never
        // moves of its own accord.
        egui::CentralPanel::default().show(ui, |ui| {
            self.canvas(ui);
            if self.show_notes_panel {
                self.notes_film(ui);
            }
        });
    }

    // ==================================================================
    // Canvas
    // ==================================================================

    /// Bottom ruler: floats transparently over the desk (the dot grid shows
    /// through), ~90% of the available width, ticks every 2% of the page
    /// span (longer every 10%), red position bracket, click / drag-scrub.
    fn page_ruler(&mut self, ui: &mut Ui) {
        // same escape as the film strip: the ruler paints in its own layer
        // pinned to the fresh client rect — its clip is fresh every frame,
        // so nothing stale can drag the map off-screen
        let avail = ui.ctx().content_rect();
        let w = avail.width() * 0.9;
        let ruler = Rect::from_min_size(
            pos2(avail.left() + (avail.width() - w) * 0.5, avail.bottom() - 32.0),
            vec2(w, 24.0),
        );
        egui::Area::new(egui::Id::unique("reader-ruler-layer"))
            .order(egui::Order::Middle)
            .fixed_pos(pos2(avail.left(), ruler.top()))
            .show(ui.ctx(), |ui| {
        let ruler_id = egui::Id::unique(("reader-ruler", self.book.id));
        let resp = ui.interact(ruler, ruler_id, Sense::click_and_drag());
        let painter = ui.painter().clone();

        let count = self.page_count.max(1);
        let pad = 10.0;
        let tw = (ruler.width() - 2.0 * pad).max(1.0);
        let fx = |frac: f32| -> f32 { ruler.left() + pad + frac * tw };
        let page_at = |frac: f32| -> u32 {
            (1.0 + frac * (count as f32 - 1.0)).round().clamp(1.0, count as f32) as u32
        };

        let mut pct = 0.0f32;
        while pct <= 100.001 {
            let frac = pct / 100.0;
            let x = fx(frac);
            let major = (pct as i32) % 10 == 0;
            let tick_h = if major { 13.0 } else { 6.0 };
            let tick_c = if major {
                Color32::from_rgb(168, 172, 180)
            } else {
                Color32::from_rgb(112, 116, 124)
            };
            painter.line_segment(
                [pos2(x, ruler.bottom()), pos2(x, ruler.bottom() - tick_h)],
                Stroke::new(1.0, tick_c),
            );
            if major && count > 20 {
                painter.text(
                    pos2(x, ruler.top()),
                    Align2::CENTER_TOP,
                    page_at(frac).to_string(),
                    FontId::proportional(9.0),
                    Color32::from_rgb(150, 154, 162),
                );
            }
            pct += 2.0;
        }

        let marker_pages: Vec<u32> = if self.facing {
            let (l, r) = self.spread_pair();
            [l, r].into_iter().flatten().collect()
        } else {
            vec![self.page]
        };
        let frac_of = |page_no: u32| -> f32 {
            if count > 1 {
                ((page_no as f32 - 1.0) / (count as f32 - 1.0)).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        // chapter marks: thin amber lines, twice the 10% tick height.
        // Fractional stroke width (bright-on-dark + hi-dpi reads fatter),
        // deduped when several chapters share a start page.
        let mut last_chap_x: Option<f32> = None;
        for (chg_page, _title) in self.chapter_marks.iter() {
            if *chg_page > count {
                continue;
            }
            let x = fx(frac_of(*chg_page));
            if last_chap_x.map(|lx| (lx - x).abs() < 1.5).unwrap_or(false) {
                continue;
            }
            // twice the height of everything else on the ruler — exaggerated
            // on purpose: the chapter skeleton should read from across the room
            painter.line_segment(
                [pos2(x, ruler.bottom()), pos2(x, ruler.bottom() - 52.0)],
                Stroke::new(0.6, crate::theme::CHAPTER_MARK),
            );
            last_chap_x = Some(x);
        }

        // current position: a red hairline (0.4pt) centered over the spread
        if let (Some(first), Some(last)) = (marker_pages.first(), marker_pages.last()) {
            let mid_frac = 0.5 * (frac_of(*first) + frac_of(*last));
            let bx = fx(mid_frac);
            painter.line_segment(
                [pos2(bx, ruler.top() + 2.0), pos2(bx, ruler.bottom() - 1.0)],
                Stroke::new(0.4, crate::theme::PIN_RED),
            );
        }

        // page dots: every page holding a post-it shows the pin's red, and
        // every page holding marginalia the margin dot's light blue — both
        // stacked north of the ruler in clear sky. The tallest vertical on
        // the strip is the 52px chapter spike; the dots start above the
        // spike tips, so no line can ever reach them, and the red rides
        // low, blue hovers high, like marginalia floats above its page.
        let (postit_pages, margin_pages) = ruler_page_marks(&self.annotations);
        for page_no in &postit_pages {
            painter.circle_filled(
                pos2(fx(frac_of(*page_no)), ruler.top() - 35.0),
                4.5,
                crate::theme::PIN_RED,
            );
        }
        for page_no in &margin_pages {
            painter.circle_filled(
                pos2(fx(frac_of(*page_no)), ruler.top() - 46.0),
                3.0,
                crate::theme::MARGIN_BLUE,
            );
        }

        // bookmark tabs: small colored squares, north of the marginalia
        // circles — the closed-book skyline as tiny tiles, one per flagged
        // page, in its first-planted tab's color
        for (page_no, rgb) in ruler_flag_pages(&self.flags) {
            painter.rect_filled(
                Rect::from_center_size(
                    pos2(fx(frac_of(page_no)), ruler.top() - 56.0),
                    vec2(7.0, 7.0),
                ),
                CornerRadius::same(1),
                Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
            );
        }

        if resp.clicked() || resp.dragged() {
            if resp.dragged() {
                self.scrub_until =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(450));
            }
            if let Some(p) = resp.hover_pos().or(ui.input(|i| i.pointer.interact_pos())) {
                let frac = ((p.x - pad - ruler.left()) / tw).clamp(0.0, 1.0);
                let target = page_at(frac);
                if target != self.page {
                    self.go_to_page(target);
                    ui.ctx().request_repaint();
                }
            }
        }

        if resp.hovered() && !resp.dragged() {
            if let Some(p) = resp.hover_pos() {
                let frac = ((p.x - pad - ruler.left()) / tw).clamp(0.0, 1.0);
                let preview = page_at(frac);
                // nearest chapter mark within reach → title in the chip
                let mut chapter: Option<String> = None;
                for (chg_page, title) in self.chapter_marks.iter() {
                    let dist = (fx(frac_of(*chg_page)) - p.x).abs();
                    if dist < 10.0 {
                        let short: String = title.chars().take(26).collect::<String>();
                        chapter = Some(if title.chars().count() > 26 {
                            format!("{short}…")
                        } else {
                            short
                        });
                        break;
                    }
                }
                let (label, chip_w) = match &chapter {
                    Some(t) => (format!("p.{preview} · {t}"), 12.0 * t.len() as f32 + 34.0),
                    None => (format!("p.{preview}"), 36.0),
                };
                let label_rect = Rect::from_min_size(
                    pos2(
                        ((p.x - chip_w * 0.5).max(ruler.left())).min(ruler.right() - chip_w),
                        ruler.top() - 18.0,
                    ),
                    vec2(chip_w, 15.0),
                );
                painter.rect_filled(
                    label_rect,
                    CornerRadius::same(3),
                    Color32::from_rgb(58, 62, 70),
                );
                painter.text(
                    label_rect.center(),
                    Align2::CENTER_CENTER,
                    label,
                    FontId::proportional(9.0),
                    Color32::from_rgb(225, 227, 231),
                );
            }
        }
        });
    }

    fn canvas(&mut self, ui: &mut Ui) {
        // charcoal + dot grid backdrop, behind everything
        crate::theme::paint_canvas(ui);

        // zoom (ctrl+wheel)
        let zd = ui.input(|i| i.zoom_delta());
        if zd != 1.0 {
            self.zoom = (self.zoom * zd).clamp(0.35, 4.0);
        }

        // panning: space+drag anywhere, middle-click drag, wheel
        let editing_active = self.editing.is_some();
        let (space_down, drag_delta, primary_down, middle_down) = ui.input(|i| {
            (
                i.key_down(Key::Space),
                i.pointer.delta(),
                i.pointer.primary_down(),
                i.pointer.middle_down(),
            )
        });
        let pan_now = !editing_active && ((space_down && primary_down) || middle_down);
        if pan_now && drag_delta != Vec2::ZERO {
            self.pan += drag_delta;
        }
        if !editing_active && zd == 1.0 {
            let wheel = ui.input(|i| i.smooth_scroll_delta());
            if wheel != Vec2::ZERO {
                // the pointer over the open strip belongs to the strip:
                // its own ScrollArea consumes the scroll (and scrolls the
                // notes), so the desk must not pan from the same event.
                // (raw pointer test: ui.rect_contains_pointer would check
                // the PANEL's layer, and the pointer rests on the film's
                // Middle layer instead — it is never "contained".)
                let film = self.notes_film_rect(ui.ctx().content_rect());
                let over_strip = self.show_notes_panel
                    && ui.input(|i| {
                        i.pointer
                            .latest_pos()
                            .map(|p| film.contains(p))
                            .unwrap_or(false)
                    });
                if !over_strip {
                    self.pan += wheel;
                }
            }
        }
        if !editing_active && (space_down || middle_down) {
            ui.ctx().set_cursor_icon(if pan_now {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }

        // keyboard page nav — NOT while an inline editor is open: the
        // caret's arrows, Home/End and paging belong to the words you're
        // typing. A stray arrow mid-typing would flip the page, arm the
        // quality ladder, and stall the UI thread on a re-render — frozen
        // keystrokes, then the repeat queue flushing as a burst.
        let (pgup, pgdn, home, end) = if editing_active {
            (false, false, false, false)
        } else {
            ui.input(|i| {
                (
                    i.key_pressed(Key::PageUp) || i.key_pressed(Key::ArrowLeft),
                    i.key_pressed(Key::PageDown) || i.key_pressed(Key::ArrowRight),
                    i.key_pressed(Key::Home),
                    i.key_pressed(Key::End),
                )
            })
        };
        let step = if self.facing { 2 } else { 1 };
        if pgup {
            self.page = self.page.saturating_sub(step).max(1);
        }
        if pgdn {
            self.page = (self.page + step).min(self.page_count.max(1));
        }
        if home {
            self.page = 1;
        }
        if end {
            self.page = self.anchor_for_mode(self.page_count.max(1), self.facing);
        }

        let postit_min = self.card_min_size(ui);

        // page geometry from the anchor page (pages share the fit); in book
        // view we also budget the two outer margins so they stay on screen
        let page_pts = self
            .doc_info
            .as_ref()
            .and_then(|d| d.sizes.get(self.page.saturating_sub(1) as usize).copied())
            .unwrap_or((595.0, 842.0));
        let avail_h = ui.available_height() - 24.0;
        let scale_h = (avail_h / page_pts.1).max(0.05) * self.zoom;
        let base_scale = if self.facing {
            let outer_budget = (MARGIN_STRIP_W + 18.0) * 2.0 + 60.0;
            let scale_w = ((ui.available_width() - outer_budget) / (2.0 * page_pts.0)).max(0.02) * self.zoom;
            scale_h.min(scale_w)
        } else {
            scale_h
        };
        let page_size = vec2(page_pts.0 * base_scale, page_pts.1 * base_scale);

        // visible sheets: single = one sheet; book = left+right with a gutter
        let avail = ui.max_rect();
        let gutter = 34.0;
        let y0 = avail.top() + 28.0 + self.pan.y;
        let cx_center = avail.center().x + self.pan.x;

        struct Sheet {
            page_no: u32,
            rect: Rect,
            strip_left: bool, // marginalia hangs on the outer (left) edge
        }
        let mut sheets: Vec<Sheet> = Vec::new();
        if !self.facing {
            sheets.push(Sheet {
                page_no: self.page,
                rect: Rect::from_min_size(
                    pos2(cx_center - page_size.x * 0.5, y0),
                    page_size,
                ),
                strip_left: false,
            });
        } else {
            let (left, right) = self.spread_pair();
            match (left, right) {
                (Some(l), Some(r)) => {
                    let total = page_size.x * 2.0 + gutter;
                    let x0 = cx_center - total * 0.5;
                    sheets.push(Sheet {
                        page_no: l,
                        rect: Rect::from_min_size(pos2(x0, y0), page_size),
                        strip_left: true,
                    });
                    sheets.push(Sheet {
                        page_no: r,
                        rect: Rect::from_min_size(pos2(x0 + page_size.x + gutter, y0), page_size),
                        strip_left: false,
                    });
                    let _ = total;
                }
                (None, Some(r)) => {
                    let x = cx_center + gutter * 0.5;
                    sheets.push(Sheet {
                        page_no: r,
                        rect: Rect::from_min_size(pos2(x, y0), page_size),
                        strip_left: false,
                    });
                }
                (Some(l), None) => {
                    let x = cx_center - gutter * 0.5 - page_size.x;
                    sheets.push(Sheet {
                        page_no: l,
                        rect: Rect::from_min_size(pos2(x, y0), page_size),
                        strip_left: true,
                    });
                }
                (None, None) => {}
            }
        }

        let clicks_enabled = !pan_now;

        // hover bars render in an overlay pass after the sheets, threads
        // and ruler — egui hit-testing favors later widgets, so drawing the
        // bars last guarantees no neighbor can sit on top of them
        let mut hover_bars: Vec<(Annotation, Rect)> = Vec::new();

        // bookmark census: every tabbed page, in book order
        let tabbed: Vec<u32> = {
            let set: std::collections::BTreeSet<u32> =
                self.flags.iter().map(|f| f.page).collect();
            set.into_iter().collect()
        };

        // draw each sheet (annotations ride along, per page). The sky
        // splits at the spine: a book view's left sheet carries the tips
        // of previous pages, the right sheet the future's; single view's
        // one sheet shows the whole sky but its own page's tabs.
        let visible: Vec<u32> = sheets.iter().map(|s| s.page_no).collect();
        let mut sheet_data: Vec<(u32, Rect, Vec<Annotation>)> = Vec::new();
        for sheet in &sheets {
            let skyline: Vec<u32> = if visible.len() <= 1 {
                tabbed.iter().copied().filter(|p| *p != sheet.page_no).collect()
            } else if sheet.strip_left {
                tabbed.iter().copied().filter(|p| *p < sheet.page_no).collect()
            } else {
                tabbed.iter().copied().filter(|p| *p > sheet.page_no).collect()
            };
            let page_anns: Vec<Annotation> = self
                .annotations
                .iter()
                .filter(|a| a.page == sheet.page_no)
                .cloned()
                .collect();
            self.draw_sheet(
                ui,
                sheet.page_no,
                sheet.rect,
                sheet.strip_left,
                &page_anns,
                postit_min,
                clicks_enabled,
                &mut hover_bars,
                &skyline,
            );
            sheet_data.push((sheet.page_no, sheet.rect, page_anns));
        }

        // link overlays across visible sheets
        self.draw_link_overlays(ui, &sheet_data);

        // link picker dialog
        if let Some(mut dialog) = self.link_dialog.take() {
            links::show_dialog(ui.ctx(), self.store.clone(), &mut dialog);
            if !dialog.open {
                if let Some((src, target)) = dialog.result.take() {
                    if let Err(e) = crate::store::with_store(&self.store, |s| {
                        s.insert_link(&src, &target)
                    }) {
                        self.error = Some(format!("could not add the link: {e:#}"));
                    }
                } else {
                    self.link_dialog = Some(dialog);
                }
            }
        }

        // page ruler: floats transparently over the desk, ~90% window width
        self.page_ruler(ui);
        self.hover_bar_pass(ui, &hover_bars);
    }

    /// Overlay pass: at most one note hover bar (the bar the pointer nearest
    /// sits on), drawn after every desk widget so it can never be covered.
    fn hover_bar_pass(&mut self, ui: &mut Ui, bars: &[(Annotation, Rect)]) {
        for (ann, rect) in bars {
            self.note_hover_bar(ui, ann, *rect);
        }
    }

    /// One physical page on the desk: shadow, paper, texture, interaction,
    /// pins + cards + marginalia for that page.
    #[allow(clippy::too_many_arguments)]
    fn draw_sheet(
        &mut self,
        ui: &mut Ui,
        page_no: u32,
        page_rect: Rect,
        strip_left: bool,
        page_anns: &[Annotation],
        postit_min: [f32; 2],
        allow_clicks: bool,
        bars: &mut Vec<(Annotation, Rect)>,
        skyline: &[u32],
    ) {
        let Some(tex) = self.ensure_page_texture(ui.ctx(), page_no) else {
            // a cold page: the paper is here, the pixels are on the way
            ui.painter().rect_filled(
                page_rect.translate(vec2(6.0, 8.0)),
                CornerRadius::same(2),
                Color32::from_black_alpha(90),
            );
            ui.painter().rect_filled(page_rect, CornerRadius::same(2), Color32::from_rgb(232, 230, 223));
            ui.painter().text(
                page_rect.center(),
                Align2::CENTER_CENTER,
                "…",
                FontId::proportional(20.0),
                Color32::from_rgb(120, 120, 120),
            );
            return;
        };

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

        // page interaction: armed hint + creation (space-pan suppresses clicks)
        let page_id = egui::Id::unique(("reader-page", self.book.id, page_no));
        ui.advance_cursor_after_rect(page_rect);
        let resp = ui.interact(page_rect, page_id, Sense::click());

        if self.armed.is_some() {
            if let Some(p) = resp.hover_pos() {
                let hint = match self.armed {
                    Some(ArmMode::PostIt) => "📌 pin here",
                    Some(ArmMode::Margin) => "🗒 place here",
                    Some(ArmMode::Flag) => "🔖 stick here",
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
        if resp.clicked() && allow_clicks {
            if let Some(p) = resp.hover_pos() {
                self.on_page_clicked(p, page_rect, page_no, postit_min);
            }
        }

        // post-its: pins + auto-hugging cards (outer side in book view)
        let auto_hang =
            self.compute_auto_hangs(page_anns, page_rect, postit_min, if strip_left { Some(true) } else { Some(false) });
        for ann in page_anns {
            if let Anchor::PostIt { .. } = ann.anchor {
                let auto = auto_hang.get(&ann.id).copied();
                self.draw_postit(ui, ann, page_rect, auto, bars);
            }
        }
        // marginalia: chalk in the outer margin of the spread
        let has_margin_notes = page_anns
            .iter()
            .any(|a| matches!(a.anchor, Anchor::Margin { .. }));
        if has_margin_notes {
            let strip_rect = if strip_left {
                Rect::from_min_size(
                    pos2(page_rect.left() - MARGIN_STRIP_W, page_rect.top()),
                    vec2(MARGIN_STRIP_W, page_rect.height()),
                )
            } else {
                Rect::from_min_size(
                    pos2(page_rect.right(), page_rect.top()),
                    vec2(MARGIN_STRIP_W, page_rect.height()),
                )
            };
            // margin solver: chalk boxes may not overlap. Remedy one: an
            // overfilling box reflows wider into the free desk — fewer
            // rows, shorter box, top still pinned at its dot. Remedy two:
            // whatever still collides slides down the strip, hanging from
            // its dot by its thread. Single-row notes are pins: never
            // widened, never moved, never overlapped — the solve stops at
            // them. Measured fresh every frame; the widened shape is
            // view-layer state, so everything reverts by itself.
            let mut solved: std::collections::HashMap<String, (f32, f32)> =
                std::collections::HashMap::new();
            if has_margin_notes {
                let content = ui.ctx().content_rect();
                let cols: Vec<MarginCol> = page_anns
                    .iter()
                    .filter_map(|ann| {
                        let Anchor::Margin { y } = &ann.anchor else { return None };
                        let natural_w = strip_rect.width() - 12.0;
                        // room to widen: outward from the page, bounded by
                        // the live window edge
                        let max_w = if strip_left {
                            strip_rect.left() + 4.0 + natural_w - (content.left() + 8.0)
                        } else {
                            content.right() - 8.0 - (strip_rect.left() + 4.0)
                        };
                        Some(MarginCol {
                            id: ann.id.clone(),
                            dot_y: strip_rect.top() + y * page_rect.height(),
                            body: if ann.body.is_empty() {
                                "…".to_string()
                            } else {
                                ann.body.clone()
                            },
                            natural_w,
                            max_w: max_w.max(natural_w),
                        })
                    })
                    .collect();
                for (id, top, width) in solve_margin_boxes(ui, content.bottom(), cols) {
                    solved.insert(id, (top, width));
                }
            }
            for ann in page_anns {
                if let Anchor::Margin { .. } = ann.anchor {
                    let place = solved.get(&ann.id).copied();
                    self.draw_margin_note(ui, ann, strip_rect, page_rect, bars, place);
                }
            }
        }

        // bookmark tabs: frosted slips with colored tips hanging on the
        // page's top edge (drawn after the notes; they ride the same pass)
        self.draw_flags(ui, page_no, page_rect, skyline);
    }

    fn ensure_page_texture(&mut self, ctx: &egui::Context, page_no: u32) -> Option<egui::TextureHandle> {
        // the ladder lives in ASKING, not rendering: the UI thread never
        // rasterizes. Whatever is cached shows right away (a preview after
        // a fast scrub, full pixels after a settle); a cold page shows
        // blank paper until the worker answers, usually within a beat.
        let now = std::time::Instant::now();
        let scrub_recent = self
            .scrub_until
            .map(|t| now < t)
            .unwrap_or(false)
            || self
                .last_page_jump
                .map(|t| now.duration_since(t) < std::time::Duration::from_millis(320))
                .unwrap_or(false);
        let full_q: u32 = (ctx.pixels_per_point() * 1.7)
            .max(1100.0)
            .min(2000.0) as u32;
        let target_q = if scrub_recent { full_q.min(520) } else { full_q };

        if let Some(t) = self.page_tex_cache.get(&page_no) {
            if t.quality >= target_q {
                return Some(t.tex.clone());
            }
        }
        // a cached (preview) page shows now while its upgrade is asked
        if let Some(t) = self.page_tex_cache.get(&page_no).cloned() {
            self.request_render(page_no, target_q);
            return Some(t.tex);
        }
        // cold page: ask the worker, show paper until it answers
        self.request_render(page_no, target_q);
        None
    }

    /// Blurred shadow sprite: radial gaussian alpha, tinted at draw time.
    fn ensure_shadow(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        if let Some(t) = &self.shadow {
            return t.clone();
        }
        let s = 160usize;
        let c = (s as f32 - 1.0) / 2.0;
        let sigma = 30.0f32;
        let mut px = Vec::with_capacity(s * s * 4);
        for y in 0..s {
            for x in 0..s {
                let dx = x as f32 - c;
                let dy = y as f32 - c;
                let d2 = dx * dx + dy * dy;
                let p = (-(d2) / (2.0 * sigma * sigma)).exp();
                let a = (255.0 * 0.36 * p) as u8;
                px.extend([0u8, 0, 0, a]);
            }
        }
        let ci = egui::ColorImage::from_rgba_unmultiplied([s, s], &px);
        let tex = ctx.load_texture("marginal-shadow", ci, egui::TextureOptions::LINEAR);
        self.shadow = Some(tex.clone());
        tex
    }

    /// Procedural paper-grain tile, per note color: card color with baked
    /// luminance wobble and sparse flecks — OPAQUE, so the paper is solid.
    fn ensure_grain(&mut self, ctx: &egui::Context, color: [u8; 3]) -> egui::TextureHandle {
        let key = format!("{},{},{}", color[0], color[1], color[2]);
        if let Some(t) = self.grain.get(&key) {
            return t.clone();
        }
        let size = 96usize;
        let mut state: u32 = 0x1234_5678
            ^ ((color[0] as u32) << 16)
            ^ ((color[1] as u32) << 8)
            ^ (color[2] as u32);
        let mut px = Vec::with_capacity(size * size * 4);
        for _ in 0..size * size {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let r = (state >> 24) as u8;
            let mut f = 1.0 + (r as f32 / 255.0 - 0.5) * 0.055;
            if r < 24 {
                f *= 0.94; // sparse darker fleck
            } else if r > 240 {
                f *= 1.02; // papery highlight
            }
            let c = tinted(color, f);
            px.extend([c.r(), c.g(), c.b(), 255]);
        }
        let ci = egui::ColorImage::from_rgba_unmultiplied([size, size], &px);
        let tex = ctx.load_texture(format!("marginal-grain-{key}"), ci, egui::TextureOptions::NEAREST_REPEAT);
        self.grain.insert(key, tex.clone());
        tex
    }

    fn on_page_clicked(
        &mut self,
        p: Pos2,
        page_rect: Rect,
        page_no: u32,
        default_size: [f32; 2],
    ) {
        let Some(mode) = self.armed else { return };
        let nx = ((p.x - page_rect.left()) / page_rect.width()).clamp(0.0, 1.0);
        let ny = ((p.y - page_rect.top()) / page_rect.height()).clamp(0.0, 1.0);
        let now = now_secs();

        // a tab doesn't take a chosen spot: it takes its page's address —
        // the page's precise percentage along the top edge, the same math
        // as the ruler. A second tab on the same page hugs the first,
        // stepping right with a tiny gap (an exceptional spot that still
        // vaguely signifies the page's station), and the far ends nudge
        // themselves inward just enough to stay fully on the sheet.
        if mode == ArmMode::Flag {
            let now = now_secs();
            let cw = tab_ribbon_w(page_rect.width());
            let k = self.flags.iter().filter(|f| f.page == page_no).count();
            let x = flag_station_x(page_no, self.page_count, page_rect.width(), cw, k);
            let flag = Flag {
                id: uuid::Uuid::new_v4().to_string(),
                page: page_no,
                x,
                color: self.last_flag_color.clone(),
                label: String::new(),
                created_at: now,
                updated_at: now,
            };
            crate::store::with_store(&self.store, |s| s.insert_flag(self.book.id, &flag)).ok();
            // spawn straight into in-place editing, exactly like a fresh post-it
            self.editing = Some(EditingNote {
                id: flag.id.clone(),
                buf: String::new(),
            });
            self.flags.push(flag);
            // the strip reads in book order — keep the vec there after
            // every in-session mutation (stable sort keeps sibling order)
            self.flags.sort_by_key(|f| f.page);
            self.armed = None;
            return;
        }

        let (anchor, default_color) = match mode {
            ArmMode::PostIt => (
                Anchor::PostIt {
                    x: nx,
                    y: ny,
                    card: None,
                    card_s: None,
                    size: Some(default_size),
                },
                "yellow",
            ),
            ArmMode::Margin => (Anchor::Margin { y: ny }, "blue"),
            // tabs already returned above
            ArmMode::Flag => unreachable!("flag placement returns early"),
        };
        let ann = Annotation {
            id: uuid::Uuid::new_v4().to_string(),
            page: page_no,
            anchor,
            body: String::new(),
            color: default_color.into(),
            created_at: now,
            updated_at: now,
        };
        crate::store::with_store(&self.store, |s| s.insert_annotation(self.book.id, &ann)).ok();
        self.annotations.push(ann.clone());
        // cards auto-hug on draw; the user's own drags freeze them per view mode
        self.armed = None;
        self.editing = Some(EditingNote {
            id: ann.id,
            buf: String::new(),
        });
    }

    // ==================================================================
    // Bookmark tabs
    // ==================================================================

    /// Bookmark tabs for one sheet. A tab never takes a chosen spot: its
    /// primary sits at the page's address — the page's precise percentage
    /// along the top edge, the same math as the ruler — with siblings
    /// hugging right in tiny steps; the far ends nudge inward just enough
    /// to stay on the sheet. There is no drag: a tab never moves; peel
    /// (delete) and re-stick instead. `skyline` lists the non-visible
    /// pages whose tabs contribute only their tip over this edge (in book
    /// view the left sheet gets previous pages, the right gets future's);
    /// hovering a tip confesses the name vertically in white chalk.
    fn draw_flags(&mut self, ui: &mut Ui, page_no: u32, page_rect: Rect, skyline: &[u32]) {
        let inert = self.armed.is_none();
        let count = self.page_count.max(1);
        let cw = tab_ribbon_w(page_rect.width());
        let body = tab_body_len(page_rect.height());
        let tip = tab_tip_len(body);
        let edge = page_rect.top();
        let ink = |sz: f32| {
            FontId::new(
                sz,
                egui::FontFamily::Name(crate::theme::NOTE_FONT_FAMILY.into()),
            )
        };

        // the sky: other pages' tabs, tip-only, at their stations
        for sky_page in skyline {
            let mut tabs: Vec<Flag> = self
                .flags
                .iter()
                .filter(|f| f.page == *sky_page)
                .cloned()
                .collect();
            tabs.sort_by_key(|f| f.created_at);
            for (k, flag) in tabs.iter().enumerate() {
                let xc = page_rect.left()
                    + flag_station_x(*sky_page, count, page_rect.width(), cw, k)
                        * page_rect.width();
                let tilt = (str_hash01(&flag.id) * 2.0 - 1.0) * 0.038;
                let rgb = crate::model::flag_rgb(&flag.color);
                paint_tab_tip(ui, rgb, pos2(xc, edge), cw, tip, tilt, false);
                if flag.label.trim().is_empty() {
                    continue; // a blank tab has nothing to say
                }
                let hb = Rect::from_min_max(
                    pos2(xc - cw * 0.5 - 3.0, edge - tip - 3.0),
                    pos2(xc + cw * 0.5 + 3.0, edge + 3.0),
                );
                let hid = egui::Id::unique(("flag-tip", self.book.id, flag.id.clone()));
                let resp = ui
                    .interact(hb, hid, egui::Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.hovered() {
                    // the name rises from the tip's far edge — north of the
                    // colored tongue, not from the page edge — white chalk
                    let font = ink((body * 0.16).clamp(8.5, 13.0));
                    let g = ui.painter().layout_no_wrap(
                        flag.label.trim().to_owned(),
                        font.clone(),
                        crate::theme::NOTE_CHALK,
                    );
                    let h = g.size().x;
                    paint_text_rotated(
                        ui,
                        pos2(xc, edge - tip - 2.0 - h * 0.5),
                        -std::f32::consts::FRAC_PI_2,
                        flag.label.trim(),
                        font,
                        f32::MAX,
                        crate::theme::NOTE_CHALK,
                    );
                }
                // a tip is a call button: touching it opens that page
                if resp.clicked() && inert {
                    self.go_to_page(*sky_page);
                }
            }
        }

        // bodies: this sheet's own tabs; the primary at the address, the
        // rest hugging right
        let mut mine: Vec<Flag> = self
            .flags
            .iter()
            .filter(|f| f.page == page_no)
            .cloned()
            .collect();
        mine.sort_by_key(|f| f.created_at);
        for (k, flag) in mine.iter().enumerate() {
            let xc = page_rect.left()
                + flag_station_x(page_no, count, page_rect.width(), cw, k) * page_rect.width();
            let pivot = pos2(xc, edge);
            // a tab being written on straightens, and its vertical ink makes
            // way for the inline editor's words
            let editing_this = self
                .editing
                .as_ref()
                .map(|e| e.id == flag.id)
                .unwrap_or(false);
            let tilt = if editing_this {
                0.0
            } else {
                (str_hash01(&flag.id) * 2.0 - 1.0) * 0.038
            };
            paint_tab(ui, flag, pivot, cw, body, tip, tilt, false, editing_this, -std::f32::consts::FRAC_PI_2);

            // ---- in-place editing, the same way as marginalia & post-its ----
            if editing_this {
                let editing = self.editing.take().unwrap();
                let edit_rect = Rect::from_min_size(
                    pos2(xc + cw * 0.5 + 6.0, edge + 4.0),
                    vec2(150.0, 26.0),
                );
                // only a ghost outline marks the editing area
                ui.painter().rect_stroke(
                    edit_rect,
                    CornerRadius::same(3),
                    Stroke::new(1.0, Color32::from_white_alpha(45)),
                    egui::StrokeKind::Middle,
                );
                let ink = Color32::from_rgb(64, 66, 72);
                let mut buf = editing.buf.clone();
                let mut layouter =
                    |ui: &Ui, text: &dyn egui::TextBuffer, width: f32| -> std::sync::Arc<egui::Galley> {
                        crate::theme::layout_note(ui, text.as_str(), width, ink)
                    };
                let edit_id = egui::Id::unique(("edit-tab", editing.id.clone()));
                let te = egui::TextEdit::singleline(&mut buf)
                    .desired_width(edit_rect.width())
                    .frame(egui::Frame::NONE)
                    .layouter(&mut layouter)
                    .id(edit_id);
                let resp = ui.put(edit_rect, te);
                ui.memory_mut(|m| m.request_focus(edit_id));
                // tab labels are Latin scratch — drop the platform IME for
                // this editor so keys bypass fcitx5 and its per-keystroke
                // protocol round-trips (note bodies keep the full IME)
                ui.output_mut(|o| o.ime = None);
                ui.painter().text(
                    pos2(edit_rect.left(), edit_rect.bottom() + 12.0),
                    Align2::LEFT_TOP,
                    "Esc or click away to save",
                    FontId::proportional(10.0),
                    Color32::from_rgb(140, 140, 145),
                );
                if editor_should_commit(ui, &resp, edit_rect) {
                    self.save_flag_label(&flag.id, &buf);
                } else {
                    self.editing = Some(EditingNote {
                        id: editing.id,
                        buf,
                    });
                }
                continue;
            }

            // hit box: axis-aligned around the tilted tab
            let bb = Rect::from_min_max(
                pos2(xc - cw * 0.5 - 4.0, edge - tip - 4.0),
                pos2(xc + cw * 0.5 + 4.0, edge + body + 4.0),
            );
            let fid = egui::Id::unique(("flag-tab", self.book.id, flag.id.clone()));
            let resp = ui.interact(bb, fid, egui::Sense::click());
            if !inert {
                continue;
            }
            // a blank tab asks to be written on; double-click retypes a
            // named one, exactly like the notes' inline editing
            if resp.double_clicked() {
                self.editing = Some(EditingNote {
                    id: flag.id.clone(),
                    buf: flag.label.clone(),
                });
            } else if resp.clicked() && flag.label.trim().is_empty() {
                self.editing = Some(EditingNote {
                    id: flag.id.clone(),
                    buf: flag.label.clone(),
                });
            }
            resp.context_menu(|menu| {
                let mut picked: Option<String> = None;
                menu.horizontal(|row| {
                    row.weak("tip:");
                    for (name, rgb) in crate::model::FLAG_COLORS {
                        if row
                            .button(
                                egui::RichText::new("■")
                                    .color(Color32::from_rgb(rgb[0], rgb[1], rgb[2])),
                            )
                            .clicked()
                        {
                            picked = Some(name.to_string());
                        }
                    }
                });
                if let Some(name) = picked {
                    self.set_flag_color(&flag.id, &name);
                    menu.close();
                }
                if menu.button("Rename tab").clicked() {
                    self.editing = Some(EditingNote {
                        id: flag.id.clone(),
                        buf: flag.label.clone(),
                    });
                    menu.close();
                }
                if menu.button("Delete tab").clicked() {
                    self.delete_flag(&flag.id);
                    menu.close();
                }
            });
        }
    }

    fn set_flag_color(&mut self, id: &str, color: &str) {
        self.last_flag_color = color.to_owned();
        let Some(f) = self.flags.iter_mut().find(|f| f.id == id) else { return };
        f.color = color.to_owned();
        f.updated_at = now_secs();
        let (page, x, label) = (f.page, f.x, f.label.clone());
        crate::store::with_store(&self.store, |s| {
            s.update_flag(id, page, x, color, &label, now_secs())
        })
        .ok();
    }

    fn save_flag_label(&mut self, id: &str, label: &str) {
        let label = label.trim().to_string();
        let Some(f) = self.flags.iter_mut().find(|f| f.id == id) else { return };
        f.label = label.clone();
        f.updated_at = now_secs();
        let (page, x, color) = (f.page, f.x, f.color.clone());
        crate::store::with_store(&self.store, |s| {
            s.update_flag(id, page, x, &color, &label, now_secs())
        })
        .ok();
    }

    fn delete_flag(&mut self, id: &str) {
        crate::store::with_store(&self.store, |s| s.delete_flag(id)).ok();
        self.flags.retain(|f| f.id != id);
        if self.editing.as_ref().map(|e| e.id.as_str()) == Some(id) {
            self.editing = None;
        }
    }

    // ==================================================================
    // Render worker plumbing
    // ==================================================================

    /// Drain finished render results: swap textures in, adopt the book's
    /// metadata (page count, outline, page sizes) the first time it lands,
    /// and surface one error if the book itself won't open.
    fn poll_renders(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.render_rx else { return };
        let mut repaint = false;
        loop {
            let Ok(result) = rx.try_recv() else { break };
            repaint = true;
            match result {
                RenderResult::Meta(meta) => {
                    self.page_count = meta.page_count.max(1);
                    self.chapter_marks = meta.outline.clone();
                    self.doc_info = Some(meta);
                    crate::store::with_store(&self.store, |s| {
                        s.set_page_count(self.book.id, self.page_count)
                    })
                    .ok();
                }
                RenderResult::Image { page, quality, img } => {
                    let color = egui::ColorImage::from_rgba_unmultiplied(
                        [img.width as usize, img.height as usize],
                        &img.rgba,
                    );
                    let tex = ctx.load_texture(
                        format!("marginal-page-{}-{}", self.book.id, page),
                        color,
                        egui::TextureOptions::LINEAR,
                    );
                    self.page_tex_cache
                        .insert(page, PageTex { tex, quality });
                    self.render_pending.remove(&page);
                    while self.page_tex_cache.len() > 6 {
                        let Some(victim) = self
                            .page_tex_cache
                            .keys()
                            .find(|k| **k != page)
                            .copied()
                        else { break };
                        self.page_tex_cache.remove(&victim);
                    }
                }
                RenderResult::Failed { page, open_failed } => {
                    self.render_pending.remove(&page);
                    if open_failed {
                        if self.error.is_none() {
                            self.error =
                                Some("could not open the book file".into());
                        }
                    } else {
                        self.render_failed.insert(page);
                        if self.error.is_none() {
                            self.error = Some(format!("could not render page {page}"));
                        }
                    }
                }
            }
        }
        if repaint {
            ctx.request_repaint();
        }
    }

    /// Ask the worker for `page` at `quality`, deduped: an in-flight job at
    /// equal or higher quality is never re-sent, and failed pages are
    /// never asked again.
    fn request_render(&mut self, page: u32, quality: u32) {
        if self.render_failed.contains(&page) {
            return;
        }
        let pending_q = self.render_pending.get(&page).copied().unwrap_or(0);
        let Some(wanted) = request_wants(pending_q, quality) else { return };
        if let Some(tx) = &self.render_tx {
            let _ = tx.send(RenderJob {
                page,
                quality: wanted,
            });
            self.render_pending.insert(page, wanted);
        }
    }

    // ==================================================================
    // Post-its
    // ==================================================================

    /// A post-it = a red pin on the page + a paper card hanging on the desk,
    /// joined by a faint dashed thread. Analog paper: hand-tilted silhouette,
    /// dog-ear curl, layered shadow, grain, elastic ink-height, resize grip.
    fn draw_postit(
        &mut self,
        ui: &mut Ui,
        ann: &Annotation,
        page_rect: Rect,
        auto: Option<[f32; 2]>,
        bars: &mut Vec<(Annotation, Rect)>,
    ) {
        let Anchor::PostIt {
            x,
            y,
            card,
            card_s,
            size: size_stored,
        } = ann.anchor
        else {
            return;
        };
        // per-view-mode slot: user-set position wins, else auto-hug the pin
        let user_slot = if self.facing { card_s } else { card };
        let [cx, cy] = user_slot.or(auto).unwrap_or([0.4, 0.0]);

        // ---- size: user choice, floor = pad minimum, ceiling = the ink ----
        let [min_w, min_h] = self.card_min_size(ui);
        let pad = crate::theme::NOTE_PAD;
        let [mut w, mut h] = size_stored.unwrap_or([min_w, min_h]);
        if w < 1.0 {
            w = min_w;
        }
        if h < 1.0 {
            h = min_h;
        }
        w = w.max(min_w);
        h = h.max(min_h);
        let text_w = w - 2.0 * pad;

        let pin_pos = pos2(
            page_rect.min.x + x * page_rect.width(),
            page_rect.min.y + y * page_rect.height(),
        );

        // ---- effective height: ink + padding (ann body, or live buffer) ----
        let ink_height = |s: &str| -> f32 {
            if s.trim().is_empty() {
                0.0
            } else {
                crate::theme::layout_note(ui, s, text_w, Color32::TRANSPARENT).size().y
                    + 2.0 * pad
            }
        };
        let mut h_eff = h.max(min_h).max(ink_height(&ann.body).ceil());

        let card_top = pos2(
            page_rect.min.x + cx * page_rect.width(),
            page_rect.min.y + cy * page_rect.height(),
        );
        let mut card = Rect::from_min_size(card_top, vec2(w, h_eff));

        // ---- if editing, the live buffer drives growth ----
        let editing_this = self.editing.as_ref().map(|e| e.id == ann.id).unwrap_or(false);
        let editing = if editing_this {
            let e = self.editing.take().unwrap();
            let g = crate::theme::layout_note(ui, e.buf.as_str(), text_w, Color32::TRANSPARENT);
            h_eff = h.max(min_h).max((g.size().y + 2.0 * pad).ceil());
            card = Rect::from_min_size(card_top, vec2(w, h_eff));
            Some(e)
        } else {
            None
        };

        // cards run a touch paler since the v2 look pass
        let card_fill = paler(note_color_parts(&ann.color), 0.22);

        // ----- red pin, draggable anywhere on the page -----
        let pin_id = egui::Id::unique(("postit-pin", ann.id.clone()));
        let pin_rect = Rect::from_center_size(pin_pos, vec2(14.0, 14.0));
        ui.advance_cursor_after_rect(pin_rect);
        let pin_resp = ui.interact(pin_rect, pin_id, Sense::click_and_drag());
        self.note_context_menu(ann, &pin_resp);

        // ----- pencil thread — turns red while this note is being edited -----
        if !card.contains(pin_pos) {
            let attach = edge_point_toward(card, pin_pos);
            let seed = egui::util::hash((ann.id.clone(), "thread"));
            let thread_color = if editing_this {
                crate::theme::PIN_RED
            } else {
                crate::theme::postit_thread()
            };
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

        // interaction: pin + card + grip all register with stable ids
        let card_id = egui::Id::unique(("postit", ann.id.clone()));
        ui.advance_cursor_after_rect(card);
        let resp = ui.interact(card, card_id, Sense::click_and_drag());
        self.note_context_menu(ann, &resp);

        let grip_id = egui::Id::unique(("postit-grip", ann.id.clone()));
        let grip_rect = Rect::from_center_size(card.right_bottom() - vec2(8.0, 8.0), vec2(18.0, 18.0));
        ui.advance_cursor_after_rect(grip_rect);
        let grip_resp = ui.interact(grip_rect, grip_id, Sense::click_and_drag());

        let grain = self.ensure_grain(ui.ctx(), card_fill);
        let shadow = self.ensure_shadow(ui.ctx());
        let painter = ui.painter().clone();
        let lifted = resp.dragged();
        paint_paper(
            &painter,
            card,
            egui::util::hash(ann.id.clone()),
            card_fill,
            &grain,
            &shadow,
            lifted,
        );
        if editing.is_some() {
            // mark the writing area with the pin color ring while typing
            ui.painter().rect_stroke(
                card,
                CornerRadius {
                    nw: 3,
                    ne: 3,
                    se: 9,
                    sw: 3,
                },
                Stroke::new(2.0, crate::theme::PIN_RED),
                egui::StrokeKind::Middle,
            );
        }

        // hover bar (stays up while the pointer is over the bar itself)
        if editing.is_none() {
            bars.push((ann.clone(), card));
        }

        // text
        if let Some(mut editing) = editing {
            let edit_id = egui::Id::unique(("edit-note", editing.id.clone()));
            let mut buf = editing.buf.clone();
            let dark_ink = Color32::from_rgb(45, 45, 45);
            let mut layouter =
                |ui: &Ui, text: &dyn egui::TextBuffer, width: f32| -> std::sync::Arc<egui::Galley> {
                    crate::theme::layout_note(ui, text.as_str(), width, dark_ink)
                };
            let te = egui::TextEdit::multiline(&mut buf)
                .desired_width(text_w)
                .frame(egui::Frame::NONE)
                .layouter(&mut layouter)
                .id(edit_id);
            let resp = ui.put(card.shrink2(vec2(pad - 5.0, pad - 6.0)), te);
            editing.buf = buf;
            ui.memory_mut(|m| m.request_focus(edit_id));
            // key events bypass the input-method daemon here — this is the
            // per-keystroke round-trip that batches and freezes typing
            ui.output_mut(|o| o.ime = None);
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
        } else if ann.body.trim().is_empty() && !resp.hovered() {
            ui.painter().text(
                card.center(),
                Align2::CENTER_CENTER,
                "double-click to write",
                FontId::proportional(11.0),
                Color32::from_rgb(120, 120, 120),
            );
        } else if !ann.body.trim().is_empty() {
            let g = crate::theme::layout_note(
                ui,
                ann.body.as_str(),
                text_w,
                Color32::from_rgb(40, 40, 40),
            );
            let wob = vec2(
                (egui::util::hash((ann.id.clone(), "ix")) % 7) as f32 - 3.0,
                (egui::util::hash((ann.id.clone(), "iy")) % 7) as f32 - 3.0,
            );
            ui.painter().galley(card.min + vec2(pad, pad) + wob, g, Color32::from_rgb(40, 40, 40));
        }

        // drag the card anywhere on the desk — freezes the auto-hang spot
        // FOR THIS VIEW MODE only (single and spread keep separate memories)
        if resp.dragged() {
            let delta = resp.drag_delta();
            let mut moved = self.fetch_annotation(&ann.id).unwrap_or_else(|| ann.clone());
            if let Anchor::PostIt { card, card_s, .. } = &mut moved.anchor {
                let new_pos = [
                    cx + delta.x / page_rect.width(),
                    cy + delta.y / page_rect.height(),
                ];
                if self.facing {
                    *card_s = Some(new_pos);
                } else {
                    *card = Some(new_pos);
                }
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
        // drag the grip to resize (grows past the minimum when ink needs it)
        if grip_resp.dragged() {
            let delta = grip_resp.drag_delta();
            let [uw, uh] = size_stored.unwrap_or([w, h_eff]);
            let mut moved = self.fetch_annotation(&ann.id).unwrap_or_else(|| ann.clone());
            if let Anchor::PostIt { size, .. } = &mut moved.anchor {
                let nw = (uw + delta.x).clamp(min_w, 520.0);
                let nh = (uh + delta.y).clamp(min_h, 620.0);
                *size = Some([nw, nh]);
            }
            self.save_annotation(moved);
        }
        if grip_resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
        }
        if resp.double_clicked() && grip_resp.hovered() == false && !resp.dragged() {
            self.editing = Some(EditingNote {
                id: ann.id.clone(),
                buf: ann.body.clone(),
            });
        }
    }

    // ==================================================================
    // Marginalia
    // ==================================================================

    fn draw_margin_note(
        &mut self,
        ui: &mut Ui,
        ann: &Annotation,
        strip: Rect,
        page_rect: Rect,
        bars: &mut Vec<(Annotation, Rect)>,
        solved: Option<(f32, f32)>, // (box top, wrap width) from the margin solver
    ) {
        // marginalia = white chalk handwriting floating on the desk.
        // No card. The note's color only shows in the connector + anchor dot.
        let ink = crate::theme::NOTE_CHALK;
        let thread = note_color_ui(&ann.color);
        let Anchor::Margin { y } = ann.anchor else { return };

        // the dot never moves; the solver may widen the wrap and slide the
        // box down the strip when boxes crowd — the thread stays attached
        let text_w = solved.map(|s| s.1).unwrap_or(strip.width() - 12.0);
        let box_top = solved
            .map(|s| s.0)
            .unwrap_or_else(|| strip.top() + y * page_rect.height());
        let y_screen = strip.top() + y * page_rect.height();

        // measure the text so the interaction area fits the handwriting
        let probe_text = if ann.body.is_empty() {
            "…".to_string()
        } else {
            ann.body.clone()
        };
        let probe = crate::theme::layout_note(ui, probe_text.as_str(), text_w, ink);
        let h = (probe.size().y + 8.0).max(42.0);
        // widening grows outward from the page: on a left strip the box's
        // right edge stays put and the box grows leftward
        let rect = if strip.right() <= page_rect.left() + 1.0 {
            let right = strip.left() + 4.0 + (strip.width() - 12.0);
            Rect::from_min_max(pos2(right - text_w, box_top), pos2(right, box_top + h))
        } else {
            Rect::from_min_size(pos2(strip.left() + 4.0, box_top), vec2(text_w, h))
        };

        // the anchor dot on the page edge is itself draggable: grabbing it
        // re-targets which passage this marginalia points at
        // anchor mirrors with the sheet: left sheets anchor on their right
        // (spine-side) edge, right sheets on their left edge — the thread
        // always crosses the full page as the underline
        let anchor_on_right = strip.right() <= page_rect.left() + 1.0;
        let anchor_pt = pos2(
            if anchor_on_right {
                page_rect.right()
            } else {
                page_rect.left()
            },
            y_screen,
        );
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
            // key events bypass the input-method daemon here — this is the
            // per-keystroke round-trip that batches and freezes typing
            ui.output_mut(|o| o.ime = None);
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
        self.note_context_menu(ann, &resp);
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
            bars.push((ann.clone(), r));

        // vertical drag re-anchors the note height (card OR its anchor dot)
        if resp.dragged() {
            let mut moved = self.fetch_annotation(&ann.id).unwrap_or_else(|| ann.clone());
            if let Anchor::Margin { y } = &mut moved.anchor {
                *y = (*y + resp.drag_delta().y / page_rect.height()).clamp(0.0, 1.0);
            }
            self.save_annotation(moved);
        }
        if dot_resp.dragged() {
            let mut moved = self.fetch_annotation(&ann.id).unwrap_or_else(|| ann.clone());
            if let Anchor::Margin { y } = &mut moved.anchor {
                *y = (*y + dot_resp.drag_delta().y / page_rect.height()).clamp(0.0, 1.0);
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

    /// Duplicate: same ink, color and page on a fresh id and pin spot.
    /// The card slot is not carried over — it re-hugs the new pin like a
    /// fresh spawn would, so the copy reads as a new note, not a shadow.
    fn duplicate_note(&mut self, ann: &Annotation) {
        let now = now_secs();
        let mut dup = ann.clone();
        dup.id = uuid::Uuid::new_v4().to_string();
        dup.created_at = now;
        dup.updated_at = now;
        match &mut dup.anchor {
            Anchor::PostIt { x, y, card, card_s, .. } => {
                *x = nudge_frac(*x, 0.035);
                *y = nudge_frac(*y, 0.035);
                *card = None; // re-hug: hang beside its own new pin
                *card_s = None;
            }
            Anchor::Margin { y } => {
                *y = nudge_frac(*y, 0.055);
            }
        }
        crate::store::with_store(&self.store, |s| s.insert_annotation(self.book.id, &dup)).ok();
        self.annotations.push(dup);
    }

    /// Delete a note and every link touching it (outgoing and incoming),
    /// so no orphan connections survive the note.
    fn delete_note(&mut self, id: &str) {
        crate::store::with_store(&self.store, |s| {
            s.delete_annotation(id).ok();
            for (src, link_id, target) in s.links_of_book(self.book.id).unwrap_or_default() {
                let points_here = matches!(&target, LinkTarget::Annotation(dst) if dst == id);
                if src == id || points_here {
                    s.delete_link(&link_id).ok();
                }
            }
            Ok(())
        })
        .ok();
        self.annotations.retain(|a| a.id != id);
        if self.editing.as_ref().is_some_and(|e| e.id == id) {
            self.editing = None;
        }
    }

    /// Right-click on any note — desk or sidebar — duplicates or deletes it.
    fn note_context_menu(&mut self, ann: &Annotation, resp: &egui::Response) {
        resp.context_menu(|ui| {
            if ui
                .button("Duplicate note")
                .on_hover_text("same ink, fresh pin spot")
                .clicked()
            {
                self.duplicate_note(ann);
                ui.close();
                ui.ctx().request_repaint();
            }
            if ui
                .button("Delete note")
                .on_hover_text("remove the note and its links")
                .clicked()
            {
                self.delete_note(&ann.id);
                ui.close();
                ui.ctx().request_repaint();
            }
        });
    }

    fn note_hover_bar(&mut self, ui: &mut Ui, ann: &Annotation, rect: Rect) {
        let bar_y = (rect.min.y - BAR_H - 4.0).max(4.0);
        let bar = Rect::from_min_size(pos2(rect.min.x, bar_y), vec2(rect.width(), BAR_H));

        // the bar shows while the pointer is anywhere on card or bar
        if !(ui.rect_contains_pointer(bar) || ui.rect_contains_pointer(rect)) {
            return;
        }
        // The bar must stay interactive: a child ui built with
        // UiBuilder::invisible() is disabled in egui 0.37+, so every widget
        // inside it was dead (hover never registered). Place the buttons
        // directly at absolute rects instead — same idiom as the page ruler.
        let cell = |ix: f32| Rect::from_min_size(pos2(bar.min.x + ix, bar.min.y), vec2(22.0, BAR_H));
        if ui
            .put(cell(0.0), egui::Button::new("✎").small())
            .on_hover_text("edit")
            .clicked()
        {
            self.editing = Some(EditingNote {
                id: ann.id.clone(),
                buf: ann.body.clone(),
            });
        }
        if ui
            .put(cell(26.0), egui::Button::new("🔗").small())
            .on_hover_text("add link")
            .clicked()
        {
            self.link_dialog = Some(links::LinkDialog::new(
                &ann.id,
                self.book.fingerprint.clone(),
                self.page,
            ));
        }
        if ui
            .put(cell(52.0), egui::Button::new("🗑").small())
            .on_hover_text("delete")
            .clicked()
        {
            self.delete_note(&ann.id);
        }
        let outgoing = crate::store::with_store(&self.store, |s| s.links_for(&ann.id))
            .unwrap_or_default();
        // one → per outgoing link (up to three): each jumps to its own
        // destination — another note, a page, or a URL
        for (ix, (_, target)) in outgoing.iter().enumerate().take(3) {
            let tip = format!("jump to {}", links::describe(target));
            if ui
                .put(cell(78.0 + 26.0 * ix as f32), egui::Button::new("→").small())
                .on_hover_text(tip)
                .clicked()
            {
                self.navigate_target(target.clone());
            }
        }
    }

    /// Draw note↔note link curves for annotations on this page.
    fn draw_link_overlays(&mut self, ui: &mut Ui, sheets: &[(u32, Rect, Vec<Annotation>)]) {
        let links = crate::store::with_store(&self.store, |s| s.links_of_book(self.book.id))
            .unwrap_or_default();
        let active_src = self
            .link_dialog
            .as_ref()
            .map(|d| d.from.clone())
            .unwrap_or_default();

        // resolve every annotation id to its on-desk focus point
        let mut placed: Vec<(String, Pos2)> = Vec::new();
        for (_page_no, rect, anns) in sheets {
            for a in anns {
                placed.push((a.id.clone(), card_focus(a, *rect)));
            }
        }

        for (src, _link_id, target) in &links {
            let src_pos = placed.iter().find(|(id, _)| id == src).map(|(_, p)| *p);
            match target {
                LinkTarget::Annotation(dst) => {
                    let dst_pos = placed.iter().find(|(id, _)| id == dst).map(|(_, p)| *p);
                    if let (Some(a), Some(b)) = (src_pos, dst_pos) {
                        links::hand_dashed_line(
                            &ui.painter().clone(),
                            a,
                            b,
                            egui::util::hash((src.clone(), dst.clone())),
                            Color32::from_rgb(125, 95, 170),
                        );
                    }
                }
                _ if src == &active_src => {
                    if let Some(a) = src_pos {
                        ui.painter().circle_filled(
                            a,
                            7.0,
                            Color32::from_rgba_unmultiplied(125, 95, 170, 140),
                        );
                    }
                }
                _ => {}
            }
        }
    }

    // ==================================================================
    // Notes index strip
    // ==================================================================

    /// Sort key: reading order — page, then anchor y (top to bottom),
    /// then anchor x (left to right). Marginalia has no x; its thread
    /// anchors the page edge, so it sorts by y alone and reads before the
    /// post-its of the same height band (the margin hangs beside the text).
    fn anchor_sort_key(ann: &Annotation) -> (u32, u32, u32) {
        let page = ann.page;
        match ann.anchor {
            Anchor::PostIt { x, y, .. } => (page, (y * 1000.0).round() as u32, (x * 1000.0).round() as u32),
            Anchor::Margin { y } => (
                page,
                (y * 1000.0).round() as u32,
                0, // margin strip sits beside the page's left edge
            ),
        }
    }

    /// The notes strip as a strip of celluloid film laid ON the desk: the
    /// workspace keeps running beneath the translucent base, the drop
    /// shadow is the post-its' own (same offset, same light), so the film
    /// reads as part of the same pile of desk objects.
    /// The notes strip: pinned sidebar UI, 20%-transparent Midnight, laid
    /// over the full-width desk. Pages, notes and the dot grid show
    /// through the translucency and pan beneath like anywhere else; the
    /// strip's own contents respond to nothing — no wheel, no drag — so
    /// the sidebar itself never moves — except that wheel-over-strip now
    /// scrolls the strip (the desk politely skips those events).
    fn notes_film_rect(&self, avail: Rect) -> Rect {
        Rect::from_min_max(
            pos2(avail.right() - 294.0, self.toolbar_bottom + 10.0),
            pos2(avail.right() - 8.0, avail.bottom() - 8.0),
        )
    }

    fn notes_film(&mut self, ui: &mut Ui) {
        // fresh screen truth: the panel path can hold a stale window size
        // across resizes; the context always knows the live client, so the
        // film anchors to it and paints in its own layer, with its own —
        // fresh, every frame — clip. Nothing stale can drag it off-screen.
        let avail = ui.ctx().content_rect();
        let film = self.notes_film_rect(avail);
        egui::Area::new(egui::Id::unique("notes-film-layer"))
            .order(egui::Order::Middle)
            .fixed_pos(film.min)
            .show(ui.ctx(), |ui| {
        // the film lies on the desk: clicks and drags on the strip must
        // not reach paper, ruler or cards underneath
        let _ = ui.allocate_rect(film, egui::Sense::click_and_drag());

        // soft drop shadow — the post-its' blurred sprite, same offset
        let shadow = self.ensure_shadow(ui.ctx());
        let q = film.expand2(vec2(22.0, 22.0));
        let mut sm = egui::Mesh::default();
        sm.texture_id = shadow.id();
        for (pt, uv) in [
            (q.left_top(), pos2(0.0, 0.0)),
            (q.right_top(), pos2(1.0, 0.0)),
            (q.right_bottom(), pos2(1.0, 1.0)),
            (q.left_bottom(), pos2(0.0, 1.0)),
        ] {
            sm.vertices.push(egui::epaint::Vertex {
                pos: pt + vec2(2.0, 3.5),
                color: Color32::from_black_alpha(150),
                uv,
            });
        }
        sm.indices.extend([0u32, 1, 2, 0, 2, 3]);
        ui.painter().add(egui::Shape::mesh(sm));

        // translucent Midnight base + a wisp of emulsion grain
        ui.painter().rect_filled(film, 0.0, crate::theme::film_midnight());
        #[cfg(debug_assertions)]
        if std::env::var("MARGINAL_FILM_TRACE").as_deref() == Ok("1") {
            eprintln!(
                "[film-trace] max={:?} clip={:?} film={:?} pan=({:.0},{:.0}) z={:.2}",
                ui.max_rect(),
                ui.painter().clip_rect(),
                film,
                self.pan.x,
                self.pan.y,
                self.zoom
            );
        }
        let painter = ui.painter().clone();
        for (tint, alpha) in [([245u8, 235, 220], 10), ([10, 12, 34], 16)] {
            let grain = self.ensure_grain(ui.ctx(), tint);
            let ns = 46.0;
            let mut mesh = egui::Mesh::default();
            mesh.texture_id = grain.id();
            for (pt, uv) in [
                (film.left_top(), pos2(0.0, 0.0)),
                (film.right_top(), pos2(film.width() / ns, 0.0)),
                (
                    film.right_bottom(),
                    pos2(film.width() / ns, film.height() / ns),
                ),
                (film.left_bottom(), pos2(0.0, film.height() / ns)),
            ] {
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: pt,
                    color: Color32::from_rgba_unmultiplied(tint[0], tint[1], tint[2], alpha),
                    uv,
                });
            }
            mesh.indices.extend([0u32, 1, 2, 0, 2, 3]);
            painter.add(egui::Shape::mesh(mesh));
        }

                // content, in a sub-ui exactly over the film
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .id_salt("notes-film-body")
                        .max_rect(film.shrink2(vec2(6.0, 6.0))),
                    |ui| {
                        self.notes_body(ui);
                    },
                );
            },
        );
    }

    /// The strip's contents: every annotation rendered as it appears on
    /// the desk — post-its as real paper tiles (fixed width), marginalia
    /// as white chalk handwriting (fixed width) — grouped by page,
    /// ordered by reading position. Moving pins on the desk re-orders
    /// the strip.
    fn notes_body(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.heading("Notes in book order");
        ui.add_space(4.0);

        let mut anns = self.annotations.clone();
        anns.sort_by_key(Self::anchor_sort_key);

        // every link of the book, grouped under its source, so the strip can
        // attach connections directly to the note they belong to
        let links_by_src = links::group_by_src(
            crate::store::with_store(&self.store, |s| s.links_of_book(self.book.id))
                .unwrap_or_default(),
        );

        let tile_w = ui.available_width() - 24.0;

        // the strip scrolls on the wheel while the pointer rides it
        // (hovering the strip is the strip's business, not the desk's);
        // content drags stay dead — only the scrollbar and the wheel move it
        ScrollArea::vertical()
            .scroll_source(egui::containers::scroll_area::ScrollSource {
                scroll_bar: true,
                drag: egui::containers::scroll_area::DragScroll::Never,
                mouse_wheel: true,
            })
            .show(ui, |ui| {
            // bookmarks first: the pages you tagged, tabs lying flat, tips
            // toward the spine. Click jumps to the page.
            if !self.flags.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("🔖 Bookmarks")
                        .strong()
                        .color(crate::theme::CHAPTER_MARK),
                );
                ui.add_space(2.0);
                for flag in self.flags.clone() {
                    self.sidebar_flag(ui, &flag);
                }
                ui.add_space(10.0);
            }

            let mut last_page: Option<u32> = None;
            for ann in &anns {
                // page group headers — a thin amber chapter-rule
                if last_page != Some(ann.page) {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("p. {}", ann.page))
                                .strong()
                                .color(crate::theme::CHAPTER_MARK),
                        );
                        let label = self
                            .chapter_marks
                            .iter()
                            .filter(|(p, _)| *p == ann.page)
                            .map(|(_, t)| t.clone())
                            .next();
                        if let Some(t) = label {
                            ui.label(egui::RichText::new(t).weak().small());
                        }
                    });
                    ui.separator();
                    last_page = Some(ann.page);
                }

                match ann.anchor {
                    Anchor::PostIt { .. } => {
                        self.sidebar_postit(ui, ann, tile_w);
                    }
                    Anchor::Margin { .. } => {
                        self.sidebar_margin(ui, ann, tile_w);
                    }
                }

                // connections ride with their note — click to jump, ✕ to remove
                if let Some(rows) = links_by_src.get(&ann.id) {
                    for (link_id, target) in rows {
                        self.sidebar_link_row(ui, link_id, target);
                    }
                }
            }
            if anns.is_empty() {
                ui.label(
                    egui::RichText::new("No notes yet — arm 📌 or 🗒 and click the page.").weak(),
                );
            }
            ui.add_space(24.0);
        });

        // footer: sidecar buttons (moved over from the links panel)
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
                        .add_filter("Marginal sidecar", &["json"])
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

    /// One sidebar bookmark tab: THE tab - the same paper as the page's,
    /// fallen over flat (the post-its' rule: sidebar tiles are the desk
    /// object, pinned to the strip). Colored tip at the west end toward the
    /// spine, same frost, crease, tongue shadow and per-tab wobble; the
    /// title lies down with it and reads left-to-right upright, 14pt Rock
    /// Salt like every other ink in the strip. Pencil & bin ride the hover;
    /// the pencil opens the same inline editor on the tab's page.
    fn sidebar_flag(&mut self, ui: &mut Ui, flag: &Flag) {
        let w = (ui.available_width() - 8.0).max(120.0);
        let editing = self.editing.as_ref().map(|e| e.id == flag.id).unwrap_or(false);

        // the tab lying down: a quarter-turn of the page object, tip west,
        // at the book's own proportions with 50% more runway: the ink is
        // left-aligned, starting at the tongue and running downstream.
        let ribbon = 24.0;
        let body_len = ribbon * 3.5 * 1.5;
        let tip_len = body_len * 0.34;
        let h = ribbon + 10.0; // the row hugs the slip
        let (rect, resp) = ui.allocate_exact_size(vec2(w, h), egui::Sense::click());
        let pivot = pos2(rect.left() + tip_len, rect.center().y);
        // the same per-tab wobble as on the page, folded into the fall;
        // ink stays upright (as on the page, ink never wobbles)
        let wobble = (str_hash01(&flag.id) * 2.0 - 1.0) * 0.038;
        let lay = -std::f32::consts::FRAC_PI_2 + wobble;
        paint_tab(ui, flag, pivot, ribbon, body_len, tip_len, lay, false, true, 0.0);

        // the strip's ink starts AT the tip and runs downstream — left-
        // aligned like a pen touching the sticker's base edge; shrink to
        // fit within the paper
        let empty = flag.label.trim().is_empty();
        let title = if empty {
            "untitled".to_string()
        } else {
            flag.label.trim().to_string()
        };
        let mut g = ui.painter().layout_no_wrap(
            title.clone(),
            crate::theme::note_font(),
            Color32::TRANSPARENT,
        );
        let mut len = g.size().x;
        let max_len = body_len - 16.0;
        if len > max_len && len > 0.0 {
            let font = egui::FontId::new(
                (crate::theme::note_font().size * max_len / len).max(6.0),
                crate::theme::note_font().family.clone(),
            );
            g = ui.painter().layout_no_wrap(title.clone(), font, Color32::TRANSPARENT);
            len = g.size().x;
        }
        let r_lay = egui::emath::Rot2::from_angle(lay);
        let anchor = pivot + r_lay * vec2(0.0, 8.0 + len * 0.5);
        paint_text_rotated(
            ui,
            anchor,
            0.0,
            &title,
            crate::theme::note_font(),
            max_len,
            if empty {
                Color32::from_rgba_unmultiplied(62, 64, 70, 170)
            } else {
                Color32::from_rgb(56, 58, 64)
            },
        );

        // the page chip: small print at the slip's far end, over the frost
        ui.painter().text(
            pos2(rect.left() + tip_len + body_len - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            format!("p. {}", flag.page),
            FontId::proportional(9.0),
            Color32::from_rgba_unmultiplied(58, 60, 66, 180),
        );
        if editing {
            let ring = Rect::from_min_max(
                pos2(rect.left() - 2.0, rect.center().y - ribbon * 0.5 - 2.0),
                pos2(rect.left() + tip_len + body_len + 4.0, rect.center().y + ribbon * 0.5 + 2.0),
            );
            ui.painter().rect_stroke(
                ring,
                CornerRadius::same(4),
                Stroke::new(1.2, crate::theme::CHAPTER_MARK),
                egui::StrokeKind::Outside,
            );
        }

        // pencil & bin ride the pointer hover, over the tab's far end
        let mut used_button = false;
        if ui.rect_contains_pointer(rect) {
            if ui
                .put(
                    Rect::from_min_size(pos2(rect.right() - 30.0, rect.min.y), vec2(24.0, h)),
                    egui::Button::new("🗑").small(),
                )
                .clicked()
            {
                self.delete_flag(&flag.id);
                used_button = true;
            }
            if ui
                .put(
                    Rect::from_min_size(pos2(rect.right() - 58.0, rect.min.y), vec2(24.0, h)),
                    egui::Button::new("✎").small(),
                )
                .clicked()
            {
                // renaming from the strip takes you to the tab, where the
                // same inline editor opens as on the page
                self.go_to_page(flag.page);
                self.editing = Some(EditingNote {
                    id: flag.id.clone(),
                    buf: flag.label.clone(),
                });
                used_button = true;
            }
        }
        if resp.clicked() && !used_button {
            self.go_to_page(flag.page);
        }
    }

    /// One sidebar post-it: the same paper tile code path as the desk,
    /// width pinned to the strip (height flows from the ink).
    fn sidebar_postit(&mut self, ui: &mut Ui, ann: &Annotation, tile_w: f32) {
        let pad = crate::theme::NOTE_PAD;
        let w = tile_w.max(60.0);
        let text_w = w - 2.0 * pad;

        let card_fill = paler(note_color_parts(&ann.color), 0.22);
        let grain = self.ensure_grain(ui.ctx(), card_fill);
        let shadow = self.ensure_shadow(ui.ctx());
        let painter = ui.painter().clone();

        // height from the ink, exactly like the desk resolves it
        let ink_h = if ann.body.trim().is_empty() {
            2.0 * pad + crate::theme::note_font().size * 2.0
        } else {
            crate::theme::layout_note(ui, ann.body.as_str(), text_w, Color32::TRANSPARENT)
                .size()
                .y
                + 2.0 * pad
        };
        let h = ink_h.max(46.0);

        let (rect, _resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
        let card = rect.intersect(rect.expand(0.0)); // full row = card

        paint_paper(&painter, card, egui::util::hash(ann.id.clone()), card_fill, &grain, &shadow, false);

        // text (wobble keeps the hand-placed feel, as on the desk)
        if ann.body.trim().is_empty() {
            painter.text(
                card.center(),
                Align2::CENTER_CENTER,
                "empty note",
                FontId::proportional(11.0),
                Color32::from_rgb(120, 120, 120),
            );
        } else {
            let g = crate::theme::layout_note(ui, ann.body.as_str(), text_w, Color32::from_rgb(40, 40, 40));
            let wob = vec2(
                (egui::util::hash((ann.id.clone(), "ix")) % 7) as f32 - 3.0,
                (egui::util::hash((ann.id.clone(), "iy")) % 7) as f32 - 3.0,
            );
            painter.galley(
                card.min + vec2(pad, pad) + wob,
                g,
                Color32::from_rgb(40, 40, 40),
            );
        }

        // red pin dot, top-left, so the row announces itself as a post-it
        painter.circle_filled(pos2(card.left() + pad * 0.55, card.top() + pad * 0.55), 4.0, crate::theme::PIN_RED);

        // click → jump to the note's page
        let resp = ui.interact(
            rect,
            egui::Id::unique(("sidebar-postit", ann.id.clone())),
            Sense::click(),
        );
        self.note_context_menu(ann, &resp);
        if resp.clicked() {
            self.go_to_page(ann.page);
        }
    }

    /// One connection row under a sidebar note: where the link goes —
    /// click jumps (even across books / to URLs), ✕ removes just the link.
    fn sidebar_link_row(&mut self, ui: &mut Ui, link_id: &str, target: &LinkTarget) {
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            let label = egui::RichText::new(links::describe(target)).weak();
            if ui
                .button(label)
                .on_hover_text("go to the linked entity")
                .clicked()
            {
                self.navigate_target(target.clone());
            }
            if ui
                .small_button("✕")
                .on_hover_text("remove this link")
                .clicked()
            {
                crate::store::with_store(&self.store, |s| s.delete_link(link_id)).ok();
                ui.ctx().request_repaint();
            }
        });
    }

    /// One sidebar marginalia: white chalk handwriting on the panel,
    /// fixed to strip width — same face and ink as on the desk.
    fn sidebar_margin(&mut self, ui: &mut Ui, ann: &Annotation, tile_w: f32) {
        let chalk = crate::theme::NOTE_CHALK;
        let w = tile_w.max(60.0);
        let ink_h = if ann.body.trim().is_empty() {
            42.0
        } else {
            crate::theme::layout_note(ui, ann.body.as_str(), w - 12.0, chalk)
                .size()
                .y
                + 8.0
        };
        let h = ink_h.max(42.0);
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::click());

        let g = crate::theme::layout_note(ui, ann.body.as_str(), w - 12.0, chalk);
        ui.painter().galley(rect.min + vec2(6.0, 4.0), g, chalk);

        // the colored anchor dot hints the thread color on the desk
        ui.painter()
            .circle_filled(pos2(rect.left() + 2.0, rect.top() + 8.0), 3.0, note_color_ui(&ann.color));

        let resp = ui.interact(
            rect,
            egui::Id::unique(("sidebar-margin", ann.id.clone())),
            Sense::click(),
        );
        self.note_context_menu(ann, &resp);
        if resp.clicked() {
            self.go_to_page(ann.page);
        }
    }

    fn reload_annotations(&mut self) {
        self.annotations =
            crate::store::with_store(&self.store, |s| s.annotations_for_book(self.book.id))
                .unwrap_or_default();
        self.flags =
            crate::store::with_store(&self.store, |s| s.flags_for_book(self.book.id))
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

/// Stacking along a horizontal margin (top/bottom hangs): slide sideways
/// past neighbors, clamped to stay above/below the page's width.
fn stack_along_v(x_pin: f32, stack: &mut Vec<f32>, w_frac: f32) -> f32 {
    // clamp bounds can invert when the card is wider than the page
    // (w_frac > 1.16) — sort them so the clamp never panics.
    // NB: bound goes through a named var; `-0.08_f32.min(..)` would parse
    // as -(0.08_f32.min(..)) since unary minus binds looser than methods.
    let t = 1.08_f32 - w_frac;
    let mut x_c = (x_pin - w_frac * 0.45).clamp(t.min(-0.08), t.max(-0.08));
    for _ in 0..40 {
        if !stack.iter().any(|&ox| (ox - x_c).abs() < w_frac + 0.014) {
            break;
        }
        x_c += w_frac + 0.014;
    }
    let pushed = x_c.min(1.12 - w_frac).max(-0.18);
    stack.push(pushed);
    pushed
}

/// Stacking along a vertical margin (left/right hangs): slide downward
/// past neighbors.
fn stack_along_h(y_pin: f32, stack: &mut Vec<f32>, h_frac: f32) -> f32 {
    // clamp bounds can invert when the card is taller than the page —
    // same ordering guard as stack_along_v (see its comment)
    let t = 1.1_f32 - h_frac;
    let mut y_c = (y_pin - h_frac * 0.4).clamp(t.min(-0.16), t.max(-0.16));
    for _ in 0..40 {
        if !stack.iter().any(|&oy| (oy - y_c).abs() < h_frac + 0.012) {
            break;
        }
        y_c += h_frac + 0.012;
    }
    let pushed = y_c.min(1.12 - h_frac).max(-0.18);
    stack.push(pushed);
    pushed
}

/// Stable 0..1 pseudo-random from an id (per-note look, no shimmer).
fn hash01(v: u64) -> f32 {
    let x = egui::util::hash(v);
    ((x >> 11) as f64 / (1u64 << 53) as f64) as f32
}

/// Nudge a 0..1 anchor fraction; wraps toward the inside at the edge, so a
/// duplicate lands beside the original instead of stacking at a clamp.
fn nudge_frac(v: f32, d: f32) -> f32 {
    let nv = v + d;
    if nv > 0.97 { (v - d).max(0.0) } else { nv }
}

/// Pages holding at least one post-it, and pages holding at least one
/// marginalium — sorted, one entry per page; drives the ruler's page dots.
fn ruler_page_marks(anns: &[Annotation]) -> (Vec<u32>, Vec<u32>) {
    let mut reds = std::collections::BTreeSet::new();
    let mut blues = std::collections::BTreeSet::new();
    for ann in anns {
        match ann.anchor {
            Anchor::PostIt { .. } => {
                reds.insert(ann.page);
            }
            Anchor::Margin { .. } => {
                blues.insert(ann.page);
            }
        }
    }
    (reds.into_iter().collect(), blues.into_iter().collect())
}

// ===== bookmark tabs ========================================================

/// Ribbon width in screen pt, scaling gently with the page.
fn tab_ribbon_w(page_w: f32) -> f32 {
    (page_w * 0.030).clamp(13.0, 26.0)
}

/// On-page body length (the frosted part below the edge).
fn tab_body_len(page_h: f32) -> f32 {
    (page_h * 0.075).clamp(40.0, 96.0)
}

/// Overhang above the edge — the part a closed book shows.
fn tab_tip_len(body: f32) -> f32 {
    (body * 0.34).clamp(12.0, 30.0)
}

/// Deterministic 0..1 from a string id (tiny fnv, for per-tab tilt).
fn str_hash01(s: &str) -> f32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    hash01(h as u64)
}

/// Station of tab k (0 = the page's primary tab) along a sheet's top
/// edge: the page's precise address — (page-1)/(count-1), the same math
/// as the ruler — with siblings hugging right in tiny steps; the far
/// ends nudge inward just enough to keep the whole tab on the sheet.
fn flag_station_x(page: u32, page_count: u32, sheet_w: f32, cw: f32, k: usize) -> f32 {
    let count = page_count.max(1) as f32;
    let addr = if page_count <= 1 {
        0.5
    } else {
        (page as f32 - 1.0) / (count - 1.0)
    };
    let step = (cw + 3.0) / sheet_w; // the sibling's tiny gap
    let pad = (cw * 0.5 + 1.0) / sheet_w;
    (addr + step * k as f32).clamp(pad, 1.0 - pad)
}

/// Dedup rule for render requests: `pending_q` is already in flight,
/// `target_q` is wanted now. Return the quality to actually ask for —
/// None when the wish is covered or would downgrade an in-flight job.
fn request_wants(pending_q: u32, target_q: u32) -> Option<u32> {
    if target_q <= pending_q {
        None
    } else {
        Some(target_q)
    }
}

/// One page's first-planted tab wins the pennant color (tabs arrive in
/// planting order; the stored x is only a record of where its station was).
fn ruler_flag_pages(flags: &[Flag]) -> Vec<(u32, [u8; 3])> {
    let mut best: std::collections::BTreeMap<u32, [u8; 3]> = Default::default();
    for f in flags {
        best.entry(f.page).or_insert(crate::model::flag_rgb(&f.color));
    }
    best.into_iter().collect()
}

/// Just the tab's tip: shadow + colored tongue above an edge at `pivot`
/// (the point ON the edge). Bodies and the skyline share it — the sky is
/// a tab without its paper, nothing more. `lifted` grows the shadow.
fn paint_tab_tip(ui: &Ui, rgb: [u8; 3], pivot: Pos2, cw: f32, tip: f32, tilt: f32, lifted: bool) {
    let painter = ui.painter();
    let r = Rot2::from_angle(tilt);
    let pt = |v: Vec2| pivot + r * v;
    let tip_c = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    let (sx, sy, sa) = if lifted { (4.0, 6.5, 120u8) } else { (2.2, 3.2, 70u8) };
    // shadow = the rotated tongue itself, offset screen-down: it falls
    // with the tab in any orientation (hanging on the page or lying flat
    // in the strip) instead of staying in the page frame
    let shadow: Vec<Pos2> = [
        vec2(-cw * 0.5, -tip),
        vec2(cw * 0.5, -tip),
        vec2(cw * 0.5, 0.0),
        vec2(-cw * 0.5, 0.0),
    ]
    .iter()
    .map(|v| pt(*v) + vec2(sx, sy))
    .collect();
    painter.add(Shape::convex_polygon(
        shadow,
        Color32::from_black_alpha(sa),
        Stroke::NONE,
    ));
    let tip_pts = vec![
        pt(vec2(-cw * 0.5, -tip)),
        pt(vec2(cw * 0.5, -tip)),
        pt(vec2(cw * 0.5, 0.0)),
        pt(vec2(-cw * 0.5, 0.0)),
    ];
    painter.add(Shape::convex_polygon(tip_pts, tip_c, Stroke::NONE));
}

/// Paint one tab at `pivot` (the point ON the page's top edge under the
/// tab's centre). `tilt` carries the per-tab wobble — or the whole
/// quarter-turn when the tab lies down in the sidebar. `label_angle` is
/// the ink's own rotation: vertical (spine-reading) on the page, upright
/// (left-to-right) in the strip. `lifted` = being carried: straight,
/// fatter shadow.
fn paint_tab(
    ui: &Ui,
    flag: &Flag,
    pivot: Pos2,
    cw: f32,
    body: f32,
    tip: f32,
    tilt: f32,
    lifted: bool,
    hide_label: bool,
    label_angle: f32,
) {
    let painter = ui.painter();
    let r = Rot2::from_angle(tilt);
    let pt = |v: Vec2| pivot + r * v;
    let rgb = crate::model::flag_rgb(&flag.color);
    paint_tab_tip(ui, rgb, pivot, cw, tip, tilt, lifted);

    let frost = if lifted {
        Color32::from_rgba_unmultiplied(249, 249, 246, 230)
    } else {
        Color32::from_rgba_unmultiplied(247, 247, 244, 205)
    };
    let frost_line = Color32::from_rgba_unmultiplied(126, 128, 133, 130);

    // the frosted body pressed on the page — milky, page text ghosts through
    let body_pts = vec![
        pt(vec2(-cw * 0.5, 0.0)),
        pt(vec2(cw * 0.5, 0.0)),
        pt(vec2(cw * 0.5, body)),
        pt(vec2(-cw * 0.5, body)),
    ];
    painter.add(Shape::convex_polygon(
        body_pts,
        frost,
        Stroke::new(1.0, frost_line),
    ));

    // the crease: fold shadow just inside the page edge
    painter.line_segment(
        [pt(vec2(-cw * 0.5, 0.8)), pt(vec2(cw * 0.5, 0.8))],
        Stroke::new(1.4, Color32::from_black_alpha(34)),
    );

    // the label runs along the tab, reading bottom-to-top like a spine,
    // in the same handwritten pen as every note
    if !flag.label.trim().is_empty() && !hide_label {
        paint_text_rotated(
            ui,
            pt(vec2(0.0, body * 0.5)),
            label_angle,
            flag.label.trim(),
            FontId::new(
                (body * 0.16).clamp(8.5, 13.0),
                egui::FontFamily::Name(crate::theme::NOTE_FONT_FAMILY.into()),
            ),
            body - 10.0,
            Color32::from_rgb(64, 66, 72),
        );
    }
}

/// Rotated single-line text: lay out upright, then rotate the galley
/// around the position so its centre lands on `center`.
fn paint_text_rotated(
    ui: &Ui,
    center: Pos2,
    angle: f32,
    text: &str,
    font: FontId,
    max_len: f32,
    color: Color32,
) {
    let id_for = |s: f32| FontId::new(s, font.family.clone());
    let mut size = font.size;
    let mut galley = ui.painter().layout_no_wrap(text.to_owned(), id_for(size), color);
    let mut sz = galley.size();
    if sz.x > max_len && sz.x > 0.0 {
        size = (size * max_len / sz.x).max(6.0);
        galley = ui.painter().layout_no_wrap(text.to_owned(), id_for(size), color);
        sz = galley.size();
    }
    let half = Rot2::from_angle(angle) * vec2(sz.x * 0.5, sz.y * 0.5);
    let mut ts = TextShape::new(center - half, galley, color);
    ts.angle = angle;
    ui.painter().add(ts);
}

#[cfg(test)]
mod stack_tests {
    use super::*;

    /// Oversized cards (wider/taller than the page) must not panic the
    /// clamps — regression for the launch crash (f32 clamp min > max).
    #[test]
    fn stack_along_tolerates_oversized_cards() {
        for frac in [1.16_f32, 1.55, 2.0, 3.0] {
            let mut s = Vec::new();
            let x = stack_along_v(0.5, &mut s, frac);
            assert!(x.is_finite());
            let mut s = Vec::new();
            let y = stack_along_h(0.5, &mut s, frac);
            assert!(y.is_finite());
            let _ = (x, y);
        }
        // stacked neighbors with oversized cards too — one stack per axis,
        // mirroring how compute_auto_hangs keeps separate top/bottom and
        // left/right stacks
        let mut sv = Vec::new();
        let mut sh = Vec::new();
        for _ in 0..40 {
            let _ = stack_along_v(0.9, &mut sv, 2.5);
            let _ = stack_along_h(0.1, &mut sh, 2.5);
        }
        assert_eq!(sv.len(), 40);
        assert_eq!(sh.len(), 40);
    }
}

fn tinted(c: [u8; 3], factor: f32) -> Color32 {
    Color32::from_rgb(
        ((c[0] as f32 * factor).min(255.0)) as u8,
        ((c[1] as f32 * factor).min(255.0)) as u8,
        ((c[2] as f32 * factor).min(255.0)) as u8,
    )
}

/// Paper silhouette: the card's four corners, tilted and jittered per seed.
fn paper_corners(card: Rect, seed: u64, scale: f32) -> [Pos2; 4] {
    let c = card.center();
    let theta = (hash01(seed) - 0.5) * 0.0803; // ±2.3° tilt
    let base = [
        card.left_top(),
        card.right_top(),
        card.right_bottom(),
        card.left_bottom(),
    ];
    let mut out = [pos2(0.0, 0.0); 4];
    for (i, b) in base.iter().enumerate() {
        let v = *b - c;
        // per-corner orthogonality slop, capped at ±1°
        let eps = (hash01(seed + (i as u64 + 7) * 977) - 0.5) * 0.0349;
        let a = theta + eps;
        let (sn, cs) = (a.sin(), a.cos());
        out[i] = pos2(c.x + (v.x * cs - v.y * sn) * scale, c.y + (v.x * sn + v.y * cs) * scale);
    }
    out
}

/// Paint one analog post-it: layered shadow, grain mesh with edge shading,
/// and a dog-ear curl on a seeded corner.
fn paint_paper(
    painter: &egui::Painter,
    card: Rect,
    seed: u64,
    fill: [u8; 3],
    grain: &egui::TextureHandle,
    shadow: &egui::TextureHandle,
    lifted: bool,
) {
    let scale = if lifted { 1.04 } else { 1.0 };
    let corners = paper_corners(card, seed, scale);

    // blurred shadow: ONE soft gaussian sprite under an inflated quad
    let l = if lifted { 1.0 } else { 0.0 };
    let off = vec2(2.0 + 3.0 * l, 3.5 + 5.0 * l);
    let inflate = 22.0 + 9.0 * l;
    let q = card.expand2(vec2(inflate, inflate));
    let mut sm = egui::Mesh::default();
    sm.texture_id = shadow.id();
    let vc = Color32::from_black_alpha(if lifted { 255 } else { 225 });
    for (pos, uv) in [
        (q.left_top(), pos2(0.0, 0.0)),
        (q.right_top(), pos2(1.0, 0.0)),
        (q.right_bottom(), pos2(1.0, 1.0)),
        (q.left_bottom(), pos2(0.0, 1.0)),
    ] {
        sm.vertices.push(egui::epaint::Vertex {
            pos: pos + off,
            color: vc,
            uv,
        });
    }
    sm.indices.extend([0u32, 1, 2, 0, 2, 3]);
    painter.add(egui::Shape::mesh(sm));

    // the card: opaque grain tile × per-corner shading (fake gradient)
    let ns = 46.0;
    let uvs = [
        pos2(0.0, 0.0),
        pos2(card.width() / ns, 0.0),
        pos2(card.width() / ns, card.height() / ns),
        pos2(0.0, card.height() / ns),
    ];
    let shade = [1.045, 1.01, 0.94, 0.98];
    let mut mesh = egui::Mesh::default();
    mesh.texture_id = grain.id();
    for (i, pt) in corners.iter().enumerate() {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: *pt,
            color: tinted(fill, shade[i]),
            uv: uvs[i],
        });
    }
    mesh.indices.extend([0u32, 1, 2, 0, 2, 3]);
    painter.add(egui::Shape::mesh(mesh));
}

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

#[allow(dead_code)] // returns with the card-colors pass
pub fn border_color_ui(name: &str) -> Color32 {
    let [r, g, b] = darker_parts(note_color_parts(name), 0.72);
    Color32::from_rgb(r, g, b)
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

#[cfg(test)]
mod ruler_marks_tests {
    use super::*;

    fn ann(page: u32, margin: bool) -> Annotation {
        Annotation {
            id: uuid::Uuid::new_v4().to_string(),
            page,
            anchor: if margin {
                Anchor::Margin { y: 0.5 }
            } else {
                Anchor::PostIt { x: 0.5, y: 0.5, card: None, card_s: None, size: None }
            },
            body: String::new(),
            color: "yellow".into(),
            created_at: 0,
            updated_at: 0,
        }
    }

    /// The ruler shows one dot per page, not per note — a page with three
    /// post-its and a marginalium contributes exactly one red and one blue.
    #[test]
    fn ruler_marks_dedupe_by_page_and_kind() {
        let anns = vec![ann(6, false), ann(6, false), ann(6, true), ann(7, true)];
        let (reds, blues) = ruler_page_marks(&anns);
        assert_eq!(reds, vec![6]);
        assert_eq!(blues, vec![6, 7]);
        let (reds, blues) = ruler_page_marks(&[]);
        assert!(reds.is_empty() && blues.is_empty(), "empty book, empty ruler");
    }
}

#[cfg(test)]
mod margin_solve_tests {
    use super::*;

    fn run(cols: &[MarginCol]) -> Vec<(String, f32, f32)> {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(Rect::from_min_max(pos2(0.0, 0.0), pos2(900.0, 700.0)));
        let mut out = Vec::new();
        let mut full = ctx.run_ui(raw, |ui| {
            out = solve_margin_boxes(ui, 700.0, cols.to_vec());
        });
        full.textures_delta.clear(); // test pass: no renderer consumes fonts
        out
    }

    fn col(id: &str, dot_y: f32, body: &str, natural_w: f32, max_w: f32) -> MarginCol {
        MarginCol {
            id: id.into(),
            dot_y,
            body: body.into(),
            natural_w,
            max_w,
        }
    }

    fn find(out: &[(String, f32, f32)], id: &str) -> (f32, f32) {
        let (_, t, w) = out.iter().find(|(i, _, _)| i == id).unwrap();
        (*t, *w)
    }

    #[test]
    fn a_crowded_marginalia_widens_into_its_gap() {
        // ~44 chars: two rows at the natural 238px wrap, one row when wide
        let body = "alpha beta gamma delta epsilon zeta eta theta";
        let out = run(&vec![
            col("a", 100.0, &body, 238.0, 460.0),
            col("b", 140.0, "single", 238.0, 460.0),
        ]);
        let (ta, wa) = find(&out, "a");
        let (tb, _) = find(&out, "b");
        assert!(wa > 238.0, "clashing box widens (got {}px)", wa);
        assert_eq!(ta, 100.0, "top stays pinned at the dot");
        assert_eq!(tb, 140.0, "the pin sits at its dot, unmoved");
    }

    #[test]
    fn single_line_pins_refuse_and_the_movable_note_defers_past_them() {
        // long body: many rows even at the widened width cap, so remedy
        // one is exhausted and the note must give way to the pin
        let body = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi "
            .repeat(2);
        let out = run(&vec![
            col("a", 0.0, &body, 90.0, 120.0),
            col("b", 40.0, "hi", 238.0, 460.0),
        ]);
        let (ta, _) = find(&out, "a");
        let (tb, _) = find(&out, "b");
        assert_eq!(tb, 40.0, "the pin refuses to move");
        assert!(
            ta >= 40.0 + 42.0 + 4.0,
            "the movable note defers below the pin, clear of it (got {})",
            ta
        );
    }

    #[test]
    fn uncrowded_columns_keep_original_widths() {
        let out = run(&vec![
            col("a", 0.0, "hello world", 238.0, 460.0),
            col("b", 300.0, "another note", 238.0, 460.0),
        ]);
        let (ta, wa) = find(&out, "a");
        let (tb, wb) = find(&out, "b");
        assert_eq!(wa, 238.0, "no pressure, no widening");
        assert_eq!(wb, 238.0);
        assert_eq!(ta, 0.0);
        assert_eq!(tb, 300.0);
    }
}

#[cfg(test)]
mod flag_tests {
    use super::*;

    #[test]
    fn render_requests_dedupe() {
        // a cold ask goes out as-is
        assert_eq!(request_wants(0, 520), Some(520));
        assert_eq!(request_wants(0, 2000), Some(2000));
        // a full-quality job already flying covers a preview ask
        assert_eq!(request_wants(2000, 520), None);
        assert_eq!(request_wants(2000, 2000), None);
        // a preview flying upgrades to full when asked
        assert_eq!(request_wants(520, 2000), Some(2000));
    }

    #[test]
    fn stations_are_addresses_with_end_nudges_and_right_hugging_siblings() {
        let (cw, w) = (16.0, 800.0);
        // mid-book: the exact percentage, no band, no fudge
        let mid = flag_station_x(21, 41, w, cw, 0);
        assert!((mid - 0.5).abs() < 1e-6);
        // the ends nudge inward by just over half a tab width, nothing more
        let pad = (cw * 0.5 + 1.0) / w;
        assert!((flag_station_x(1, 41, w, cw, 0) - pad).abs() < 1e-6);
        assert!((flag_station_x(41, 41, w, cw, 0) - (1.0 - pad)).abs() < 1e-6);
        // a sibling hugs right with a tiny gap; it stays near the address
        let sib = flag_station_x(21, 41, w, cw, 1);
        let step = (cw + 3.0) / w;
        assert!(((sib - mid) - step).abs() < 1e-6);
        assert!(sib - mid < 0.06, "the sibling gap is tiny");
        // a single-page book centers its lone sheet
        assert!((flag_station_x(1, 1, w, cw, 0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn pennants_dedupe_per_page_and_take_the_first_tab_color() {
        let f = |page: u32, x: f32, color: &str| Flag {
            id: format!("{page}-{x}-{color}"),
            page,
            x,
            color: color.to_owned(),
            label: String::new(),
            created_at: 0,
            updated_at: 0,
        };
        let flags = vec![
            f(7, 0.5, "red"), // first planted on p.7 → red wins
            f(7, 0.2, "green"),
            f(7, 0.8, "blue"),
            f(3, 0.6, "yellow"),
        ];
        let out = ruler_flag_pages(&flags);
        assert_eq!(out.len(), 2, "one pennant per page: {out:?}");
        assert_eq!(out[0], (3, [242, 200, 56]));
        assert_eq!(out[1], (7, [232, 74, 74]));
    }
}
