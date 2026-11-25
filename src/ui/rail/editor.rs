//! The look editor of an account tile: name, icon and color.

use egui::{Sense, vec2};

use crate::account_look::{ICONS, Look, SWATCHES};
use crate::settings::AccountLook;
use crate::theme::{self, Icon};

const CELL: f32 = 28.0;
const SWATCH: f32 = 20.0;

/// A name the person typed: blank (or only spaces) means the automatic name.
fn name_value(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn parse_hex(s: &str) -> Option<egui::Color32> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(egui::Color32::from_rgb(
        (v >> 16) as u8,
        (v >> 8) as u8,
        v as u8,
    ))
}

/// Draws the editor for one account. `look` is what the tile shows now,
/// `saved` what is stored. Returns the new saved look in the frame something
/// changed, otherwise `None`.
pub(crate) fn look_editor(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    look: &Look,
    saved: &AccountLook,
) -> Option<AccountLook> {
    let mut next = saved.clone();
    let mut changed = false;

    // Name: the buffer lives in egui's memory while the editor is open.
    let id = ui.id().with("look-name");
    let mut text = ui
        .data_mut(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| saved.name.clone().unwrap_or_default());
    ui.label(
        egui::RichText::new("Name")
            .font(theme::semibold(12.0))
            .color(palette.dim),
    );
    let edit = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text(look.name.as_str())
            .char_limit(24)
            .desired_width(f32::INFINITY),
    );
    if edit.changed() {
        next.name = name_value(&text);
        changed = true;
    }
    ui.data_mut(|d| d.insert_temp(id, text));

    // Icon: initials, or one of the icons.
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new("Icon")
            .font(theme::semibold(12.0))
            .color(palette.dim),
    );
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        let mut cell = |ui: &mut egui::Ui, name: Option<&str>| {
            let (rect, response) = ui.allocate_exact_size(vec2(CELL, CELL), Sense::click());
            let selected = saved.icon.as_deref() == name;
            if selected || response.hovered() {
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::same(6),
                    if selected {
                        palette.surface_active
                    } else {
                        palette.surface_hover
                    },
                );
            }
            let tint = if selected {
                palette.accent
            } else {
                palette.text
            };
            match name.and_then(Icon::for_account) {
                Some(icon) => {
                    icon.image(tint, 16.0).paint_at(
                        ui,
                        egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(16.0)),
                    );
                }
                None => {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        &look.initials,
                        theme::bold(11.0),
                        tint,
                    );
                }
            }
            let response = response
                .on_hover_text(name.map_or("Initials".to_owned(), |n| n.replace('-', " ")))
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() && !selected {
                next.icon = name.map(str::to_owned);
                changed = true;
            }
        };
        cell(ui, None);
        for name in ICONS {
            cell(ui, Some(name));
        }
    });

    // Color: ten swatches, the current one ringed.
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new("Color")
            .font(theme::semibold(12.0))
            .color(palette.dim),
    );
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for hex in SWATCHES {
            let Some(color) = parse_hex(hex) else {
                continue;
            };
            let (rect, response) = ui.allocate_exact_size(vec2(SWATCH, SWATCH), Sense::click());
            let selected = saved.color.as_deref() == Some(hex) || look.color == color;
            ui.painter()
                .circle_filled(rect.center(), SWATCH / 2.0, color);
            if selected {
                ui.painter().circle_stroke(
                    rect.center(),
                    SWATCH / 2.0 + 2.0,
                    egui::Stroke::new(1.5, palette.text),
                );
            }
            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() {
                next.color = Some(hex.to_owned());
                changed = true;
            }
        }
    });

    changed.then_some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look() -> Look {
        Look {
            name: "Contoso".into(),
            initials: "C".into(),
            icon: None,
            color: parse_hex(SWATCHES[0]).unwrap(),
        }
    }

    #[test]
    fn look_editor_returns_none_without_input() {
        let ctx = egui::Context::default();
        crate::ui::install(&ctx, true);
        let palette = theme::Palette::dark();
        let look = look();
        let saved = AccountLook::default();
        let mut out = Some(AccountLook::default());
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            out = look_editor(ui, &palette, &look, &saved);
        });
        output.textures_delta.clear();
        assert_eq!(out, None);
    }

    #[test]
    fn look_editor_blank_name_is_none() {
        assert_eq!(name_value("   "), None);
        assert_eq!(name_value(""), None);
        assert_eq!(name_value("  Work "), Some("Work".to_owned()));
    }
}
