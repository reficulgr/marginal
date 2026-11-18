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
fn context_menu_keeps_hover_alive() {
    // guards the right-click work: attaching a context menu to the card
    // response must not cost the card its hover state
    let ctx = Context::default();
    let mut hud = card_hud();
    let mut input = base_input();
    mouse_move(&mut input, CARD.center());
    for _ in 0..3 {
        frame(&ctx, input.take(), |ui| {
            ui.advance_cursor_after_rect(CARD);
            let resp = ui.interact(CARD, egui::Id::unique(("m", "1")), Sense::click_and_drag());
            resp.context_menu(|ui| {
                if ui.button("Duplicate note").clicked() {
                    ui.close();
                }
                if ui.button("Delete note").clicked() {
                    ui.close();
                }
            });
            hud.hovered = resp.hovered();
        });
    }
    assert!(hud.hovered, "attaching a context menu must not break card hover");
}

/// The bar the reader draws above a hovered note.
fn bar_rect() -> Rect {
    let bar_h = 18.0_f32; // reader's BAR_H
    Rect::from_min_size(
        pos2(CARD.min.x, CARD.min.y - bar_h - 4.0),
        vec2(CARD.width(), bar_h),
    )
}

#[test]
fn hover_bar_button_senses_input() {
    let ctx = Context::default();
    let mut input = base_input();
    let bar = bar_rect();
    let mut hovered = false;
    let mut clicked = false;

    let draw = |ui: &mut Ui, hovered: &mut bool, clicked: &mut bool| {
        ui.advance_cursor_after_rect(CARD);
        let resp = ui.interact(CARD, egui::Id::unique(("c", "1")), Sense::click_and_drag());
        let bar_hovered = ui.rect_contains_pointer(bar) || ui.rect_contains_pointer(CARD);
        let gate_open = resp.hovered() || bar_hovered;
        // a neighbor drawn after the card, covering the bar zone (margin
        // text, a ruler, a thread endpoint — the things that killed the bar)
        ui.interact(
            bar.translate(vec2(40.0, 2.0)),
            egui::Id::unique(("nbr", "1")),
            Sense::click_and_drag(),
        );
        // the hover-bar pass — last, so it can't be covered
        if !gate_open {
            return;
        }
        let cell = |ix: f32| {
            Rect::from_min_size(pos2(bar.min.x + ix, bar.min.y), vec2(22.0, bar.height()))
        };
        let r = ui.put(cell(0.0), egui::Button::new("✎").small());
        *hovered |= r.hovered();
        *clicked |= r.clicked();
        let _ = ui.put(cell(26.0), egui::Button::new("🔗").small());
        let _ = ui.put(cell(52.0), egui::Button::new("🗑").small());
    };

    // warm on the card, then park the pointer on the first bar button
    mouse_move(&mut input, CARD.center());
    for _ in 0..2 {
        frame(&ctx, input.take(), |ui| draw(ui, &mut hovered, &mut clicked));
    }
    mouse_move(&mut input, pos2(bar.min.x + 11.0, bar.center().y));
    for _ in 0..3 {
        frame(&ctx, input.take(), |ui| draw(ui, &mut hovered, &mut clicked));
    }
    assert!(hovered, "the ✎ button inside the hover bar must sense the pointer");
    assert!(!clicked, "no click was sent in this test");
}

#[test]
fn hover_bar_button_clicks_among_page_interacts() {
    // mirrors the desk layering: page interact first, then the card (with a
    // context menu attached), then the hover-bar cells placed with `put`
    let ctx = Context::default();
    let mut input = base_input();
    let bar = bar_rect();
    let mut clicked = false;

    let draw = |ui: &mut Ui, clicked: &mut bool| {
        ui.allocate_exact_size(PAGE.size(), Sense::click_and_drag()); // the page below
        ui.advance_cursor_after_rect(CARD);
        let resp = ui.interact(CARD, egui::Id::unique(("c", "1")), Sense::click_and_drag());
        resp.context_menu(|_| {});
        let bar_hovered = ui.rect_contains_pointer(bar) || ui.rect_contains_pointer(CARD);
        // a neighbor covering the bar zone, drawn after the card
        ui.interact(
            bar.translate(vec2(40.0, 2.0)),
            egui::Id::unique(("nbr", "1")),
            Sense::click_and_drag(),
        );
        // the hover-bar pass — last, above every prior widget
        if resp.hovered() || bar_hovered {
            let cell = Rect::from_min_size(pos2(bar.min.x + 26.0, bar.min.y), vec2(22.0, bar.height()));
            let r = ui.put(cell, egui::Button::new("🔗").small());
            *clicked |= r.clicked();
        }
    };

    let link_pos = pos2(bar.min.x + 37.0, bar.center().y);
    mouse_move(&mut input, CARD.center());
    for _ in 0..2 {
        frame(&ctx, input.take(), |ui| draw(ui, &mut clicked));
    }
    mouse_move(&mut input, link_pos);
    frame(&ctx, input.take(), |ui| draw(ui, &mut clicked));
    mouse_down(&mut input, link_pos);
    frame(&ctx, input.take(), |ui| draw(ui, &mut clicked));
    mouse_up(&mut input, link_pos);
    frame(&ctx, input.take(), |ui| draw(ui, &mut clicked));
    assert!(clicked, "the 🔗 hover-bar cell must receive the click");
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
