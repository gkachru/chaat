//! Draws a parsed Adaptive Card (display only).

use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke, vec2};

use crate::cards::{Card, CardAction, Element, TextSize};
use crate::model::Action;
use crate::theme;
use crate::ui::widgets;

const PADDING: f32 = 10.0;
const GAP: f32 = 8.0;
const HINT: &str = "Open in Teams to use this part";

/// Draws `card` in `width` points; links push actions.
pub fn show(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    card: &Card,
    width: f32,
    actions: &mut Vec<Action>,
) {
    let inner = (width - 2.0 * PADDING - 2.0).max(40.0);
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(PADDING as i8))
        .show(ui, |ui| {
            ui.set_width(inner);
            ui.spacing_mut().item_spacing = vec2(GAP, 6.0);
            for (i, element) in card.body.iter().enumerate() {
                ui.push_id(i, |ui| element_ui(ui, palette, element, actions));
            }
            if !card.actions.is_empty() {
                ui.push_id("card-actions", |ui| {
                    action_row(ui, palette, &card.actions, actions)
                });
            }
        });
}

/// Point size of a card text size.
fn size_points(size: TextSize) -> f32 {
    match size {
        TextSize::Small => 12.0,
        TextSize::Default => 13.0,
        TextSize::Medium => 15.0,
        TextSize::Large => 18.0,
        TextSize::ExtraLarge => 20.0,
    }
}

fn text_ui(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    text: &str,
    points: f32,
    bold: bool,
    subtle: bool,
) {
    let font = if bold {
        theme::bold(points)
    } else {
        theme::regular(points)
    };
    let color = if subtle {
        palette.secondary
    } else {
        palette.text
    };
    ui.add(egui::Label::new(RichText::new(text).font(font).color(color)).wrap());
}

fn hint_ui(ui: &mut egui::Ui, palette: &theme::Palette) {
    text_ui(ui, palette, HINT, 12.0, false, true);
}

fn is_web(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

fn element_ui(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    element: &Element,
    actions: &mut Vec<Action>,
) {
    match element {
        Element::Text {
            text,
            size,
            bold,
            subtle,
        } => text_ui(ui, palette, text, size_points(*size), *bold, *subtle),
        Element::Facts(facts) => {
            egui::Grid::new("facts")
                .num_columns(2)
                .spacing(vec2(12.0, 4.0))
                .min_row_height(18.0)
                .show(ui, |ui| {
                    for (title, value) in facts {
                        // The title keeps its line (a wrapping label in a grid
                        // column shrinks to a letter per line); the value wraps.
                        ui.add(
                            egui::Label::new(
                                RichText::new(title)
                                    .font(theme::bold(13.0))
                                    .color(palette.text),
                            )
                            .extend(),
                        );
                        text_ui(ui, palette, value, 13.0, false, false);
                        ui.end_row();
                    }
                });
        }
        Element::Container(children) => {
            for (i, child) in children.iter().enumerate() {
                ui.push_id(i, |ui| element_ui(ui, palette, child, actions));
            }
        }
        Element::Columns(columns) => {
            if columns.is_empty() {
                return;
            }
            let count = columns.len() as f32;
            let each = ((ui.available_width() - GAP * (count - 1.0)) / count).max(20.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                for (c, column) in columns.iter().enumerate() {
                    ui.push_id(c, |ui| {
                        ui.allocate_ui_with_layout(
                            vec2(each, 0.0),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ui.set_width(each);
                                for (i, child) in column.iter().enumerate() {
                                    ui.push_id(i, |ui| element_ui(ui, palette, child, actions));
                                }
                            },
                        );
                    });
                }
            });
        }
        Element::Image { alt, url } => {
            let label = alt.as_deref().filter(|a| !a.is_empty()).unwrap_or("Image");
            let size = vec2(ui.available_width().min(240.0), 72.0);
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            ui.painter().rect(
                rect,
                CornerRadius::same(6),
                palette.panel,
                Stroke::new(1.0, palette.outline),
                egui::StrokeKind::Inside,
            );
            let galley = ui.painter().layout(
                label.to_owned(),
                theme::regular(12.0),
                palette.secondary,
                (rect.width() - 12.0).max(10.0),
            );
            let pos = rect.center() - galley.size() / 2.0;
            ui.painter().galley(pos, galley, palette.secondary);
            if let Some(url) = url.as_deref().filter(|u| is_web(u)) {
                let response = widgets::text_button(ui, palette, "Open image", theme::medium(12.0));
                widgets::on_click(&response, actions, Action::OpenUrl(url.to_owned()));
            }
        }
        Element::Actions(list) => action_row(ui, palette, list, actions),
        Element::Unsupported(_) => hint_ui(ui, palette),
    }
}

fn action_row(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    list: &[CardAction],
    actions: &mut Vec<Action>,
) {
    let mut unsupported = false;
    ui.horizontal_wrapped(|ui| {
        for action in list {
            match action {
                CardAction::OpenUrl { title, url } => {
                    let label = if title.is_empty() { "Open" } else { title };
                    let response = widgets::secondary_button(ui, palette, label);
                    widgets::on_click(&response, actions, Action::OpenUrl(url.clone()));
                }
                CardAction::Unsupported { .. } => unsupported = true,
            }
        }
    });
    if unsupported {
        hint_ui(ui, palette);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::Shape;

    fn text(t: &str) -> Element {
        Element::Text {
            text: t.into(),
            size: TextSize::Default,
            bold: false,
            subtle: false,
        }
    }

    fn flatten<'a>(shape: &'a Shape, out: &mut Vec<&'a Shape>) {
        if let Shape::Vec(list) = shape {
            list.iter().for_each(|s| flatten(s, out));
        } else {
            out.push(shape);
        }
    }

    struct Frame1 {
        shapes: Vec<Shape>,
        textures: usize,
    }

    fn run(
        ctx: &egui::Context,
        card: &Card,
        events: Vec<egui::Event>,
        actions: &mut Vec<Action>,
    ) -> Frame1 {
        let palette = theme::Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                vec2(600.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            show(ui, &palette, card, 400.0, actions);
        });
        let textures = out
            .textures_delta
            .set
            .iter()
            .filter(|(id, _)| **id != egui::TextureId::Managed(0))
            .count();
        out.textures_delta.clear();
        Frame1 {
            textures,
            shapes: out.shapes.into_iter().map(|c| c.shape).collect(),
        }
    }

    fn new_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx, true);
        ctx
    }

    fn texts(frame: &Frame1) -> Vec<(String, f32)> {
        let mut flat = Vec::new();
        frame.shapes.iter().for_each(|s| flatten(s, &mut flat));
        flat.into_iter()
            .filter_map(|s| match s {
                Shape::Text(t) => Some((
                    t.galley.job.text.clone(),
                    t.galley.job.sections[0].format.font_id.size,
                )),
                _ => None,
            })
            .collect()
    }

    fn pos_of(frame: &Frame1, wanted: &str) -> Option<egui::Pos2> {
        let mut flat = Vec::new();
        frame.shapes.iter().for_each(|s| flatten(s, &mut flat));
        flat.into_iter().find_map(|s| match s {
            Shape::Text(t) if t.galley.job.text == wanted => {
                Some(t.pos + t.galley.rect.center().to_vec2())
            }
            _ => None,
        })
    }

    fn click(ctx: &egui::Context, card: &Card, at: egui::Pos2, actions: &mut Vec<Action>) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        run(
            ctx,
            card,
            vec![egui::Event::PointerMoved(at), button(true)],
            actions,
        );
        run(ctx, card, vec![button(false)], actions);
    }

    #[test]
    fn openurl_button_pushes_open_action() {
        let ctx = new_ctx();
        let card = Card {
            body: vec![],
            actions: vec![CardAction::OpenUrl {
                title: "Go there".into(),
                url: "https://example.com/x".into(),
            }],
        };
        let mut actions = Vec::new();
        run(&ctx, &card, vec![], &mut actions);
        let frame = run(&ctx, &card, vec![], &mut actions);
        let at = pos_of(&frame, "Go there").expect("button label painted");
        click(&ctx, &card, at, &mut actions);
        assert!(matches!(
            actions.as_slice(),
            [Action::OpenUrl(u)] if u == "https://example.com/x"
        ));
    }

    #[test]
    fn image_draws_placeholder_not_fetch() {
        let ctx = new_ctx();
        let card = Card {
            body: vec![Element::Image {
                alt: Some("Team photo".into()),
                url: Some("https://example.com/a.png".into()),
            }],
            actions: vec![],
        };
        let mut actions = Vec::new();
        run(&ctx, &card, vec![], &mut actions);
        let frame = run(&ctx, &card, vec![], &mut actions);
        let painted = texts(&frame);
        assert!(painted.iter().any(|(t, _)| t == "Team photo"));
        assert!(painted.iter().any(|(t, _)| t == "Open image"));
        assert_eq!(frame.textures, 0);
        let mut flat = Vec::new();
        frame.shapes.iter().for_each(|s| flatten(s, &mut flat));
        assert!(!flat.iter().any(|s| matches!(s, Shape::Mesh(_))));
        assert!(actions.is_empty());
    }

    #[test]
    fn image_without_web_url_has_no_open_link() {
        let ctx = new_ctx();
        let card = Card {
            body: vec![Element::Image {
                alt: None,
                url: Some("data:image/png;base64,AAAA".into()),
            }],
            actions: vec![],
        };
        let mut actions = Vec::new();
        run(&ctx, &card, vec![], &mut actions);
        let painted = texts(&run(&ctx, &card, vec![], &mut actions));
        assert!(painted.iter().any(|(t, _)| t == "Image"));
        assert!(!painted.iter().any(|(t, _)| t == "Open image"));
    }

    #[test]
    fn unsupported_shows_open_in_teams_hint() {
        let ctx = new_ctx();
        let card = Card {
            body: vec![Element::Unsupported("Input.Text".into())],
            actions: vec![CardAction::Unsupported {
                title: "Submit".into(),
            }],
        };
        let mut actions = Vec::new();
        run(&ctx, &card, vec![], &mut actions);
        let painted = texts(&run(&ctx, &card, vec![], &mut actions));
        let hints = painted.iter().filter(|(t, _)| t == HINT).count();
        assert_eq!(hints, 2);
        assert!(!painted.iter().any(|(t, _)| t == "Submit"));
    }

    #[test]
    fn sizes_map_to_points() {
        assert_eq!(size_points(TextSize::Small), 12.0);
        assert_eq!(size_points(TextSize::Default), 13.0);
        assert_eq!(size_points(TextSize::Medium), 15.0);
        assert_eq!(size_points(TextSize::Large), 18.0);
        assert_eq!(size_points(TextSize::ExtraLarge), 20.0);

        let ctx = new_ctx();
        let sized = |t: &str, size| Element::Text {
            text: t.into(),
            size,
            bold: false,
            subtle: false,
        };
        let card = Card {
            body: vec![
                sized("s", TextSize::Small),
                sized("d", TextSize::Default),
                sized("m", TextSize::Medium),
                sized("l", TextSize::Large),
                sized("x", TextSize::ExtraLarge),
                text("plain"),
            ],
            actions: vec![],
        };
        let mut actions = Vec::new();
        run(&ctx, &card, vec![], &mut actions);
        let painted = texts(&run(&ctx, &card, vec![], &mut actions));
        let size_of = |t: &str| painted.iter().find(|(p, _)| p == t).unwrap().1;
        assert_eq!(size_of("s"), 12.0);
        assert_eq!(size_of("d"), 13.0);
        assert_eq!(size_of("m"), 15.0);
        assert_eq!(size_of("l"), 18.0);
        assert_eq!(size_of("x"), 20.0);
    }

    #[test]
    fn facts_columns_and_containers_draw_their_text() {
        let ctx = new_ctx();
        let card = Card {
            body: vec![
                Element::Facts(vec![("Owner".into(), "Ana".into())]),
                Element::Columns(vec![vec![text("left")], vec![text("right")]]),
                Element::Container(vec![text("inside")]),
            ],
            actions: vec![],
        };
        let mut actions = Vec::new();
        run(&ctx, &card, vec![], &mut actions);
        let painted = texts(&run(&ctx, &card, vec![], &mut actions));
        for want in ["Owner", "Ana", "left", "right", "inside"] {
            assert!(painted.iter().any(|(t, _)| t == want), "{want}");
        }
    }
}
