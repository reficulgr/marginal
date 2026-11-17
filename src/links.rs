//! Hyperlink picking + helpers. The dialog lets the user link an annotation
//! to: another annotation, any page of a book, or an external URL.

use egui::{Color32, Pos2};

use crate::model::LinkTarget;
use crate::store::SharedStore;

pub struct LinkDialog {
    pub from: String,          // source annotation id
    pub book_fp: String,       // current book fingerprint (default target book)
    pub page: u32,             // current page (default target page)
    pub open: bool,
    pub result: Option<(String, LinkTarget)>,
    mode: u8, // 0 = annotation, 1 = page, 2 = url
    chosen_ann: usize,
    page_input: String,
    url: String,
    book_input: String,
}

impl LinkDialog {
    pub fn new(from: &str, book_fp: String, page: u32) -> LinkDialog {
        LinkDialog {
            from: from.to_string(),
            book_fp,
            page,
            open: true,
            result: None,
            mode: 0,
            chosen_ann: 0,
            page_input: page.to_string(),
            url: String::new(),
            book_input: String::new(),
        }
    }
}

pub fn show_dialog(ctx: &egui::Context, store: SharedStore, d: &mut LinkDialog) {
    let mut open = d.open;
    egui::Window::new("Add link")
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            let body = crate::store::with_store(&store, |s| -> anyhow::Result<Vec<(String, String, u32)>> {
                // (id, preview, page) for annotations of the whole library, newest first
                let mut out = Vec::new();
                for b in s.books()? {
                    for a in s.annotations_for_book(b.id)? {
                        let prev = a
                            .body
                            .lines()
                            .next()
                            .unwrap_or("(empty)")
                            .chars()
                            .take(30)
                            .collect::<String>();
                        out.push((a.id, format!("p.{}: {}", a.page, prev), a.page));
                    }
                }
                Ok(out)
            })
            .unwrap_or_default();

            ui.radio_value(&mut d.mode, 0u8, "→ Another note");
            ui.radio_value(&mut d.mode, 1u8, "→ A page");
            ui.radio_value(&mut d.mode, 2u8, "→ External URL");
            ui.separator();

            match d.mode {
                0 => {
                    d.chosen_ann = d.chosen_ann.min(body.len().saturating_sub(1));
                    if body.is_empty() {
                        ui.label("No notes exist yet anywhere.");
                    } else {
                        egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                            egui::ComboBox::from_label("note")
                                .selected_text(
                                    body.get(d.chosen_ann)
                                        .map(|(_, label, _)| label.clone())
                                        .unwrap_or_default(),
                                )
                                .show_ui(ui, |ui| {
                                    for (i, (_, label, _)) in body.iter().enumerate() {
                                        ui.selectable_value(&mut d.chosen_ann, i, label.clone());
                                    }
                                });
                        });
                    }
                }
                1 => {
                    ui.horizontal(|ui| {
                        ui.label("Book (fingerprint, blank = current):");
                        ui.add(egui::TextEdit::singleline(&mut d.book_input).desired_width(260.0));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Page:");
                        ui.add(egui::TextEdit::singleline(&mut d.page_input).desired_width(60.0));
                    });
                }
                _ => {
                    ui.horizontal(|ui| {
                        ui.label("URL:");
                        ui.add(egui::TextEdit::singleline(&mut d.url).desired_width(280.0));
                    });
                }
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Link").clicked() {
                    let target = match d.mode {
                        0 => body.get(d.chosen_ann).map(|(id, _, _)| LinkTarget::Annotation(id.clone())),
                        1 => {
                            let page_v = d.page_input.trim().parse::<u32>().unwrap_or(d.page);
                            let fp = if d.book_input.trim().is_empty() {
                                d.book_fp.clone()
                            } else {
                                d.book_input.trim().to_string()
                            };
                            Some(LinkTarget::Page { book: fp, page: page_v.max(1) })
                        }
                        _ => {
                            if !d.url.trim().is_empty() {
                                Some(LinkTarget::Url(d.url.trim().to_string()))
                            } else {
                                None
                            }
                        }
                    };
                    if let Some(t) = target {
                        d.result = Some((d.from.clone(), t));
                        d.open = false;
                    }
                }
                if ui.button("Cancel").clicked() {
                    d.open = false;
                }
            });
        });
    d.open = open;
}

/// Human-readable description of a target for panels.
pub fn describe(target: &LinkTarget) -> String {
    match target {
        LinkTarget::Annotation(_) => "→ a note".into(),
        LinkTarget::Page { book, page } => {
            let short: String = book.chars().take(8).collect();
            format!("→ p.{page} ({short}…)")
        }
        LinkTarget::Url(u) => {
            let short: String = u.chars().take(36).collect();
            format!("→ {short}")
        }
    }
}

/// A hand-drawn-looking dashed trail: the path wobbles deterministically
/// (stable for a given seed, so notes don't shimmer), and the ink is laid in
/// short dashes of slightly varying length and spacing — pencil, not ruler.
pub fn hand_dashed_line(
    painter: &egui::Painter,
    from: Pos2,
    to: Pos2,
    seed: u64,
    color: Color32,
) {
    // 1) base path: cubic bezier
    let mid_y = (from.y + to.y) * 0.5;
    let p1 = Pos2::new(from.x, mid_y);
    let p2 = Pos2::new(to.x, mid_y);

    let n = 48;
    let mut pts: Vec<Pos2> = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let mt = 1.0 - t;
        let mut x = mt * mt * mt * from.x
            + 3.0 * mt * mt * t * p1.x
            + 3.0 * mt * t * t * p2.x
            + t * t * t * to.x;
        let mut y = mt * mt * mt * from.y
            + 3.0 * mt * mt * t * p1.y
            + 3.0 * mt * t * t * p2.y
            + t * t * t * to.y;
        if i == 0 {
            x = from.x;
            y = from.y;
        }
        if i == n {
            x = to.x;
            y = to.y;
        }
        pts.push(Pos2::new(x, y));
    }

    // 2) deterministic smooth wobble perpendicular to the path.
    let p = seed as f32 * 0.618_033_99;
    let mut wobbled = pts.clone();
    for i in 1..n {
        let t = i as f32 / n as f32;
        let prev = pts[i - 1];
        let cur = pts[i];
        let next = pts[i + 1];
        let tangent = Pos2::new(next.x - prev.x, next.y - prev.y);
        let len = tangent.x.hypot(tangent.y).max(0.001);
        let nx = -tangent.y / len;
        let ny = tangent.x / len;
        // fade the wobble near the endpoints
        let fade = (3.0 * t * (1.0 - t)).min(1.0);
        let amp = 2.6 * fade;
        let wobble = amp * (0.65 * (t * 7.0 + p).sin() + 0.35 * (t * 13.0 + p * 1.7).sin());
        wobbled[i] = Pos2::new(cur.x + nx * wobble, cur.y + ny * wobble);
    }
    let pts = wobbled;

    // 3) cumulative arclength, then lay dashes and gaps along it, each piece
    //    a touch different in length — like real handwriting pressure
    let mut cumulative = Vec::with_capacity(pts.len());
    cumulative.push(0.0);
    for i in 1..pts.len() {
        cumulative.push(cumulative[i - 1] + (pts[i] - pts[i - 1]).length());
    }
    let total = *cumulative.last().unwrap_or(&0.0);
    if total <= 2.0 {
        return;
    }

    // position at arclength `s`; `cursor` resumes the search each call
    let pos_at = |s: f32, cursor: &mut usize| -> Pos2 {
        while *cursor + 1 < cumulative.len() && cumulative[*cursor + 1] < s {
            *cursor += 1;
        }
        let a = cumulative[*cursor];
        let b = cumulative.get(*cursor + 1).copied().unwrap_or(a);
        let span = (b - a).max(0.0001);
        let f = ((s - a) / span).clamp(0.0, 1.0);
        let p0 = pts[*cursor];
        let p1_ = pts.get(*cursor + 1).copied().unwrap_or(p0);
        Pos2::new(p0.x + (p1_.x - p0.x) * f, p0.y + (p1_.y - p0.y) * f)
    };

    let stroke = egui::Stroke::new(1.4, color);
    let mut cursor = 0usize;
    let mut s = 0.0;
    let mut is_ink = true;
    while s < total {
        let len = if is_ink {
            // dashes: ~4–7px, breathing like pressure
            4.5 + 1.8 * (0.5 + 0.5 * (s * 0.21 + p).sin())
        } else {
            // gaps: ~6–9px
            6.5 + 2.5 * (0.5 + 0.5 * (s * 0.17 + p * 1.3).sin())
        };
        let end_s = (s + len).min(total);
        if is_ink && end_s > s {
            let a = pos_at(s, &mut cursor);
            let b = pos_at(end_s, &mut cursor);
            painter.line_segment([a, b], stroke);
        }
        s = end_s;
        is_ink = !is_ink;
    }
}
