//! Visual theme: charcoal canvas with a bullet-journal dot grid.

use egui::{Color32, CornerRadius, Frame, Pos2, Rect, Stroke, Ui};

/// Canvas ("desk") behind pages and the library grid — near-black.
pub const CANVAS_BG: Color32 = Color32::from_rgb(15, 15, 15);
/// Slightly darker, solid chrome for panels (toolbar, status, sidebars).
pub const CHROME_BG: Color32 = Color32::from_rgb(33, 36, 41);
/// Popup / window fill.
pub const WINDOW_BG: Color32 = Color32::from_rgb(39, 42, 48);
/// Dot grid color (subtle gray on the near-black desk).
pub const DOT: Color32 = Color32::from_rgb(43, 46, 51);
/// Dot spacing in logical pixels.
pub const DOT_SPACING: f32 = 16.0;
pub const DOT_RADIUS: f32 = 1.15;

/// Margin notes hover as white chalk on the desk (no card).
pub const NOTE_CHALK: Color32 = Color32::from_rgb(238, 238, 234);
/// Connector trail from the anchor dot to marginalia — pencil tone on the desk.
pub const CONNECTOR: Color32 = Color32::from_rgb(43, 46, 51);
/// Post-it pins are always red.
pub const PIN_RED: Color32 = Color32::from_rgb(226, 72, 72);
/// Faint dashed thread between a post-it pin and its card (pencil, quite transparent).
pub fn postit_thread() -> Color32 {
    Color32::from_rgba_unmultiplied(43, 46, 51, 95)
}

/// The standard panel chrome frame.
pub fn panel_frame() -> Frame {
    Frame::new()
        .fill(CHROME_BG)
        .inner_margin(6.0)
        .corner_radius(CornerRadius::ZERO)
        .stroke(Stroke::NONE)
}

/// Paint a bullet-journal style dot grid over `rect`.
/// Dots are anchored so the grid is globally aligned (doesn't swim).
pub fn draw_dot_grid(painter: &egui::Painter, rect: Rect) {
    if rect.width() <= 1.0 || rect.height() <= 1.0 {
        return;
    }
    let spacing = DOT_SPACING;
    let offset_x = rect.left() % spacing;
    let offset_y = rect.top() % spacing;
    let cols = ((rect.width() + offset_x) / spacing).ceil() as usize;
    let rows = ((rect.height() + offset_y) / spacing).ceil() as usize;

    for row in 0..=rows {
        for col in 0..=cols {
            let x = rect.left() - offset_x + col as f32 * spacing;
            let y = rect.top() - offset_y + row as f32 * spacing;
            if x < rect.left() || y < rect.top() {
                continue;
            }
            painter.circle_filled(Pos2::new(x, y), DOT_RADIUS, DOT);
        }
    }
}

/// Apply the theme to egui's global visuals (panels, windows, text).
pub fn apply_visuals(visuals: &mut egui::Visuals) {
    visuals.panel_fill = CHROME_BG;
    visuals.window_fill = WINDOW_BG;
    visuals.extreme_bg_color = Color32::from_rgb(16, 18, 21);
    visuals.faint_bg_color = Color32::from_rgb(48, 52, 58);
}

/// Paint the canvas backdrop over the whole of `ui` (dot grid included),
/// *before* any content that should sit on top of it.
pub fn paint_canvas(ui: &Ui) {
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, CornerRadius::ZERO, CANVAS_BG);
    draw_dot_grid(&ui.painter().clone(), rect);
}

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

/// Font family name for the handwritten "Rock Salt" note font (Google Fonts,
/// Apache 2.0, © Font Diner Inc. — embedded at assets/fonts/).
pub const NOTE_FONT_FAMILY: &str = "rocksalt";
/// Default on-note text size for the handwritten font.
pub const NOTE_FONT_SIZE: f32 = 14.0;

/// A FontId for note text (handwritten).
pub fn note_font() -> egui::FontId {
    egui::FontId::new(
        NOTE_FONT_SIZE,
        egui::FontFamily::Name(NOTE_FONT_FAMILY.into()),
    )
}

/// Register the embedded fonts with egui. Call once at startup.
pub fn register_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "glossa-rocksalt".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/RockSalt-Regular.ttf"
        ))),
    );
    // dedicated family, used explicitly for note text
    fonts
        .families
        .entry(egui::FontFamily::Name(NOTE_FONT_FAMILY.into()))
        .or_default()
        .push("glossa-rocksalt".into());
    ctx.set_fonts(fonts);
}

/// Line height for handwritten note text: one step tighter than the font's
/// natural row height. Dial in `theme.rs`: smaller = tighter.
pub const NOTE_LEADING: f32 = 18.0;

/// Lay out note text in the handwritten font with the tightened leading.
pub fn layout_note(
    ui: &egui::Ui,
    text: &str,
    wrap_width: f32,
    color: Color32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::text::TextFormat {
            font_id: note_font(),
            color,
            line_height: Some(NOTE_LEADING),
            ..Default::default()
        },
    );
    job.wrap.max_width = wrap_width;
    ui.fonts_mut(|f| f.layout_job(job))
}
