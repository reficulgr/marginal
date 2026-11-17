//! Headless egui interaction tests — no display needed.
//!
//! egui hit-tests at the *start* of a pass using the widget rects from the
//! previous pass, so every test warms up with a couple of idle frames to
//! mimic the steady frame loop a real window provides.

use egui::*;

const PAGE: Rect = Rect::from_min_max(Pos2::new(50.0, 50.0), Pos2::new(650.0, 850.0));
const CARD: Rect = Rect::from_min_max(Pos2::new(300.0, 300.0), Pos2::new(450.0, 410.0));

fn mouse_move(input: &mut RawInput, pos: Pos2) {
    input.events.push(Event::PointerMoved(pos));
}

fn mouse_down(input: &mut RawInput, pos: Pos2) {
    mouse_move(input, pos);
    input.events.push(Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
}

fn mouse_up(input: &mut RawInput, pos: Pos2) {
    mouse_move(input, pos);
    input.events.push(Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
}

fn typed(input: &mut RawInput, s: &str) {
    input.events.push(Event::Text(s.to_string()));
}

fn press_escape(input: &mut RawInput) {
    input.events.push(Event::Key {
        key: Key::Escape,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
        physical_key: None,
    });
}

/// One egui frame with synthetic input; consumes texture deltas so the
/// headless context can be dropped without panicking.
fn frame(ctx: &Context, input: RawInput, draw: impl FnOnce(&mut Ui)) {
    let mut draw = Some(draw);
    let mut out = ctx.run_ui(input, move |ui| {
        if let Some(d) = draw.take() {
            d(ui);
        }
    });
    out.textures_delta.clear();
}

fn base_input() -> RawInput {
    let mut input = RawInput::default();
    input.screen_rect = Some(Rect::from_min_size(Default::default(), Vec2::new(800.0, 1000.0)));
    input
}

struct CardHud {
    hovered: bool,
    clicked: bool,
    double_clicked: bool,
    dragged: bool,
    drag_started: bool,
    drag_stopped: bool,
    drag_delta: Vec2,
}

impl CardHud {
    fn new() -> Self {
        Self {
            hovered: false,
            clicked: false,
            double_clicked: false,
            dragged: false,
            drag_started: false,
            drag_stopped: false,
            drag_delta: Vec2::ZERO,
        }
    }
}

fn card_hud() -> CardHud {
    CardHud::new()
}

/// The idiom the reader uses for post-it cards: painter for visuals,
/// `advance_cursor_after_rect` + `ui.interact` with a stable id for input.
fn draw_card(ui: &mut Ui, hud: &mut CardHud) {
    // page behind
    let (page_rect, _page_resp) = ui.allocate_exact_size(PAGE.size(), Sense::click());
    ui.painter()
        .rect_filled(page_rect, CornerRadius::ZERO, Color32::WHITE);

    // the card, at an absolute position
    ui.advance_cursor_after_rect(CARD);
    let resp = ui.interact(CARD, egui::Id::unique(("postit-card", "fixed-test-id")), Sense::click_and_drag());
    hud.hovered = resp.hovered();
    hud.clicked = resp.clicked();
    hud.double_clicked = resp.double_clicked();
    hud.dragged = resp.dragged();
    hud.drag_started = resp.drag_started();
    hud.drag_stopped = resp.drag_stopped();
    hud.drag_delta = resp.drag_delta();
}

/// Warm the context: widgets exist, pointer hovering the card center.
fn warm_up_card(ctx: &Context, input: &mut RawInput, hud: &mut CardHud, frames: usize) {
    mouse_move(input, CARD.center());
    for _ in 0..frames {
        frame(ctx, input.take(), |ui| draw_card(ui, hud));
    }
}

#[test]
fn card_hover_works() {
    let ctx = Context::default();
    let mut hud = card_hud();
    let mut input = base_input();
    warm_up_card(&ctx, &mut input, &mut hud, 2);
    frame(&ctx, input.take(), |ui| draw_card(ui, &mut hud));
    assert!(hud.hovered, "card must report hover in steady state");
}

#[test]
fn card_click_works() {
    let ctx = Context::default();
    let mut hud = card_hud();
    let mut input = base_input();
    warm_up_card(&ctx, &mut input, &mut hud, 2);

    mouse_down(&mut input, CARD.center());
    frame(&ctx, input.take(), |ui| draw_card(ui, &mut hud));
    mouse_up(&mut input, CARD.center());
    frame(&ctx, input.take(), |ui| draw_card(ui, &mut hud));
    assert!(hud.clicked, "click on the card must register");
}

#[test]
fn card_double_click_works() {
    let ctx = Context::default();
    let mut hud = card_hud();
    let mut input = base_input();
    warm_up_card(&ctx, &mut input, &mut hud, 2);

    for _ in 0..2 {
        mouse_down(&mut input, CARD.center());
        frame(&ctx, input.take(), |ui| draw_card(ui, &mut hud));
        mouse_up(&mut input, CARD.center());
        frame(&ctx, input.take(), |ui| draw_card(ui, &mut hud));
    }
    // multi-click state updates over the next frame or two
    let mut saw_double = hud.double_clicked;
    for _ in 0..3 {
        frame(&ctx, input.take(), |ui| draw_card(ui, &mut hud));
        saw_double |= hud.double_clicked;
    }
    assert!(
        saw_double,
        "a fast double-press on the card must register (timing or tracking bug)"
    );
}

#[test]
fn card_drag_works() {
    let ctx = Context::default();
    let mut hud = card_hud();
    let mut input = base_input();
    warm_up_card(&ctx, &mut input, &mut hud, 2);

    mouse_down(&mut input, CARD.center());
    let mut saw_drag = false;
    let mut drag_vec = Vec2::ZERO;
    for step in 1..=6 {
        mouse_move(
            &mut input,
            CARD.center() + Vec2::new(10.0 * step as f32, 4.0 * step as f32),
        );
        frame(&ctx, input.take(), |ui| draw_card(ui, &mut hud));
        if hud.dragged {
            saw_drag = true;
            drag_vec += hud.drag_delta;
        }
    }
    mouse_up(&mut input, CARD.center() + Vec2::new(60.0, 24.0));
    frame(&ctx, input.take(), |ui| draw_card(ui, &mut hud));

    println!("[drag] saw_started={} saw_drag={saw_drag} drag_vec={drag_vec:?}", hud.drag_started);
    assert!(saw_drag, "pressing and moving on the card must yield drags");
    assert!(drag_vec.x > 20.0, "drag delta must accumulate");
}

#[test]
fn textedit_receives_typing() {
    let ctx = Context::default();
    let edit_id = egui::Id::unique(("edit-note", "test"));
    let mut buf = String::new();
    let mut input = base_input();
    mouse_move(&mut input, CARD.center());

    // The app requests focus every frame the editor is alive.
    // Warm-up frames also render the editor so focus persists.
    for _ in 0..2 {
        frame(&ctx, input.take(), |ui| {
            ui.put(
                CARD.shrink2(vec2(7.0, 5.0)),
                TextEdit::multiline(&mut buf).id(edit_id),
            );
            ui.memory_mut(|m| m.request_focus(edit_id));
        });
    }

    // typing frame (editor still rendered + focused each frame)
    typed(&mut input, "hello");
    let mut focused = false;
    frame(&ctx, input.take(), |ui| {
        let resp = ui.put(
            CARD.shrink2(vec2(7.0, 5.0)),
            TextEdit::multiline(&mut buf).id(edit_id),
        );
        ui.memory_mut(|m| m.request_focus(edit_id));
        focused = resp.has_focus();
    });
    println!("[focus] focused={focused} buf={buf:?}");
    assert!(focused, "text edit must hold focus while re-requested");
    assert!(buf.contains("hello"), "typing must reach the buffer (got {:?})", buf);
}

/// Escape surrenders focus on a focused TextEdit (commit trigger #1).
#[test]
fn escape_ends_editing() {
    let ctx = Context::default();
    let edit_id = egui::Id::unique(("edit-note", "test2"));
    let mut buf = String::new();
    let mut input = base_input();

    // editor + focus
    frame(&ctx, input.take(), |ui| {
        ui.put(
            CARD.shrink2(vec2(7.0, 5.0)),
            TextEdit::multiline(&mut buf).id(edit_id),
        );
        ui.memory_mut(|m| m.request_focus(edit_id));
    });
    // steady frames
    for _ in 0..2 {
        frame(&ctx, input.take(), |ui| {
            ui.put(
                CARD.shrink2(vec2(7.0, 5.0)),
                TextEdit::multiline(&mut buf).id(edit_id),
            );
            ui.memory_mut(|m| m.request_focus(edit_id));
        });
    }

    // escape → next frame the editor must report lost_focus
    press_escape(&mut input);
    let mut lost = false;
    frame(&ctx, input.take(), |ui| {
        let resp = ui.put(
            CARD.shrink2(vec2(7.0, 5.0)),
            TextEdit::multiline(&mut buf).id(edit_id),
        );
        lost = resp.lost_focus();
    });
    assert!(lost, "pressing Escape must make the TextEdit lose focus");
}
