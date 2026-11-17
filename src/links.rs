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

/// Draw a little curved connector between two points (used by reader).
pub fn draw_connector(
    painter: &egui::Painter,
    from: Pos2,
    to: Pos2,
    color: Color32,
) {
    let mut pts = Vec::with_capacity(25);
    let dy = (to.y - from.y) * 0.3;
    for i in 0..=24 {
        let t = i as f32 / 24.0;
        let mt = 1.0 - t;
        let x = mt * from.x + t * to.x;
        let y = mt * mt * from.y + 2.0 * mt * t * ((from.y + to.y) * 0.5 + dy) + t * t * to.y;
        pts.push(Pos2::new(x, y));
    }
    painter.line(pts, egui::Stroke::new(1.8, color));
    painter.circle_filled(to, 3.0, color);
}
