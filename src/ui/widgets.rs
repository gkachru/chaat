//! Small shared controls.

use egui::{Align2, Color32, CornerRadius, Response, Sense, Stroke, Ui, Vec2, vec2};

use crate::app::{App, Image};
use crate::model::Action;
use crate::theme::{self, Icon, Palette};

/// A padded single-line field with identical placeholder and input metrics.
pub fn text_field<'a>(text: &'a mut String, hint: &str, font: egui::FontId) -> egui::TextEdit<'a> {
    egui::TextEdit::singleline(text)
        // TextEdit's hint atoms otherwise use the body font independently.
        .hint_text(egui::RichText::new(hint).font(font.clone()))
        .font(font)
        .margin(egui::Margin::symmetric(8, 6))
        .desired_width(f32::INFINITY)
}

/// Loading is event driven. A static indicator lets a slow request leave the
/// entire window asleep until its backend sends the next event.
pub fn loading(ui: &mut Ui, color: Color32, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        Icon::Loader.image(color, size).paint_at(ui, rect);
    }
    response.on_hover_text("Loading…")
}

/// A round avatar: the person's photo when Chaat has it, their initials
/// otherwise, and a presence dot when `presence` is known.
pub fn avatar(
    ui: &mut Ui,
    app: &App,
    user: Option<&str>,
    name: &str,
    size: f32,
    presence: bool,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let palette = &app.palette;
    let photo = user.and_then(|u| match app.photos.get(u) {
        Some(Image::Ready(uri)) => Some(uri.clone()),
        _ => None,
    });
    match &photo {
        Some(uri) => {
            egui::Image::new(uri.clone())
                .corner_radius(CornerRadius::same((size / 2.0) as u8))
                .paint_at(ui, rect);
        }
        None => {
            let seed = user.unwrap_or(name);
            ui.painter().circle_filled(
                rect.center(),
                size / 2.0,
                theme::avatar_color(seed, palette.dark),
            );
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                theme::initials(name),
                theme::semibold((size * 0.38).round()),
                Color32::WHITE,
            );
        }
    }
    // Ask for the photo once the avatar is on screen.
    let missing_photo = match photo {
        Some(ref uri) => crate::images::needs_source(
            ui.ctx(),
            uri,
            egui::load::SizeHint::Width((size * ui.ctx().pixels_per_point()).ceil() as u32),
        ),
        None => user.is_some_and(|user| !app.photos.contains_key(user)),
    };
    if let Some(user) = user
        && !app.demo
        && missing_photo
    {
        ui.ctx().data_mut(|data| {
            data.get_temp_mut_or_default::<Vec<String>>(egui::Id::new("chaat-photo-wants"))
                .push(user.to_owned())
        });
    }
    if presence
        && let Some(user) = user
        && let Some(state) = app.presence.get(user)
    {
        let dot = (size * 0.30).max(9.0);
        let center = rect.right_bottom() - vec2(dot * 0.42, dot * 0.42);
        ui.painter()
            .circle_filled(center, dot / 2.0 + 1.5, palette.panel);
        ui.painter()
            .circle_filled(center, dot / 2.0, palette.presence(state.availability));
    }
    response
}

/// A borderless icon button with a hover background and tooltip.
pub fn icon_button(ui: &mut Ui, palette: &Palette, icon: Icon, tooltip: &str) -> Response {
    icon_button_sized(ui, palette, icon, tooltip, 30.0, 18.0, palette.secondary)
}

pub fn icon_button_sized(
    ui: &mut Ui,
    palette: &Palette,
    icon: Icon,
    tooltip: &str,
    size: f32,
    icon_size: f32,
    tint: Color32,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() || response.has_focus();
        if hovered {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(6), palette.surface_hover);
        }
        let tint = if hovered { palette.text } else { tint };
        icon.image(tint, icon_size).paint_at(
            ui,
            egui::Rect::from_center_size(rect.center(), Vec2::splat(icon_size)),
        );
    }
    response
        .on_hover_text(tooltip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Adds a button whose fill is `normal`, and `hover` while the pointer is on
/// it or it is pressed. (A `Button::fill` would stay the same in every state.)
pub fn with_fills<R>(
    ui: &mut Ui,
    normal: Color32,
    hover: Color32,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.weak_bg_fill = normal;
        widgets.hovered.weak_bg_fill = hover;
        widgets.active.weak_bg_fill = hover;
        add(ui)
    })
    .inner
}

/// A filled accent button, possibly disabled.
pub fn accent_button(
    ui: &mut Ui,
    palette: &Palette,
    enabled: bool,
    text: &str,
    font: egui::FontId,
    min_height: f32,
) -> Response {
    with_fills(ui, palette.accent, palette.accent_hover, |ui| {
        let button = egui::Button::new(
            egui::RichText::new(text)
                .color(palette.on_accent)
                .font(font),
        )
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(0.0, min_height));
        ui.add_enabled(enabled, button)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    })
}

/// A filled accent button.
pub fn primary_button(ui: &mut Ui, palette: &Palette, text: &str) -> Response {
    accent_button(ui, palette, true, text, theme::semibold(14.0), 34.0)
}

/// An outlined secondary button.
pub fn secondary_button(ui: &mut Ui, palette: &Palette, text: &str) -> Response {
    with_fills(ui, palette.surface, palette.surface_hover, |ui| {
        let button = egui::Button::new(
            egui::RichText::new(text)
                .color(palette.text)
                .font(theme::medium(13.0)),
        )
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(0.0, 32.0));
        ui.add(button)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    })
}

/// A faint wash over content that is already drawn (an image, a card) while
/// the pointer is on `response`.
pub fn hover_wash(ui: &Ui, palette: &Palette, response: &Response, radius: u8) {
    if response.hovered() {
        ui.painter().rect_filled(
            response.rect,
            CornerRadius::same(radius),
            palette.text.gamma_multiply(0.07),
        );
    }
}

/// A borderless text button: dim text that brightens under the pointer.
pub fn text_button(ui: &mut Ui, palette: &Palette, text: &str, font: egui::FontId) -> Response {
    let galley = ui
        .ctx()
        .fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font, palette.secondary));
    let size = vec2(
        galley.size().x,
        galley.size().y.max(ui.spacing().interact_size.y),
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let color = if response.hovered() || response.has_focus() {
            palette.text
        } else {
            palette.secondary
        };
        ui.painter().galley(
            egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A borderless button in the link color, with room around the text for an
/// easy click (the Undo on a toast, which has seconds to be found).
pub fn link_button(ui: &mut Ui, palette: &Palette, text: &str, font: egui::FontId) -> Response {
    let galley = ui
        .ctx()
        .fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font, palette.link));
    let size = vec2(galley.size().x + 16.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let radius = CornerRadius::same(6);
        let hovered = response.hovered() || response.has_focus();
        if hovered {
            ui.painter()
                .rect_filled(rect, radius, palette.surface_hover);
        }
        ui.painter().galley(
            egui::pos2(
                rect.center().x - galley.size().x / 2.0,
                rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            palette.link,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// An outlined toggle chip (list filters such as Unread). Filled with the
/// accent while on.
pub fn chip(ui: &mut Ui, palette: &Palette, text: &str, on: bool) -> Response {
    let fg = if on { palette.on_accent } else { palette.text };
    let galley = ui
        .ctx()
        .fonts_mut(|f| f.layout_no_wrap(text.to_owned(), theme::medium(13.0), fg));
    let size = vec2(galley.size().x + 24.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let radius = CornerRadius::same(14);
        if on {
            let fill = if response.hovered() {
                palette.accent_hover
            } else {
                palette.accent
            };
            ui.painter().rect_filled(rect, radius, fill);
        } else {
            if response.hovered() {
                ui.painter()
                    .rect_filled(rect, radius, palette.surface_hover);
            }
            ui.painter().rect_stroke(
                rect,
                radius,
                egui::Stroke::new(1.0, palette.outline),
                egui::StrokeKind::Inside,
            );
        }
        ui.painter().galley(
            egui::pos2(
                rect.center().x - galley.size().x / 2.0,
                rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            fg,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Width of an `icon_chip`.
pub const ICON_CHIP_WIDTH: f32 = 28.0;

/// A `chip` that shows only an icon, for a filter that needs little room
/// (the label belongs in a hover text).
pub fn icon_chip(ui: &mut Ui, palette: &Palette, icon: Icon, on: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ICON_CHIP_WIDTH, 28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let radius = CornerRadius::same(14);
        let tint = if on {
            let fill = if response.hovered() {
                palette.accent_hover
            } else {
                palette.accent
            };
            ui.painter().rect_filled(rect, radius, fill);
            palette.on_accent
        } else {
            if response.hovered() {
                ui.painter()
                    .rect_filled(rect, radius, palette.surface_hover);
            }
            ui.painter().rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, palette.outline),
                egui::StrokeKind::Inside,
            );
            palette.text
        };
        icon.image(tint, 16.0).paint_at(
            ui,
            egui::Rect::from_center_size(rect.center(), Vec2::splat(16.0)),
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// What a `pill` adds to its text's width: the icon and both insets.
const PILL_PADDING: f32 = 16.0 + 22.0;

/// The width a `pill` showing `text` takes, so a row can leave room for it.
pub fn pill_width(ui: &Ui, text: &str) -> f32 {
    let galley = ui.ctx().fonts_mut(|f| {
        f.layout_no_wrap(text.to_owned(), theme::semibold(13.0), Color32::PLACEHOLDER)
    });
    galley.size().x + PILL_PADDING
}

/// A small pill button with an icon and a label (Join, Call, ...).
pub fn pill(ui: &mut Ui, palette: &Palette, icon: Icon, text: &str, accent: bool) -> Response {
    let (fill, fg) = if accent {
        (palette.accent, palette.on_accent)
    } else {
        (palette.surface_hover, palette.text)
    };
    let galley = ui
        .ctx()
        .fonts_mut(|f| f.layout_no_wrap(text.to_owned(), theme::semibold(13.0), fg));
    let size = vec2(galley.size().x + PILL_PADDING, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if response.hovered() {
            if accent {
                palette.accent_hover
            } else {
                palette.surface_active
            }
        } else {
            fill
        };
        ui.painter().rect_filled(rect, CornerRadius::same(14), fill);
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 17.0, rect.center().y),
            Vec2::splat(15.0),
        );
        icon.image(fg, 15.0).paint_at(ui, icon_rect);
        ui.painter().galley(
            egui::pos2(rect.left() + 29.0, rect.center().y - galley.size().y / 2.0),
            galley,
            fg,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A count badge (unread chats).
pub fn badge(ui: &Ui, palette: &Palette, center: egui::Pos2, count: usize) {
    let text = if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    };
    let galley = ui
        .ctx()
        .fonts_mut(|f| f.layout_no_wrap(text, theme::bold(11.0), palette.on_accent));
    let width = (galley.size().x + 9.0).max(17.0);
    let rect = egui::Rect::from_center_size(center, vec2(width, 17.0));
    ui.painter()
        .rect_filled(rect, CornerRadius::same(9), palette.accent);
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        palette.on_accent,
    );
}

/// One line of text, cut with an ellipsis to the width it has.
pub fn line(ui: &mut Ui, text: &str, font: egui::FontId, color: Color32) -> Response {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(ui.available_width().max(10.0));
    ui.add(egui::Label::new(job).selectable(false))
}

/// A quiet message in an empty pane.
pub fn empty_state(ui: &mut Ui, palette: &Palette, icon: Icon, title: &str, text: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.30).max(24.0));
        icon.image(palette.dim, 40.0).paint_at(
            ui,
            egui::Rect::from_center_size(
                ui.cursor().center_top() + vec2(0.0, 20.0),
                Vec2::splat(40.0),
            ),
        );
        ui.add_space(52.0);
        ui.label(
            egui::RichText::new(title)
                .font(theme::semibold(15.0))
                .color(palette.text),
        );
        ui.add_space(4.0);
        ui.set_max_width(ui.available_width().min(360.0));
        ui.label(egui::RichText::new(text).color(palette.secondary));
    });
}

/// A small uppercase heading for groups in lists.
pub fn section_label(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .font(theme::semibold(12.0))
            .color(palette.dim)
            .extra_letter_spacing(0.6),
    );
}

/// Asks for the photos of avatars drawn this frame without one (collected
/// by [`avatar`]).
pub fn request_photos(app: &mut App, ctx: &egui::Context) {
    let wants: Vec<String> = ctx.data_mut(|data| {
        std::mem::take(
            data.get_temp_mut_or_default::<Vec<String>>(egui::Id::new("chaat-photo-wants")),
        )
    });
    if !wants.is_empty() {
        app.actions.push(Action::LoadPhotos(wants));
    }
}

/// Makes `rect` drag the window (the custom title bar on macOS), and
/// double-clicking it zoom as the system setting says.
pub fn window_drag(ui: &mut Ui, rect: egui::Rect, id: &str) {
    #[cfg(target_os = "macos")]
    ui.push_id(id, |ui| fastframe_titlebar::drag(ui, rect));

    // Chaat keeps the native frame on Windows and Linux. The shared drag
    // helper only handles those platforms when its drawn title bar is on.
    #[cfg(not(target_os = "macos"))]
    {
        let response = ui.interact(rect, ui.id().with(id), Sense::click_and_drag());
        if response.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if response.double_clicked() {
            let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
    }
}

/// egui's checkbox and radio button do not show a pointing hand; these do.
pub trait PointerControls {
    fn checkbox_hand(&mut self, checked: &mut bool, text: impl Into<egui::WidgetText>) -> Response;
    fn radio_hand<V: PartialEq>(
        &mut self,
        current: &mut V,
        value: V,
        text: impl Into<egui::WidgetText>,
    ) -> Response;
}

impl PointerControls for Ui {
    fn checkbox_hand(&mut self, checked: &mut bool, text: impl Into<egui::WidgetText>) -> Response {
        self.checkbox(checked, text)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    fn radio_hand<V: PartialEq>(
        &mut self,
        current: &mut V,
        value: V,
        text: impl Into<egui::WidgetText>,
    ) -> Response {
        self.radio_value(current, value, text)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

/// Pushes `action` when `response` was clicked.
pub fn on_click(response: &Response, actions: &mut Vec<Action>, action: Action) {
    if response.clicked() {
        actions.push(action);
    }
}

/// A single line cut to `width` with an ellipsis.
pub fn elided(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    super::chat_list::elided_styled(ui, text, font, color, width, false)
}

/// A group's heading inside a page: a [`section_label`] with space above.
pub fn group(ui: &mut egui::Ui, palette: &Palette, label: &str) {
    ui.add_space(14.0);
    section_label(ui, palette, label);
}

/// A quiet explanation under a setting.
pub fn note(ui: &mut egui::Ui, palette: &Palette, text: impl Into<String>) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text.into())
                .font(theme::regular(13.0))
                .color(palette.dim),
        )
        .wrap(),
    );
}

/// Makes the scroll bars of this `ui` (and what it holds) thin, rounded and
/// in palette colours: a solid bar about 6 points wide with its margin, a
/// quiet track, and a handle that strengthens on hover and drag. Only the
/// scope it is called in changes.
pub fn thin_scrollbars(ui: &mut Ui, palette: &Palette) {
    let scroll = &mut ui.spacing_mut().scroll;
    scroll.floating = false;
    scroll.bar_width = 4.0;
    scroll.bar_inner_margin = 1.0;
    scroll.bar_outer_margin = 1.0;
    scroll.handle_min_length = 24.0;
    scroll.foreground_color = true;
    let visuals = ui.visuals_mut();
    visuals.extreme_bg_color = palette.outline.gamma_multiply(0.5);
    for (widget, color) in [
        (&mut visuals.widgets.inactive, palette.dim),
        (&mut visuals.widgets.hovered, palette.secondary),
        (&mut visuals.widgets.active, palette.text),
    ] {
        widget.fg_stroke.color = color;
        widget.corner_radius = CornerRadius::same(3);
    }
}

/// The background for marked occurrences of the query.
pub fn highlight_color(palette: &Palette) -> egui::Color32 {
    palette.accent.gamma_multiply(0.4)
}

/// One entry in a menu (a message's More menu, a chat's menu, the people
/// panel): a full width row that lights up under the pointer. Returns whether
/// it was clicked.
pub fn menu_item(ui: &mut Ui, palette: &Palette, icon: Icon, text: &str) -> bool {
    let text_width = ui
        .painter()
        .layout_no_wrap(text.to_owned(), theme::regular(13.0), palette.text)
        .size()
        .x;
    // A popup's first, hidden sizing pass offers egui's default area width
    // (600). Filling it there would make the menu that wide for good.
    let width = if ui.is_sizing_pass() {
        text_width + 44.0
    } else {
        ui.available_width().max(text_width + 44.0)
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() || response.has_focus();
        if hovered {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(6), palette.surface_hover);
        }
        let tint = if hovered {
            palette.text
        } else {
            palette.secondary
        };
        icon.image(tint, 16.0).paint_at(
            ui,
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + 16.0, rect.center().y),
                Vec2::splat(16.0),
            ),
        );
        ui.painter().text(
            egui::pos2(rect.left() + 32.0, rect.center().y),
            Align2::LEFT_CENTER,
            text,
            theme::regular(13.0),
            palette.text,
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// A full-width list row of `height` with the radius-8 wash behind it:
/// `surface_active` when `selected`, `surface_hover` under the pointer. The
/// rect returned is the washed one (the row inset by 8 and 2 points).
pub fn list_row(
    ui: &mut Ui,
    palette: &Palette,
    height: f32,
    selected: bool,
    sense: Sense,
) -> (Response, egui::Rect) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), sense);
    let inner = rect.shrink2(vec2(8.0, 2.0));
    if ui.is_rect_visible(rect) && (selected || response.hovered()) {
        ui.painter().rect_filled(
            inner,
            CornerRadius::same(8),
            if selected {
                palette.surface_active
            } else {
                palette.surface_hover
            },
        );
    }
    (response, inner)
}

/// How a [`modal`] looks and where it sits.
pub struct ModalLook {
    pub margin: i8,
    pub shadow: bool,
    /// Distance from the top of the window; `None` centres the dialog.
    pub top: Option<f32>,
}

/// A dialog over a dimmed backdrop, `width` wide.
pub fn modal(
    palette: Palette,
    ctx: &egui::Context,
    id: &str,
    width: f32,
    look: ModalLook,
    content: impl FnOnce(&mut Ui),
) -> egui::ModalResponse<()> {
    let mut frame = egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(egui::Margin::same(look.margin));
    if look.shadow {
        frame = frame.shadow(egui::Shadow {
            offset: [0, 12],
            blur: 40,
            spread: 0,
            color: palette.shadow,
        });
    }
    let mut modal = egui::Modal::new(egui::Id::new(id))
        .backdrop_color(palette.overlay)
        .frame(frame);
    if let Some(top) = look.top {
        modal = modal.area(
            egui::Modal::default_area(egui::Id::new(id).with("area"))
                .anchor(Align2::CENTER_TOP, vec2(0.0, top)),
        );
    }
    modal.show(ctx, |ui| {
        ui.set_width(width);
        content(ui);
    })
}

/// The label above a form field.
pub fn field_label(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .font(theme::regular(13.0))
            .color(palette.secondary),
    );
}
