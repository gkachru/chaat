//! Drives every screen of the demo through real egui frames, without a
//! window: layout must not panic, actions must apply, and each view must
//! draw something.

use chaat::app::{App, Overlay, PluginsView};
use chaat::backend::{AuthStatus, Waker};
use chaat::model::{Action, AgentAction, ConversationId, PluginAction, Section, SettingsPage};
use chaat::model::{Availability, MentionRef};
use chaat::paths::AppDirs;
use chaat::settings::{Settings, ThemeChoice};
use egui::load::BytesLoader as _;

/// One day in `Message::created` units (milliseconds).
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

struct Harness {
    app: App,
    ctx: egui::Context,
    time: f64,
    size: egui::Vec2,
    shapes: Vec<egui::epaint::ClippedShape>,
    repaint_after: std::time::Duration,
    cursor: egui::CursorIcon,
    #[cfg(not(target_os = "macos"))]
    viewport_commands: Vec<egui::ViewportCommand>,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(
            AppDirs::under(dir.path()),
            Settings::default(),
            Waker::default(),
            true,
        );
        let ctx = egui::Context::default();
        chaat::ui::install(&ctx, true);
        Self {
            app,
            ctx,
            time: 0.0,
            size: egui::vec2(1280.0, 820.0),
            shapes: Vec::new(),
            repaint_after: std::time::Duration::MAX,
            cursor: egui::CursorIcon::Default,
            #[cfg(not(target_os = "macos"))]
            viewport_commands: Vec::new(),
            _dir: dir,
        }
    }

    /// Runs one frame and returns how many shapes it painted.
    fn frame_with(&mut self, events: Vec<egui::Event>) -> usize {
        self.time += 1.0 / 60.0;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut output = self.ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            app.observe_window(&ctx);
            app.background(&ctx);
            chaat::ui::show(app, ui);
            app.apply_actions(&ctx);
        });
        // No renderer here: drop the texture uploads on purpose.
        output.textures_delta.clear();
        let count = output.shapes.len();
        self.cursor = output.platform_output.cursor_icon;
        self.repaint_after = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        #[cfg(not(target_os = "macos"))]
        {
            self.viewport_commands = output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .clone();
        }
        self.shapes = output.shapes;
        count
    }

    fn frames(&mut self, count: usize) -> usize {
        let mut shapes = 0;
        for _ in 0..count {
            shapes = self.frame_with(Vec::new());
        }
        shapes
    }

    fn act(&mut self, action: Action) {
        self.app.actions.push(action);
        self.frames(3);
    }

    fn wait_for_images(&mut self) {
        self.frames(1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while self.ctx.has_pending_images() {
            assert!(
                std::time::Instant::now() < deadline,
                "image loader did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
            self.frames(1);
        }
        self.frames(2);
    }

    fn click(&mut self, pos: egui::Pos2) {
        self.frame_with(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        self.frame_with(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        self.frames(3);
    }
}

#[cfg(not(target_os = "macos"))]
#[test]
fn header_drag_and_double_click_work_with_native_decorations() {
    for width in [720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = themed(&theme, width);
            let header = egui::pos2(220.0, 24.0);
            let press = egui::Event::PointerButton {
                pos: header,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            };
            let release = egui::Event::PointerButton {
                pos: header,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            };

            h.frame_with(vec![egui::Event::PointerMoved(header), press.clone()]);
            h.frame_with(vec![egui::Event::PointerMoved(
                header + egui::vec2(40.0, 0.0),
            )]);
            assert!(
                h.viewport_commands
                    .iter()
                    .any(|command| matches!(command, egui::ViewportCommand::StartDrag))
            );
            h.frame_with(vec![release.clone()]);
            h.frames(30);

            h.click(header);
            h.frame_with(vec![press]);
            h.frame_with(vec![release]);
            assert!(
                h.viewport_commands
                    .iter()
                    .any(|command| matches!(command, egui::ViewportCommand::Maximized(true)))
            );
        }
    }
}

fn painted_shapes(h: &Harness) -> Vec<&egui::epaint::Shape> {
    fn collect<'a>(shape: &'a egui::epaint::Shape, output: &mut Vec<&'a egui::epaint::Shape>) {
        if let egui::epaint::Shape::Vec(shapes) = shape {
            for shape in shapes {
                collect(shape, output);
            }
        } else {
            output.push(shape);
        }
    }
    let mut shapes = Vec::new();
    for clipped in &h.shapes {
        collect(&clipped.shape, &mut shapes);
    }
    shapes
}

#[test]
fn search_fields_keep_their_layout_when_typing_and_clearing() {
    fn text_layout(h: &Harness, value: &str) -> (egui::Pos2, f32) {
        find_text(h, value)
            .first()
            .map(|text| (text.pos, text.galley.size().y))
            .unwrap_or_else(|| panic!("{value:?} is painted"))
    }

    for width in [720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            for switcher in [false, true] {
                let mut h = Harness::new();
                h.size.x = width;
                h.app.settings.theme = theme.clone();
                let me = h.app.me().to_owned();
                h.app.chats.retain(|chat| {
                    chat.peer(&me)
                        .is_some_and(|peer| peer.display_name == "Ada Lovelace")
                });
                if switcher {
                    h.act(Action::OpenSwitcher);
                }
                h.frames(4);
                let hint = if switcher {
                    "Jump to a chat, channel or person, or search messages"
                } else {
                    "Filter chats"
                };
                let empty = text_layout(&h, hint);
                let row = text_layout(&h, "Ada Lovelace");
                h.click(empty.0 + egui::vec2(5.0, 5.0));
                h.frame_with(vec![egui::Event::Text("Ada".into())]);
                h.frames(2);
                assert_eq!(
                    text_layout(&h, "Ada"),
                    empty,
                    "search text must not move, width {width}, switcher {switcher}"
                );
                assert_eq!(
                    text_layout(&h, "Ada Lovelace"),
                    row,
                    "the first result must not move when typing"
                );
                h.frame_with(vec![egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                }]);
                h.frame_with(vec![egui::Event::Key {
                    key: egui::Key::Backspace,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }]);
                h.frames(2);
                assert_eq!(text_layout(&h, hint), empty);
                assert_eq!(text_layout(&h, "Ada Lovelace"), row);
            }
        }
    }
}

#[test]
fn the_relay_access_key_is_masked_and_never_shown_once_saved() {
    let texts = |h: &Harness| -> Vec<String> {
        painted_shapes(h)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect()
    };
    let scrolled = |h: &mut Harness| {
        h.frame_with(vec![
            egui::Event::PointerMoved(egui::pos2(640.0, 400.0)),
            egui::Event::MouseWheel {
                phase: egui::TouchPhase::Move,
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -2000.0),
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        h.frames(30);
    };
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        let mut h = Harness::new();
        h.app.settings.theme = theme;
        h.app.demo = false;
        h.app.form_relay_key = "office-relay-key-0123".into();
        h.act(Action::ShowSettingsPage(SettingsPage::Push));
        scrolled(&mut h);
        let painted = texts(&h);
        assert!(
            !painted.iter().any(|text| text.contains("office-relay-key")),
            "the typed key is masked"
        );
        assert!(
            painted
                .iter()
                .any(|text| text.chars().count() == 21 && text.chars().all(|c| c == '•')),
            "the key field shows one bullet per character"
        );
        assert!(!painted.iter().any(|text| text.contains("Access key saved")));

        h.app.form_relay_key.clear();
        h.app.settings.relay_key_set = true;
        scrolled(&mut h);
        let painted = texts(&h);
        assert!(painted.iter().any(|text| text.contains("Access key saved")));
        assert!(
            !painted
                .iter()
                .any(|text| text == "Access key, if the relay needs one"),
            "no key field while one is saved"
        );
    }
}

#[test]
fn settings_fields_have_padding_in_both_themes_and_widths() {
    let pages = [
        (
            SettingsPage::Push,
            &[
                "https://relay.example.com",
                "Access key, if the relay needs one",
            ][..],
        ),
        (
            SettingsPage::Advanced,
            &["00000000-0000-0000-0000-000000000001", "organizations"][..],
        ),
    ];
    for (page, values) in pages {
        for width in [720.0, 1280.0] {
            for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
                let mut h = Harness::new();
                h.size.x = width;
                h.app.settings.theme = theme;
                // Show account-only settings while keeping the offline demo backend.
                h.app.demo = false;
                h.app.form_client_id = "00000000-0000-0000-0000-000000000001".into();
                h.app.form_tenant = "organizations".into();
                h.act(Action::ShowSettingsPage(page));
                h.frame_with(vec![
                    egui::Event::PointerMoved(egui::pos2(width / 2.0, 400.0)),
                    egui::Event::MouseWheel {
                        phase: egui::TouchPhase::Move,
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -2000.0),
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
                h.frames(30);
                let shapes = painted_shapes(&h);
                let background = if h.app.palette.dark {
                    h.app.palette.window
                } else {
                    h.app.palette.panel
                };
                for &value in values {
                    let text = shapes
                        .iter()
                        .find_map(|shape| match shape {
                            egui::epaint::Shape::Text(text) if text.galley.job.text == value => {
                                Some(text)
                            }
                            _ => None,
                        })
                        .unwrap_or_else(|| panic!("settings field {value:?} is painted"));
                    let text_rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                    let frame = shapes
                        .iter()
                        .filter_map(|shape| match shape {
                            egui::epaint::Shape::Rect(rect)
                                if rect.fill == background
                                    && rect.rect.contains(text_rect.center()) =>
                            {
                                Some(rect.rect)
                            }
                            _ => None,
                        })
                        .min_by(|a, b| a.area().total_cmp(&b.area()))
                        .expect("settings field has a background");
                    assert!(
                        text_rect.left() - frame.left() >= 6.0,
                        "left padding: {frame:?}, {text_rect:?}"
                    );
                    assert!(
                        frame.height() - text_rect.height() >= 10.0,
                        "vertical padding: {frame:?}, {text_rect:?}"
                    );
                }
            }
        }
    }
}

fn painted_texts(h: &Harness) -> Vec<String> {
    painted_shapes(h)
        .into_iter()
        .filter_map(|shape| match shape {
            egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
            _ => None,
        })
        .collect()
}

/// The painted text shapes whose whole text equals `wanted`, in paint order.
fn find_text<'a>(h: &'a Harness, wanted: &str) -> Vec<&'a egui::epaint::TextShape> {
    painted_shapes(h)
        .into_iter()
        .filter_map(|shape| match shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text == wanted => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn every_settings_page_draws_in_both_themes_and_widths() {
    for demo in [true, false] {
        for width in [480.0, 720.0, 1280.0] {
            for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
                let mut h = Harness::new();
                h.size.x = width;
                h.app.settings.theme = theme;
                h.app.demo = demo;
                for page in SettingsPage::ALL {
                    h.act(Action::ShowSettingsPage(page));
                    assert_eq!(h.app.settings_page, Some(page));
                    assert!(h.frames(2) > 20, "{page:?} draws at {width}");
                    let painted = painted_texts(&h);
                    assert!(
                        painted.iter().any(|text| text == page.label()),
                        "{page:?} shows its title at {width}"
                    );
                    // The sidebar names every page, except in the narrow
                    // icon-only list.
                    let named = painted.iter().any(|text| text == "Appearance");
                    assert_eq!(
                        named,
                        width > 600.0 || page == SettingsPage::Appearance,
                        "page names at {width}"
                    );
                    // The demo has no relay to set up.
                    if demo {
                        assert!(
                            !painted.iter().any(|text| text == "Push updates")
                                || page == SettingsPage::Push
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn settings_replace_the_content_and_close_like_a_page() {
    let mut h = Harness::new();
    h.act(Action::OpenSettings);
    assert_eq!(h.app.settings_page, Some(SettingsPage::Account));
    assert_eq!(h.app.overlay, None);
    assert!(
        !painted_texts(&h).iter().any(|text| text == "Chaat"),
        "the chat list's header gives way to settings"
    );

    // ⌘, opens the page shown last; ⌘/ opens the shortcuts page.
    h.act(Action::ShowSettingsPage(SettingsPage::Calls));
    h.act(Action::CloseSettings);
    assert_eq!(h.app.settings_page, None);
    h.act(Action::OpenSettings);
    assert_eq!(h.app.settings_page, Some(SettingsPage::Calls));
    h.act(Action::OpenShortcuts);
    assert_eq!(h.app.settings_page, Some(SettingsPage::Shortcuts));
    let painted = painted_texts(&h);
    for group in ["NAVIGATION", "COMPOSER", "VIEW"] {
        assert!(painted.iter().any(|text| text == group), "{group} group");
    }

    // Esc closes settings.
    h.frame_with(vec![egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    h.frames(2);
    assert_eq!(h.app.settings_page, None);

    // Going to a section, or opening a conversation, leaves settings.
    h.act(Action::OpenSettings);
    h.act(Action::Navigate(Section::Calendar));
    assert_eq!(h.app.settings_page, None);
    assert_eq!(h.app.section, Section::Calendar);
    h.act(Action::OpenSettings);
    h.act(Action::OpenConversation(ConversationId::Chat(
        "19:demo-grace@unq.gbl.spaces".into(),
    )));
    assert_eq!(h.app.settings_page, None);
}

#[test]
fn composer_placeholder_is_vertically_centered_in_both_themes_and_widths() {
    for width in [720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme;
            h.frames(4);
            let shapes = painted_shapes(&h);
            let text = shapes
                .iter()
                .find_map(|shape| match shape {
                    egui::epaint::Shape::Text(text)
                        if text.galley.job.text.starts_with("Message ") =>
                    {
                        Some(text)
                    }
                    _ => None,
                })
                .expect("composer placeholder is painted");
            let text_rect = egui::Rect::from_min_size(text.pos, text.galley.size());
            let frame = shapes
                .iter()
                .filter_map(|shape| match shape {
                    egui::epaint::Shape::Rect(rect)
                        if rect.fill == h.app.palette.surface
                            && rect.rect.contains(text_rect.center()) =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .min_by(|a, b| a.area().total_cmp(&b.area()))
                .expect("composer surface is painted");
            assert!(
                (text_rect.center().y - frame.center().y).abs() < 2.0,
                "placeholder center {:?}, composer {:?}, width {width}",
                text_rect,
                frame
            );
        }
    }
}

#[test]
fn large_pasted_text_stays_scrollable_and_sendable_inside_the_composer() {
    fn editor(h: &Harness) -> (&egui::epaint::TextShape, egui::Rect) {
        h.shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text)
                    if text.galley.job.text.starts_with("SELECT column_0 ") =>
                {
                    Some((text, shape.clip_rect))
                }
                _ => None,
            })
            .expect("pasted text is painted")
    }

    fn assert_caret_visible(h: &Harness, chars: usize) {
        let (text, clip) = editor(h);
        let caret = text
            .galley
            .pos_from_cursor(egui::text::CCursor::new(chars))
            .translate(text.pos.to_vec2());
        assert!(
            clip.expand(1.0).contains_rect(caret),
            "caret {caret:?} stays inside {clip:?}"
        );
    }

    for width in [720.0, 1280.0] {
        for height in [520.0, 820.0] {
            for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
                for separator in ["\n", " "] {
                    let mut h = Harness::new();
                    h.size = egui::vec2(width, height);
                    h.app.settings.theme = theme.clone();
                    h.app.focus_composer = separator == " ";
                    h.frames(4);
                    let chat = h.app.open.clone().unwrap();
                    let pasted = (0..120)
                        .map(|i| format!("SELECT column_{i} FROM example WHERE value IS NOT NULL;"))
                        .collect::<Vec<_>>()
                        .join(separator);
                    h.app.handle(
                        chaat::backend::AccountEvent::Pasted {
                            conversation: chat.clone(),
                            range: (0, 0),
                            text: pasted.clone(),
                            image: Ok(None),
                        },
                        &h.ctx,
                    );
                    h.frames(5);
                    assert_eq!(h.app.drafts[&chat].text, pasted);
                    let (text, clip) = editor(&h);
                    assert_eq!(text.galley.job.text, pasted);
                    assert!(
                        clip.height() <= 201.0,
                        "editor clip must be bounded: {clip:?}"
                    );
                    assert!(
                        clip.top() > height / 2.0,
                        "history keeps space above the composer: {clip:?}"
                    );
                    assert!(
                        clip.bottom() < height,
                        "the composer stays inside the window"
                    );
                    assert_caret_visible(&h, pasted.chars().count());

                    let before_scroll = text.pos.y;
                    h.frame_with(vec![
                        egui::Event::PointerMoved(clip.center()),
                        egui::Event::MouseWheel {
                            phase: egui::TouchPhase::Move,
                            unit: egui::MouseWheelUnit::Point,
                            delta: egui::vec2(0.0, 100.0),
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]);
                    h.frames(25);
                    assert!(
                        editor(&h).0.pos.y > before_scroll + 10.0,
                        "the pasted text scrolls"
                    );

                    h.app.focus_composer = true;
                    h.frames(3);
                    h.frame_with(vec![egui::Event::Text(" End of draft.".into())]);
                    h.frames(4);
                    let expected = format!("{pasted} End of draft.");
                    assert_eq!(h.app.drafts[&chat].text, expected);
                    assert_caret_visible(&h, expected.chars().count());

                    let (_, clip) = editor(&h);
                    let surface = painted_shapes(&h)
                        .into_iter()
                        .filter_map(|shape| match shape {
                            egui::epaint::Shape::Rect(rect)
                                // Dark chat bubbles share the fill; only
                                // the composer holds the whole editor.
                                if rect.fill == h.app.palette.surface
                                    && rect.rect.contains_rect(clip) =>
                            {
                                Some(rect.rect)
                            }
                            _ => None,
                        })
                        .min_by(|a, b| a.area().total_cmp(&b.area()))
                        .expect("composer surface is painted");
                    assert!(
                        clip.right() < surface.right() - 60.0,
                        "buttons have their own space"
                    );
                    let count = h.app.conversations[&chat].messages.len();
                    h.click(egui::pos2(surface.right() - 24.0, surface.center().y));
                    let messages = &h.app.conversations[&chat].messages;
                    assert_eq!(
                        messages.len(),
                        count + 1,
                        "Send stays visible and clickable"
                    );
                    assert_eq!(messages.last().unwrap().body.to_plain(), expected);
                    assert!(
                        h.app
                            .drafts
                            .get(&chat)
                            .is_none_or(|draft| draft.text.is_empty())
                    );
                }
            }
        }
    }
}

#[test]
fn pasted_image_preview_and_image_only_send_draw() {
    let mut h = Harness::new();
    h.frames(3);
    let chat = h.app.open.clone().unwrap();
    let image = chaat::clipboard::encode_rgba(2, 2, &[0, 128, 255, 255].repeat(4)).unwrap();
    h.app.handle(
        chaat::backend::AccountEvent::Pasted {
            conversation: chat.clone(),
            range: (0, 0),
            text: String::new(),
            image: Ok(Some(image)),
        },
        &h.ctx,
    );
    assert!(h.frames(3) > 50);
    let images = h.app.drafts[&chat].images.clone();
    h.act(Action::Send {
        conversation: chat.clone(),
        text: String::new(),
        reply_to: None,
        mentions: Vec::new(),
        files: Vec::new(),
        images,
    });
    assert_eq!(
        h.app.conversations[&chat]
            .messages
            .last()
            .unwrap()
            .body
            .images()
            .count(),
        1
    );
    assert!(h.frames(2) > 50);
}

fn image_shapes<'a>(
    h: &'a Harness,
    image: &chaat::clipboard::InlineImage,
) -> Vec<&'a egui::epaint::RectShape> {
    cached_image_shapes(h, &image.uri())
}

fn cached_image_shapes<'a>(h: &'a Harness, uri: &str) -> Vec<&'a egui::epaint::RectShape> {
    // Inspect what was painted without requesting another thumbnail size.
    // The viewer now has a separate original-resolution texture.
    let textures: std::collections::HashSet<_> = h
        .ctx
        .tex_manager()
        .read()
        .allocated()
        .filter(|(_, meta)| meta.name == uri)
        .map(|(id, _)| *id)
        .collect();
    painted_shapes(h)
        .into_iter()
        .filter_map(|shape| match shape {
            egui::epaint::Shape::Rect(rect) if textures.contains(&rect.fill_texture_id()) => {
                Some(rect)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn clicking_received_images_opens_the_shared_viewer_at_their_original_size() {
    for width in [720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
                let mut h = themed(&theme, width);
                let chat = h.app.open.clone().unwrap();
                let url = "https://graph.microsoft.com/v1.0/chats/fixture/messages/1/hostedContents/1/$value";
                let mut bytes = std::io::Cursor::new(Vec::new());
                image::RgbImage::from_pixel(1200, 800, image::Rgb([0, 128, 255]))
                    .write_to(&mut bytes, format)
                    .unwrap();
                let bytes = bytes.into_inner();
                h.app.handle(
                    chaat::backend::AccountEvent::Media {
                        url: url.into(),
                        bytes: Some(bytes.clone()),
                    },
                    &h.ctx,
                );
                let chaat::app::Image::Ready(uri) = h.app.media[url].clone() else {
                    panic!("fixture image is registered");
                };
                let message = h
                    .app
                    .conversations
                    .get_mut(&chat)
                    .unwrap()
                    .messages
                    .last_mut()
                    .unwrap();
                message.body = chaat::html::parse(&format!(
                    "<img src=\"{url}\" alt=\"Bild\" width=\"120\" height=\"80\">"
                ));
                let count = h.app.conversations[&chat].messages.len();
                h.wait_for_images();
                let thumbnail = cached_image_shapes(&h, &uri)[0].rect;
                assert_eq!(thumbnail.size(), egui::vec2(120.0, 80.0));
                h.click(thumbnail.center());
                h.wait_for_images();
                assert_eq!(h.app.overlay, Some(Overlay::ImagePreview(uri.clone())));
                let zoom = painted_shapes(&h)
                    .into_iter()
                    .find_map(|shape| match shape {
                        egui::epaint::Shape::Text(text) if text.galley.job.text == "100%" => {
                            Some(text.pos + text.galley.size() / 2.0)
                        }
                        _ => None,
                    })
                    .unwrap();
                h.click(zoom);
                assert_eq!(
                    cached_image_shapes(&h, &uri).last().unwrap().rect.size(),
                    egui::vec2(1200.0, 800.0)
                );
                assert!(painted_shapes(&h).iter().any(|shape| matches!(shape, egui::epaint::Shape::Text(text) if text.galley.job.text == "1200 × 800")));
                // At 100% the picture is larger than the window: an open hand
                // offers to drag it, and dragging pans it.
                let before = cached_image_shapes(&h, &uri).last().unwrap().rect;
                let start = h.ctx.content_rect().center();
                h.frame_with(vec![egui::Event::PointerMoved(start)]);
                h.frames(1);
                assert_eq!(h.cursor, egui::CursorIcon::Grab);
                h.frame_with(vec![egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                }]);
                for step in 1..=10 {
                    h.frame_with(vec![egui::Event::PointerMoved(
                        start - egui::vec2(10.0, 5.0) * step as f32,
                    )]);
                }
                assert_eq!(h.cursor, egui::CursorIcon::Grabbing);
                let end = start - egui::vec2(100.0, 50.0);
                h.frame_with(vec![egui::Event::PointerButton {
                    pos: end,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }]);
                h.frames(3);
                let after = cached_image_shapes(&h, &uri).last().unwrap().rect;
                assert!(
                    after.left() < before.left() && after.top() < before.top(),
                    "dragging pans the picture: {before:?} -> {after:?}"
                );
                h.frame_with(vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }]);
                h.frames(3);
                assert!(h.app.overlay.is_none());
                assert_eq!(h.app.conversations[&chat].messages.len(), count);
                let egui::load::BytesPoll::Ready { bytes: cached, .. } =
                    h.ctx.try_load_bytes(&uri).unwrap()
                else {
                    panic!("original bytes remain cached");
                };
                assert_eq!(
                    cached.as_ref(),
                    bytes.as_slice(),
                    "preview reuses the original PNG/JPEG without re-encoding or fetching"
                );
            }
        }
    }
}

#[test]
fn clicking_an_optimistic_image_message_opens_the_same_viewer_as_its_draft() {
    let mut h = Harness::new();
    h.frames(3);
    let chat = h.app.open.clone().unwrap();
    let image =
        chaat::clipboard::encode_rgba(640, 480, &[0, 128, 255, 255].repeat(640 * 480)).unwrap();
    h.app.handle(
        chaat::backend::AccountEvent::Pasted {
            conversation: chat.clone(),
            range: (0, 0),
            text: String::new(),
            image: Ok(Some(image.clone())),
        },
        &h.ctx,
    );
    h.wait_for_images();
    h.act(Action::Send {
        conversation: chat.clone(),
        text: String::new(),
        reply_to: None,
        mentions: Vec::new(),
        files: Vec::new(),
        images: vec![image.clone()],
    });
    h.wait_for_images();
    let thumbnail = image_shapes(&h, &image)[0].rect;
    h.click(thumbnail.center());
    assert_eq!(h.app.overlay, Some(Overlay::ImagePreview(image.uri())));
}

fn image_clips(
    h: &Harness,
    image: &chaat::clipboard::InlineImage,
) -> Vec<(egui::Rect, egui::Rect)> {
    fn collect(
        shape: &egui::epaint::Shape,
        clip: egui::Rect,
        textures: &std::collections::HashSet<egui::TextureId>,
        output: &mut Vec<(egui::Rect, egui::Rect)>,
    ) {
        match shape {
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, clip, textures, output);
                }
            }
            egui::epaint::Shape::Rect(rect) if textures.contains(&rect.fill_texture_id()) => {
                output.push((rect.rect, clip))
            }
            _ => {}
        }
    }
    let textures = image_shapes(h, image)
        .into_iter()
        .map(|s| s.fill_texture_id())
        .collect();
    let mut output = Vec::new();
    for clipped in &h.shapes {
        collect(&clipped.shape, clipped.clip_rect, &textures, &mut output);
    }
    output
}

#[test]
fn composer_images_have_usable_thumbnails_and_open_full_previews() {
    for width in [720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            for (w, height) in [(2000, 20), (20, 2000), (320, 240)] {
                let mut h = themed(&theme, width);
                let chat = h.app.open.clone().unwrap();
                let image = chaat::clipboard::encode_rgba(
                    w,
                    height,
                    &[0, 128, 255, 255].repeat(w * height),
                )
                .unwrap();
                h.app.handle(
                    chaat::backend::AccountEvent::Pasted {
                        conversation: chat.clone(),
                        range: (0, 0),
                        text: String::new(),
                        image: Ok(Some(image.clone())),
                    },
                    &h.ctx,
                );
                h.frames(4);
                h.wait_for_images();
                let thumbnail = image_shapes(&h, &image)[0];
                let rect = thumbnail.rect;
                assert_eq!(rect.size(), egui::vec2(144.0, 96.0));
                let uv = thumbnail.brush.as_ref().unwrap().uv;
                let cropped_aspect = uv.width() * w as f32 / (uv.height() * height as f32);
                assert!(
                    (cropped_aspect - 1.5).abs() < 0.001,
                    "thumbnail preserves proportions"
                );
                assert!(uv.min.x >= 0.0 && uv.min.y >= 0.0 && uv.max.x <= 1.0 && uv.max.y <= 1.0);

                h.click(rect.center());
                h.wait_for_images();
                assert!(
                    matches!(&h.app.overlay, Some(Overlay::ImagePreview(shown)) if shown == &image.uri())
                );
                let fitted = image_shapes(&h, &image).last().unwrap().rect;
                // Painting snaps to pixels, which matters for a very thin
                // fitted image. Allow one pixel of rounding, not stretching.
                assert!(
                    (fitted.width() * height as f32 - fitted.height() * w as f32).abs()
                        <= w.max(height) as f32
                );
                assert!(egui::Rect::from_min_size(egui::Pos2::ZERO, h.size).contains_rect(fitted));
                let (painted, clip) = *image_clips(&h, &image).last().unwrap();
                assert!(
                    clip.expand(1.0).contains_rect(painted),
                    "the fitted preview must be painted inside its scroll viewport"
                );
                let zoom = painted_shapes(&h)
                    .into_iter()
                    .find_map(|shape| match shape {
                        egui::epaint::Shape::Text(text) if text.galley.job.text == "100%" => {
                            Some(text.pos + text.galley.size() / 2.0)
                        }
                        _ => None,
                    })
                    .unwrap();
                h.click(zoom);
                assert_eq!(
                    image_shapes(&h, &image).last().unwrap().rect.size(),
                    egui::vec2(w as f32, height as f32)
                );
                let count = h.app.conversations[&chat].messages.len();
                h.frame_with(vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }]);
                assert_eq!(
                    h.app.conversations[&chat].messages.len(),
                    count,
                    "preview must not send the draft"
                );
                h.frame_with(vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }]);
                h.frames(3);
                assert!(h.app.overlay.is_none());
                assert_eq!(h.app.drafts[&chat].images, vec![image.clone()]);
                let thumbnail = image_shapes(&h, &image)[0].rect;
                let remove = egui::pos2(
                    thumbnail.right() + h.ctx.global_style().spacing.item_spacing.x + 11.0,
                    thumbnail.center().y,
                );
                h.click(remove);
                assert!(
                    h.app
                        .drafts
                        .get(&chat)
                        .is_none_or(|draft| draft.images.is_empty())
                );
                assert!(
                    h.app.overlay.is_none(),
                    "removing an image must not open its preview"
                );
            }
        }
    }
}

#[test]
fn multiple_pasted_images_wrap_without_clipping() {
    for width in [720.0, 1280.0] {
        for theme in [ThemeChoice::Light, ThemeChoice::Dark] {
            let mut h = themed(&theme, width);
            let chat = h.app.open.clone().unwrap();
            for (w, height) in [(1200, 24), (24, 1200), (256, 256)] {
                let image = chaat::clipboard::encode_rgba(
                    w,
                    height,
                    &[0, 128, 255, 255].repeat(w * height),
                )
                .unwrap();
                h.app.handle(
                    chaat::backend::AccountEvent::Pasted {
                        conversation: chat.clone(),
                        range: (0, 0),
                        text: String::new(),
                        image: Ok(Some(image)),
                    },
                    &h.ctx,
                );
            }
            h.frames(4);
            h.wait_for_images();
            let images = &h.app.drafts[&chat].images;
            let mut rects = Vec::new();
            for image in images {
                let (rect, clip) = image_clips(&h, image)[0];
                assert_eq!(rect.size(), egui::vec2(144.0, 96.0));
                assert!(
                    clip.contains_rect(rect),
                    "all thumbnails must remain visible at width {width}"
                );
                assert!(
                    rect.right() + 30.0 <= width,
                    "the remove control must fit too"
                );
                rects.push(rect);
            }
            for (index, rect) in rects.iter().enumerate() {
                assert!(
                    rects[index + 1..]
                        .iter()
                        .all(|next| !rect.intersects(*next))
                );
            }
        }
    }
}

#[test]
fn mention_completion_keeps_space_and_typing_in_the_composer() {
    for width in [720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            for completion in [egui::Key::Tab, egui::Key::Enter] {
                for (query, name) in [("@", "Ada Lovelace"), ("@Gra", "Grace Hopper")] {
                    let mut h = Harness::new();
                    h.size.x = width;
                    h.app.settings.theme = theme.clone();
                    h.act(Action::OpenConversation(ConversationId::Chat(
                        "19:demo-launch@thread.v2".into(),
                    )));
                    let chat = h.app.open.clone().unwrap();
                    h.app.focus_composer = true;
                    h.frames(3);
                    let editor = h.ctx.memory(|m| m.focused()).unwrap();
                    h.frame_with(vec![egui::Event::Text(query.into())]);
                    h.frames(2);
                    let count = h.app.conversations[&chat].messages.len();
                    let key = |key, pressed| egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    };
                    h.frame_with(vec![key(completion, true)]);
                    assert_eq!(h.app.drafts[&chat].text, format!("@{name} "));
                    assert_eq!(h.app.drafts[&chat].mentions[0].name, name);
                    h.frame_with(vec![key(completion, false)]);
                    h.frame_with(vec![
                        key(egui::Key::Space, true),
                        egui::Event::Text(" ".into()),
                    ]);
                    assert_eq!(
                        h.app.conversations[&chat].messages.len(),
                        count,
                        "Space after {completion:?} completion must not send, width {width}"
                    );
                    assert_eq!(h.app.drafts[&chat].text, format!("@{name}  "));
                    assert!(h.ctx.memory(|m| m.has_focus(editor)));
                    h.frame_with(vec![
                        key(egui::Key::Space, false),
                        egui::Event::Text("still typing".into()),
                    ]);
                    assert_eq!(h.app.drafts[&chat].text, format!("@{name}  still typing"));
                    h.frame_with(vec![key(egui::Key::Enter, true)]);
                    assert_eq!(h.app.conversations[&chat].messages.len(), count + 1);
                    assert!(
                        h.app.conversations[&chat]
                            .messages
                            .last()
                            .unwrap()
                            .body
                            .to_plain()
                            .contains("still typing")
                    );
                }
            }
        }
    }
}

#[test]
fn tab_without_mention_suggestions_still_reaches_send() {
    let mut h = Harness::new();
    h.app.focus_composer = true;
    h.frames(3);
    let chat = h.app.open.clone().unwrap();
    let editor = h.ctx.memory(|m| m.focused()).unwrap();
    let count = h.app.conversations[&chat].messages.len();
    h.frame_with(vec![egui::Event::Text("Ready to send".into())]);
    let key = |key, pressed| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    h.frame_with(vec![key(egui::Key::Tab, true)]);
    h.frame_with(vec![key(egui::Key::Tab, false)]);
    assert!(!h.ctx.memory(|m| m.has_focus(editor)));
    h.frame_with(vec![key(egui::Key::Space, true)]);
    assert_eq!(h.app.conversations[&chat].messages.len(), count + 1);
    assert_eq!(
        h.app.conversations[&chat]
            .messages
            .last()
            .unwrap()
            .body
            .to_plain(),
        "Ready to send"
    );
}

#[test]
fn shift_enter_adds_a_line_and_plain_enter_sends_or_saves() {
    for mode in ["chat", "channel", "edit", "mention"] {
        let mut h = Harness::new();
        h.frames(3);
        if mode == "channel" {
            h.act(Action::OpenConversation(ConversationId::Channel {
                team: "t-eng".into(),
                channel: "19:eng-desktop@thread.tacv2".into(),
            }));
        } else if mode == "mention" {
            h.act(Action::OpenConversation(ConversationId::Chat(
                "19:demo-launch@thread.v2".into(),
            )));
        }
        let chat = h.app.open.clone().unwrap();
        let edited = if mode == "edit" {
            let id = h.app.conversations[&chat]
                .messages
                .iter()
                .find(|m| m.is_from(h.app.me()))
                .unwrap()
                .id
                .clone();
            h.act(Action::StartEdit(id.clone()));
            Some(id)
        } else {
            None
        };
        let first = if mode == "mention" {
            "@Gra"
        } else {
            "First line"
        };
        let draft = h.app.drafts.entry(chat.clone()).or_default();
        draft.text = first.into();
        draft.paste_cursor = Some(first.chars().count());
        h.app.focus_composer = true;
        h.frames(4);
        let count = h.app.conversations[&chat].messages.len();
        let enter = |modifiers, pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        h.frame_with(vec![
            egui::Event::ModifiersChanged(egui::Modifiers::SHIFT),
            enter(egui::Modifiers::SHIFT, true),
        ]);
        assert_eq!(h.app.drafts[&chat].text, format!("{first}\n"), "{mode}");
        assert!(
            h.app.drafts[&chat].mentions.is_empty(),
            "Shift+Enter must not pick a mention"
        );
        assert_eq!(h.app.conversations[&chat].messages.len(), count);
        if let Some(id) = &edited {
            assert_eq!(h.app.drafts[&chat].editing.as_ref(), Some(id));
        }
        h.frame_with(vec![
            enter(egui::Modifiers::SHIFT, false),
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            egui::Event::Text("Second line".into()),
        ]);
        h.frame_with(vec![enter(egui::Modifiers::NONE, true)]);
        h.frames(3);
        let messages = &h.app.conversations[&chat].messages;
        let result = if let Some(id) = &edited {
            assert_eq!(messages.len(), count);
            messages.iter().find(|m| &m.id == id).unwrap()
        } else {
            assert_eq!(messages.len(), count + 1, "{mode}");
            messages.last().unwrap()
        };
        assert_eq!(
            result.body.to_plain(),
            format!("{first}\nSecond line"),
            "{mode}"
        );
    }
}

#[test]
fn resizing_keeps_the_visible_row_when_scrolled_up() {
    let mut h = Harness::new();
    let chat = h.app.open.clone().unwrap();
    let now = chaat::time::now();
    h.app.conversations.get_mut(&chat).unwrap().messages = (0..80)
        .map(|i| chaat::model::Message {
            id: format!("row-{i}"),
            author_id: Some(format!("u-{}", i % 3)),
            author: "Someone".into(),
            created: now - (80 - i) * 60 * 60_000,
            body: chaat::html::RichText::plain(&"Long enough to wrap when narrow. ".repeat(8)),
            ..Default::default()
        })
        .collect();
    h.frames(5);
    h.frame_with(vec![
        egui::Event::PointerMoved(egui::pos2(900.0, 300.0)),
        egui::Event::MouseWheel {
            phase: egui::TouchPhase::Move,
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 1500.0),
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    h.frames(25);
    let before = h.app.conversations[&chat].history_anchor.clone().unwrap();
    // Dragging the window edge: a new width every frame.
    for width in (900..1280).rev().step_by(20) {
        h.size.x = width as f32;
        h.frames(1);
    }
    h.frames(5);
    let after = h.app.conversations[&chat].history_anchor.clone().unwrap();
    assert_eq!(before.0, after.0, "the same message stays first in view");
    assert!(
        (before.1 - after.1).abs() < 2.0,
        "at the same place: before {before:?}, after {after:?}"
    );
}

#[test]
fn prepending_history_preserves_the_visible_row() {
    let mut h = Harness::new();
    let chat = h.app.open.clone().unwrap();
    let now = chaat::time::now();
    h.app.conversations.get_mut(&chat).unwrap().messages = (0..80)
        .map(|i| chaat::model::Message {
            id: format!("anchor-{i}"),
            created: now - (80 - i) * 60_000,
            body: chaat::html::RichText::plain(
                "A message with enough text to occupy a complete row.",
            ),
            ..Default::default()
        })
        .collect();
    h.frames(5);
    h.frame_with(vec![
        egui::Event::PointerMoved(egui::pos2(900.0, 300.0)),
        egui::Event::MouseWheel {
            phase: egui::TouchPhase::Move,
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 300.0),
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    h.frames(25);
    let before = h.app.conversations[&chat].history_anchor.clone().unwrap();
    h.app.handle(
        chaat::backend::AccountEvent::Messages {
            conversation: chat.clone(),
            messages: vec![chaat::model::Message {
                id: "prepended".into(),
                created: now - 81 * 60_000,
                body: chaat::html::RichText::plain(
                    "Earlier history\nwith several lines\nto move the existing content.",
                ),
                ..Default::default()
            }],
            merge: chaat::backend::Merge::Older,
            more: Some(true),
            cached: false,
        },
        &h.ctx,
    );
    h.frames(4);
    let after = h.app.conversations[&chat].history_anchor.clone().unwrap();
    assert_eq!(before.0, after.0);
    assert!(
        (before.1 - after.1).abs() < 2.0,
        "before {before:?}, after {after:?}"
    );
}

#[test]
fn kind_chips_filter_the_chat_list_and_stack_with_unread() {
    use chaat::model::ChatKind;

    fn chip(h: &Harness, label: &str) -> egui::Pos2 {
        find_text(h, label)
            .first()
            .map(|text| text.pos + text.galley.size() / 2.0)
            .unwrap_or_else(|| panic!("the {label} chip is painted"))
    }

    for width in [720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = themed(&theme, width);
            h.act(Action::ShowAccount(0));
            let me = h.app.me().to_owned();
            let titles = |kind: ChatKind| -> Vec<String> {
                h.app
                    .chats
                    .iter()
                    .filter(|c| c.kind == kind && !c.hidden)
                    .map(|c| c.title(&me))
                    .collect()
            };
            let (ones, groups, meetings) = (
                titles(ChatKind::OneOnOne),
                titles(ChatKind::Group),
                titles(ChatKind::Meeting),
            );
            assert!(!ones.is_empty() && !groups.is_empty() && !meetings.is_empty());
            // Painted titles show a row is drawn; the open conversation also
            // paints names, so what hides is checked through `listed`.
            let painted = |h: &Harness, title: &String| painted_texts(h).contains(title);
            let kinds = |h: &Harness| -> Vec<ChatKind> {
                let mut kinds: Vec<ChatKind> = h
                    .app
                    .chats
                    .iter()
                    .filter(|c| chaat::app::chat_filter::listed(&h.app, c, &me))
                    .map(|c| c.kind)
                    .collect();
                kinds.sort_by_key(|k| *k as u8);
                kinds.dedup();
                kinds
            };

            let at = chip(&h, "Groups");
            h.click(at);
            assert!(h.app.groups_only, "Groups turns on, width {width}");
            assert!(groups.iter().any(|t| painted(&h, t)));
            assert_eq!(kinds(&h), vec![ChatKind::Group]);

            let at = chip(&h, "Meetings");
            h.click(at);
            assert!(
                h.app.groups_only && h.app.meetings_only,
                "both chips stay on"
            );
            assert!(groups.iter().any(|t| painted(&h, t)));
            assert!(meetings.iter().any(|t| painted(&h, t)));
            assert_eq!(kinds(&h), vec![ChatKind::Group, ChatKind::Meeting]);

            // Unread stacks: only unread chats of the chosen kinds remain.
            let at = chip(&h, "Unread");
            h.click(at);
            for chat in &h.app.chats {
                if chaat::app::chat_filter::listed(&h.app, chat, &me) {
                    assert!(chat.kind != ChatKind::OneOnOne && chat.is_unread(&me));
                }
            }

            // Turning both kind chips off shows every kind again.
            for label in ["Groups", "Meetings", "Unread"] {
                let at = chip(&h, label);
                h.click(at);
            }
            assert!(!h.app.groups_only && !h.app.meetings_only && !h.app.unread_only);
            assert!(ones.iter().any(|t| painted(&h, t)));
            assert_eq!(kinds(&h).len(), 3);
        }
    }
}

#[test]
fn the_rail_shows_icons_without_labels() {
    for width in [720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let h = themed(&theme, width);
            let rail_texts: Vec<String> = painted_shapes(&h)
                .iter()
                .filter_map(|shape| match shape {
                    egui::epaint::Shape::Text(text) if text.pos.x < 64.0 => {
                        Some(text.galley.job.text.clone())
                    }
                    _ => None,
                })
                .filter(|text| {
                    ["Chaat", "Teams", "Calendar", "Calls", "Recordings"].contains(&text.as_str())
                })
                .collect();
            assert!(rail_texts.is_empty(), "rail labels painted: {rail_texts:?}");
        }
    }
}

#[test]
fn message_authors_show_presence_dots_in_both_themes() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        let mut h = Harness::new();
        h.size.x = 1280.0;
        h.app.settings.theme = theme.clone();
        h.act(Action::OpenConversation(ConversationId::Chat(
            "19:demo-launch@thread.v2".into(),
        )));
        h.frames(5);
        // The conversation pane starts to the right of the chat list.
        let pane = h.app.settings.sidebar_width + 80.0;
        let katherine = h
            .app
            .palette
            .presence(h.app.presence["u-katherine"].availability);
        let dots: Vec<egui::Pos2> = painted_shapes(&h)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::epaint::Shape::Circle(circle)
                    if circle.fill == katherine && circle.center.x > pane =>
                {
                    Some(circle.center)
                }
                _ => None,
            })
            .collect();
        assert!(
            !dots.is_empty(),
            "a group chat author's avatar shows a presence dot ({theme:?})"
        );
    }
}

#[test]
fn every_screen_draws() {
    let mut h = Harness::new();
    assert!(h.frames(3) > 50, "the chat view draws");

    // Every chat, including group and meeting chats.
    let chats: Vec<String> = h.app.chats.iter().map(|c| c.id.clone()).collect();
    for chat in chats {
        h.act(Action::OpenConversation(ConversationId::Chat(chat)));
    }
    // Channels.
    let channels: Vec<ConversationId> = h
        .app
        .channels
        .values()
        .flatten()
        .map(|c| ConversationId::Channel {
            team: c.team_id.clone(),
            channel: c.id.clone(),
        })
        .collect();
    for channel in channels {
        h.act(Action::OpenConversation(channel));
    }
    assert_eq!(h.app.section, Section::Teams);
    for section in Section::ALL {
        h.act(Action::Navigate(section));
        assert!(h.frames(2) > 20, "{section:?} draws");
    }
    h.act(Action::OpenSwitcher);
    assert!(h.app.overlay.is_some());
    h.act(Action::CloseOverlay);
    for open in [Action::OpenSettings, Action::OpenShortcuts] {
        h.act(open);
        assert!(h.app.settings_page.is_some());
        h.act(Action::CloseSettings);
    }
    for theme in [
        ThemeChoice::Light,
        ThemeChoice::Dark,
        ThemeChoice::Palette("Nord.json".into()),
    ] {
        h.act(Action::SetTheme(theme));
    }
}

#[test]
fn sending_reacting_and_editing_in_the_demo() {
    let mut h = Harness::new();
    h.frames(2);
    let chat = ConversationId::Chat("19:demo-ada@unq.gbl.spaces".into());
    h.act(Action::OpenConversation(chat.clone()));
    let before = h.app.conversation(&chat).unwrap().messages.len();
    h.act(Action::Send {
        conversation: chat.clone(),
        text: "Shipping **today**".into(),
        reply_to: None,
        mentions: Vec::new(),
        files: Vec::new(),
        images: Vec::new(),
    });
    let conversation = h.app.conversation(&chat).unwrap();
    assert_eq!(conversation.messages.len(), before + 1);
    let sent = conversation.messages.last().unwrap().clone();
    assert_eq!(sent.body.to_plain(), "Shipping today");
    assert_eq!(
        h.app.chats[0].id, "19:demo-ada@unq.gbl.spaces",
        "the chat moves to the top"
    );

    h.act(Action::React {
        conversation: chat.clone(),
        message: sent.id.clone(),
        emoji: "🎉".into(),
        on: true,
    });
    assert_eq!(
        h.app
            .conversation(&chat)
            .unwrap()
            .find(&sent.id)
            .unwrap()
            .reactions
            .len(),
        1
    );

    h.act(Action::StartEdit(sent.id.clone()));
    assert_eq!(
        h.app.drafts[&chat].editing.as_deref(),
        Some(sent.id.as_str())
    );
    h.act(Action::Edit {
        conversation: chat.clone(),
        message: sent.id.clone(),
        text: "Shipping tomorrow".into(),
    });
    let edited = h.app.conversation(&chat).unwrap().find(&sent.id).unwrap();
    assert!(edited.edited);
    assert_eq!(edited.body.to_plain(), "Shipping tomorrow");

    h.act(Action::Delete {
        conversation: chat.clone(),
        message: sent.id.clone(),
    });
    assert!(
        h.app
            .conversation(&chat)
            .unwrap()
            .find(&sent.id)
            .unwrap()
            .deleted
    );
}

#[test]
fn channel_replies_land_in_their_thread() {
    let mut h = Harness::new();
    let channel = ConversationId::Channel {
        team: "t-eng".into(),
        channel: "19:eng-desktop@thread.tacv2".into(),
    };
    h.act(Action::OpenConversation(channel.clone()));
    let root = h.app.conversation(&channel).unwrap().messages[0].clone();
    let replies = root.replies.len();
    h.act(Action::StartReply(root.id.clone()));
    h.act(Action::Send {
        conversation: channel.clone(),
        text: "Agreed".into(),
        reply_to: Some(root.id.clone()),
        mentions: Vec::new(),
        files: Vec::new(),
        images: Vec::new(),
    });
    let root = h
        .app
        .conversation(&channel)
        .unwrap()
        .find(&root.id)
        .unwrap();
    assert_eq!(root.replies.len(), replies + 1);
    assert!(
        h.app
            .conversation(&channel)
            .unwrap()
            .expanded
            .contains(&root.id)
    );
}

#[test]
fn expanding_a_thread_asks_for_its_replies() {
    let mut h = Harness::new();
    let channel = ConversationId::Channel {
        team: "t-eng".into(),
        channel: "19:eng-desktop@thread.tacv2".into(),
    };
    h.act(Action::OpenConversation(channel.clone()));
    let view = h.app.conversations.get_mut(&channel).unwrap();
    let root = &mut view.messages[0];
    let template = root
        .replies
        .first()
        .cloned()
        .unwrap_or_else(|| root.clone());
    root.replies = (0..4)
        .map(|i| {
            let mut reply = template.clone();
            reply.id = format!("reply-{i}");
            reply.replies.clear();
            reply
        })
        .collect();
    let root_id = root.id.clone();
    view.expanded.remove(&root_id);
    view.layout.invalidate();
    h.frames(3);

    let link = painted_texts(&h)
        .into_iter()
        .find(|text| text.starts_with("4 replies from"))
        .expect("the collapsed thread offers its replies");
    h.click(text_center(&h, &link).unwrap());
    h.frames(2);
    let view = h.app.conversation(&channel).unwrap();
    assert!(view.expanded.contains(&root_id), "the thread expands");
    assert!(
        view.replies_asked.contains_key(&root_id),
        "and asks Graph for its replies"
    );
}

#[test]
fn switcher_finds_people_and_messages() {
    let mut h = Harness::new();
    h.act(Action::ShowAccount(0));
    h.act(Action::OpenSwitcher);
    assert_eq!(h.app.overlay, Some(Overlay::Switcher));
    h.act(Action::SearchMessages("relay".into()));
    assert!(
        !h.app.switcher.hits.is_empty(),
        "the demo searches loaded messages"
    );
    // Typing in the switcher and pressing Enter opens the best match.
    h.app.switcher.query = "grace".into();
    h.frame_with(vec![egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    h.frames(2);
    assert_eq!(
        h.app.open,
        Some(ConversationId::Chat("19:demo-grace@unq.gbl.spaces".into()))
    );
    assert_eq!(h.app.overlay, None);
}

/// Chat bubbles in the conversation pane: (own, other people's). Bubbles
/// have no stroke, which tells them from code blocks and reactions.
fn bubbles(h: &Harness) -> (Vec<egui::Rect>, Vec<egui::Rect>) {
    let pane = h.app.settings.sidebar_width + 80.0;
    let (mine, theirs) = (h.app.palette.bubble_mine(), h.app.palette.bubble());
    let mut own = Vec::new();
    let mut others = Vec::new();
    for shape in painted_shapes(h) {
        if let egui::epaint::Shape::Rect(rect) = shape
            && rect.rect.left() > pane
            && rect.stroke.width == 0.0
        {
            if rect.fill == mine {
                own.push(rect.rect);
            } else if rect.fill == theirs {
                others.push(rect.rect);
            }
        }
    }
    (own, others)
}

#[test]
fn chat_messages_are_bubbles_with_your_own_on_the_right() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in [900.0, 1440.0] {
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme.clone();
            h.act(Action::OpenConversation(ConversationId::Chat(
                "19:demo-ada@unq.gbl.spaces".into(),
            )));
            h.frames(5);
            let (own, others) = bubbles(&h);
            assert!(
                !own.is_empty() && !others.is_empty(),
                "both kinds of bubble draw ({theme:?}, {width})"
            );
            let right = own.iter().map(|r| r.right()).fold(f32::MIN, f32::max);
            assert!(
                own.iter().all(|r| (r.right() - right).abs() < 1.0),
                "own bubbles share the right edge ({theme:?}, {width})"
            );
            let left = others.iter().map(|r| r.left()).fold(f32::MAX, f32::min);
            assert!(
                others.iter().all(|r| (r.left() - left).abs() < 1.0),
                "other people's bubbles share the left edge ({theme:?}, {width})"
            );
            assert!(
                others.iter().all(|r| r.right() < right - 20.0),
                "other people's bubbles stay left of your own ({theme:?}, {width})"
            );
            assert!(right <= width, "bubbles stay inside the window");
            h.frames(40);
            assert!(
                h.repaint_after > std::time::Duration::from_millis(100),
                "measuring own bubbles settles: {:?}",
                h.ctx.repaint_causes()
            );
        }
    }
}

#[test]
fn the_hover_bar_keeps_edit_delete_and_copy_in_a_menu() {
    let mut h = Harness::new();
    h.act(Action::OpenConversation(ConversationId::Chat(
        "19:demo-ada@unq.gbl.spaces".into(),
    )));
    h.frames(5);
    let (own, _) = bubbles(&h);
    let bubble = *own.last().expect("an own bubble is on screen");
    // Beside the bubble, in the same row: no bar. 😡 shows only in the bar.
    h.frame_with(vec![egui::Event::PointerMoved(
        bubble.left_center() - egui::vec2(60.0, 0.0),
    )]);
    h.frames(2);
    assert!(
        !painted_texts(&h).iter().any(|t| t == "😡"),
        "only the bubble shows the bar"
    );
    h.frame_with(vec![egui::Event::PointerMoved(bubble.center())]);
    h.frames(2);
    assert!(
        !painted_texts(&h).iter().any(|t| t == "😡"),
        "the bar waits for the pointer to rest"
    );
    h.frames(25); // past the hover delay
    let texts = painted_texts(&h);
    assert!(texts.iter().any(|t| t == "😡"), "quick reactions show");
    assert!(!texts.iter().any(|t| t == "Edit"), "Edit waits in the menu");
    // The bar sits on the bubble's top edge from its left end (wider than
    // the bar here), its More button last.
    let bar = 6.0 * 32.0 + 28.0 + 9.0 + 2.0 * 28.0 + 6.0;
    assert!(bubble.width() > bar);
    h.click(egui::pos2(bubble.left() + bar - 17.0, bubble.top() - 16.0));
    let texts = painted_texts(&h);
    for item in ["Edit", "Delete", "Copy text"] {
        assert!(texts.iter().any(|t| t == item), "the menu offers {item}");
    }
    let chat = h.app.open.clone().unwrap();
    let menu = egui::Id::new(("chaat-message-menu", chat.key()));
    let rect = h
        .ctx
        .memory(|m| m.area_rect(menu))
        .expect("the menu is an area");
    assert!(rect.width() < 240.0, "the menu fits its items: {rect:?}");
}

#[test]
fn own_bubbles_stay_inside_a_window_narrowed_in_another_chat() {
    let ada = ConversationId::Chat("19:demo-ada@unq.gbl.spaces".into());
    let grace = ConversationId::Chat("19:demo-grace@unq.gbl.spaces".into());
    // Seen before at the wide size, or opened for the first time after
    // the window narrowed; resized at once or dragged.
    for (seen, dragged) in [(true, false), (true, true), (false, false), (false, true)] {
        let mut h = Harness::new();
        h.size.x = 1440.0;
        if seen {
            h.act(Action::OpenConversation(ada.clone()));
            h.frames(5);
        }
        h.act(Action::OpenConversation(grace.clone()));
        h.frames(5);
        if dragged {
            for width in (1000..=1440).rev().step_by(20) {
                h.size.x = width as f32;
                h.frames(1);
            }
        }
        h.size.x = 1000.0;
        h.frames(5);
        h.act(Action::OpenConversation(ada.clone()));
        h.frames(5);
        let (own, _) = bubbles(&h);
        assert!(!own.is_empty());
        for bubble in own {
            assert!(
                bubble.right() <= 1000.0 - 16.0,
                "own bubble ends inside the window (seen {seen}, dragged {dragged}): {bubble:?}"
            );
        }
    }
}

#[test]
fn a_long_file_name_does_not_widen_the_history() {
    let ada = ConversationId::Chat("19:demo-ada@unq.gbl.spaces".into());
    let mut h = Harness::new();
    h.size.x = 1000.0;
    h.act(Action::OpenConversation(ada.clone()));
    h.frames(3);
    let me = h.app.me().to_owned();
    let view = h.app.conversations.get_mut(&ada).unwrap();
    let n = view.messages.len();
    let at = view.messages[n - 2].created - 1;
    let file = |id: &str, at: i64| chaat::model::Message {
        id: id.into(),
        author_id: Some(me.clone()),
        author: "Me".into(),
        created: at,
        body: chaat::html::RichText::plain("The recording"),
        attachments: vec![chaat::model::Attachment {
            id: format!("{id}-file"),
            kind: chaat::model::AttachmentKind::File,
            name: Some(format!("{}.mp4", "a-very-long-recording-name-".repeat(12))),
            content_type: None,
            url: Some("https://example.com/file".into()),
            content: None,
        }],
        ..Default::default()
    };
    // Ordinary messages follow the file.
    view.messages.insert(n - 2, file("file-1", at));
    view.layout.invalidate();
    h.act(Action::OpenConversation(ada));
    h.frames(10);
    let (own, _) = bubbles(&h);
    assert!(own.len() > 1);
    for bubble in &own {
        assert!(
            bubble.right() <= 1000.0 - 16.0,
            "own bubbles stay inside the window: {bubble:?}"
        );
    }
}

#[test]
fn pointing_at_a_quick_reaction_dims_the_others() {
    let mut h = Harness::new();
    h.act(Action::OpenConversation(ConversationId::Chat(
        "19:demo-ada@unq.gbl.spaces".into(),
    )));
    h.frames(5);
    let (own, _) = bubbles(&h);
    let bubble = *own.last().expect("an own bubble is on screen");
    h.frame_with(vec![egui::Event::PointerMoved(bubble.center())]);
    h.frames(25); // past the hover delay
    // The bar starts at your bubble's left edge; its second button is the heart.
    let bar_left = bubble.left();
    let heart = egui::pos2(bar_left + 3.0 + 32.0 + 16.0, bubble.top() - 16.0);
    h.frame_with(vec![egui::Event::PointerMoved(heart)]);
    h.frames(2);
    let color_of = |emoji: &str| {
        painted_shapes(&h)
            .into_iter()
            .find_map(|shape| match shape {
                egui::epaint::Shape::Text(text)
                    if text.galley.job.text == emoji
                        && (bubble.top() - 38.0..bubble.top()).contains(&text.pos.y) =>
                {
                    text.galley.job.sections.first().map(|s| s.format.color)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{emoji} is in the bar"))
    };
    let text = h.app.palette.text;
    assert_eq!(color_of("❤️"), text, "the pointed reaction stays bright");
    assert_eq!(color_of("👍"), text.gamma_multiply(0.35), "the others dim");
}

/// The sizes (in points) of the sections of every painted text galley that
/// contains `needle`, as (section text, size) pairs.
fn section_sizes(h: &Harness, needle: &str) -> Vec<(String, f32)> {
    let mut sizes = Vec::new();
    for shape in painted_shapes(h) {
        if let egui::epaint::Shape::Text(text) = shape
            && text.galley.job.text.contains(needle)
        {
            for section in &text.galley.job.sections {
                sizes.push((
                    text.galley.job.text[section.byte_range.start.0..section.byte_range.end.0]
                        .to_owned(),
                    section.format.font_id.size,
                ));
            }
        }
    }
    sizes
}

#[test]
fn emoji_draw_one_step_larger_than_the_text_around_them() {
    let ada = ConversationId::Chat("19:demo-ada@unq.gbl.spaces".into());
    let mut h = Harness::new();
    h.act(Action::OpenConversation(ada.clone()));
    // A reaction on the newest message, whose bubble is in view.
    h.app
        .conversations
        .get_mut(&ada)
        .unwrap()
        .messages
        .last_mut()
        .unwrap()
        .reactions
        .push(chaat::model::Reaction {
            emoji: "👍".into(),
            users: vec!["u-ada".into()],
        });
    h.frames(5);
    // Reaction pills are 18 points.
    let pill = section_sizes(&h, "👍");
    assert!(
        pill.iter().any(|(t, s)| t == "👍" && *s == 18.0),
        "reaction pills draw at 18: {pill:?}"
    );
    // The pill sits snug around the emoji: as tall as the 18-point line,
    // not the old 30-point minimum.
    let shapes = painted_shapes(&h);
    let emoji = shapes
        .iter()
        .filter_map(|shape| match shape {
            egui::epaint::Shape::Text(text)
                if text.galley.job.text.starts_with("👍")
                    && text.galley.job.sections[0].format.font_id.size == 18.0 =>
            {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
        .max_by(|a, b| a.y.total_cmp(&b.y))
        .expect("a 👍 pill");
    let frame = shapes
        .iter()
        .filter_map(|shape| match shape {
            egui::epaint::Shape::Rect(rect) if rect.rect.contains(emoji) => Some(rect.rect),
            _ => None,
        })
        .min_by(|a, b| a.area().total_cmp(&b.area()))
        .expect("the pill frame");
    assert!(frame.height() <= 28.0, "pill height {}", frame.height());
    // It tucks 1 point into the bottom edge of the bubble above it.
    let bubble = shapes
        .iter()
        .filter_map(|shape| match shape {
            // Bubbles are rounded; the header panel above them is not.
            egui::epaint::Shape::Rect(rect)
                if rect.corner_radius != egui::CornerRadius::ZERO
                    && rect.rect.bottom() > frame.top()
                    && rect.rect.top() < frame.top() - 10.0
                    && rect.rect.x_range().intersects(frame.x_range()) =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .min_by(|a, b| a.area().total_cmp(&b.area()))
        .expect("the bubble above the pill");
    assert!(
        (bubble.bottom() - frame.top() - 1.0).abs() < 0.6,
        "pill top {} vs bubble bottom {}",
        frame.top(),
        bubble.bottom()
    );
    let me = h.app.me().to_owned();
    let view = h.app.conversations.get_mut(&ada).unwrap();
    let last = view.messages.last().expect("a message").created;
    view.messages.push(chaat::model::Message {
        id: "emoji-1".into(),
        author_id: Some(me),
        author: "Me".into(),
        created: last + 1,
        body: chaat::html::RichText::plain("Ship it 🚀"),
        ..Default::default()
    });
    view.layout.invalidate();
    h.act(Action::OpenConversation(ada.clone()));
    h.frames(10);
    let sizes = section_sizes(&h, "Ship it");
    assert!(
        sizes.iter().any(|(t, s)| t == "Ship it " && *s == 14.0),
        "text stays at 14: {sizes:?}"
    );
    assert!(
        sizes.iter().any(|(t, s)| t == "🚀" && *s == 15.0),
        "emoji draw at 15: {sizes:?}"
    );

    // The hover bar: 18 points.
    let (own, _) = bubbles(&h);
    let bubble = *own.last().expect("an own bubble is on screen");
    h.frame_with(vec![egui::Event::PointerMoved(bubble.center())]);
    h.frames(25);
    let bar: Vec<f32> = painted_shapes(&h)
        .into_iter()
        .filter_map(|shape| match shape {
            egui::epaint::Shape::Text(text)
                if text.galley.job.text == "😡"
                    && (bubble.top() - 38.0..bubble.top()).contains(&text.pos.y) =>
            {
                Some(text.galley.job.sections[0].format.font_id.size)
            }
            _ => None,
        })
        .collect();
    assert_eq!(bar, [18.0], "quick reactions in the hover bar draw at 18");

    // The Add a reaction popup: 20 points.
    let message = h
        .app
        .conversations
        .get(&ada)
        .unwrap()
        .messages
        .iter()
        .find(|m| !m.reactions.is_empty())
        .expect("a reacted message")
        .id
        .clone();
    egui::Popup::open_id(&h.ctx, egui::Id::new(("chaat-add-reaction", &message)));
    h.frames(3);
    let popup: Vec<f32> = painted_shapes(&h)
        .into_iter()
        .filter_map(|shape| match shape {
            egui::epaint::Shape::Text(text)
                if text.galley.job.text == "😡"
                    && text.galley.job.sections[0].format.font_id.size != 18.0 =>
            {
                Some(text.galley.job.sections[0].format.font_id.size)
            }
            _ => None,
        })
        .collect();
    assert_eq!(popup, [20.0], "the Add a reaction popup draws at 20");
}

#[test]
fn messages_of_only_emoji_draw_large_inside_their_bubble() {
    let ada = ConversationId::Chat("19:demo-ada@unq.gbl.spaces".into());
    for width in [720.0, 1280.0] {
        let mut h = Harness::new();
        h.size.x = width;
        h.act(Action::OpenConversation(ada.clone()));
        h.frames(3);
        let me = h.app.me().to_owned();
        let view = h.app.conversations.get_mut(&ada).unwrap();
        let last = view.messages.last().expect("a message").created;
        view.messages.push(chaat::model::Message {
            id: "jumbo-1".into(),
            author_id: Some(me),
            author: "Me".into(),
            created: last + 1,
            body: chaat::html::RichText::plain("🎉🎉"),
            ..Default::default()
        });
        view.layout.invalidate();
        h.act(Action::OpenConversation(ada.clone()));
        h.frames(10);
        let sizes = section_sizes(&h, "🎉🎉");
        assert!(
            sizes.iter().any(|(t, s)| t == "🎉🎉" && *s == 30.0),
            "emoji-only messages draw at 30 ({width}): {sizes:?}"
        );
        let (own, _) = bubbles(&h);
        let text = painted_shapes(&h)
            .into_iter()
            .find_map(|shape| match shape {
                egui::epaint::Shape::Text(text) if text.galley.job.text == "🎉🎉" => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                _ => None,
            })
            .expect("the message is on screen");
        assert!(
            own.iter().any(|bubble| bubble.contains_rect(text)),
            "the emoji fit inside their bubble ({width}): {text:?} in {own:?}"
        );
    }
}

#[test]
fn single_reactions_show_only_the_emoji() {
    let ada = ConversationId::Chat("19:demo-ada@unq.gbl.spaces".into());
    let mut h = Harness::new();
    // The demo dates its messages from the clock: after midnight Ada's
    // last message falls on a new day, and the extra day divider scrolled
    // the first reaction out of an 820-point window. Span three days at any
    // hour, in a window tall enough for the whole history.
    let view = h.app.conversations.get_mut(&ada).unwrap();
    let last = view.messages.len() - 1;
    for message in &mut view.messages[..last] {
        message.created -= DAY_MS;
    }
    h.size.y = 2000.0;
    h.act(Action::OpenConversation(ada));
    h.frames(5);
    let texts = painted_texts(&h);
    assert!(texts.iter().any(|t| t == "👍"), "one reaction: emoji only");
    assert!(!texts.iter().any(|t| t == "👍1"));
    assert!(
        texts.iter().any(|t| t == "🎉2"),
        "two reactions: with count"
    );
}

#[test]
fn unread_chats_open_at_a_last_read_divider() {
    let mut h = Harness::new();
    h.frames(2);
    let katherine = ConversationId::Chat("19:demo-katherine@unq.gbl.spaces".into());
    h.act(Action::OpenConversation(katherine.clone()));
    h.frames(3);
    assert!(
        painted_texts(&h).iter().any(|t| t == "Last read"),
        "an unread chat shows where reading stopped"
    );
    h.act(Action::OpenConversation(ConversationId::Chat(
        "19:demo-ada@unq.gbl.spaces".into(),
    )));
    h.frames(3);
    assert!(!painted_texts(&h).iter().any(|t| t == "Last read"));
    h.act(Action::OpenConversation(katherine));
    h.frames(3);
    assert!(
        !painted_texts(&h).iter().any(|t| t == "Last read"),
        "reopening a chat that is now read shows no divider"
    );
}

#[test]
fn hovering_messages_shows_their_actions() {
    let mut h = Harness::new();
    h.frames(3);
    // Sweep the pointer over the conversation; the hover toolbar must lay
    // out wherever it lands.
    for y in (120..760).step_by(40) {
        h.frame_with(vec![egui::Event::PointerMoved(egui::pos2(900.0, y as f32))]);
    }
}

#[test]
fn the_welcome_screen_draws_without_an_account() {
    let mut h = Harness::new();
    h.app.demo = false;
    h.app.chats.clear();
    for status in [
        AuthStatus::NotConfigured,
        AuthStatus::SignedOut,
        AuthStatus::WaitingForBrowser {
            url: "https://login.microsoftonline.com/x".into(),
        },
        AuthStatus::DeviceCode {
            code: "ABCD-EFGH".into(),
            url: "https://microsoft.com/devicelogin".into(),
        },
        AuthStatus::Expired {
            username: "ada@contoso.com".into(),
            reason: "Sign in again.".into(),
        },
        AuthStatus::Failed("Microsoft refused the sign-in.".into()),
    ] {
        h.app.auth = status;
        assert!(!h.app.shows_workspace());
        assert!(h.frames(2) > 10);
    }
}

#[test]
fn large_conversations_stay_cheap() {
    let mut h = Harness::new();
    let chat = ConversationId::Chat("19:demo-ada@unq.gbl.spaces".into());
    let template = h.app.conversation(&chat).unwrap().messages[0].clone();
    let messages: Vec<_> = (0..5_000)
        .map(|i| {
            let mut m = template.clone();
            m.id = format!("bulk-{i}");
            m.created = template.created - (5_000 - i) * 60_000;
            m.author_id = Some(if i % 3 == 0 { "u-me" } else { "u-ada" }.into());
            m
        })
        .collect();
    h.app.conversations.get_mut(&chat).unwrap().messages = messages;
    h.act(Action::OpenConversation(chat));
    // The first frames estimate and measure; later frames reuse heights.
    h.frames(4);
    let started = std::time::Instant::now();
    h.frames(10);
    let per_frame = started.elapsed() / 10;
    // Generous bound for debug builds on CI: virtualization keeps this far
    // below laying out 5,000 rows.
    assert!(
        per_frame < std::time::Duration::from_millis(250),
        "{per_frame:?} per frame"
    );
}

#[test]
fn accounts_switch_and_keep_their_own_chats() {
    let mut h = Harness::new();
    h.frames(2);
    assert_eq!(h.app.accounts.len(), 2, "the demo has a second account");
    let first = h.app.chats[0].id.clone();
    h.act(Action::SwitchAccount(1));
    assert_eq!(h.app.active, 1);
    assert!(h.app.chats.iter().all(|c| c.id.starts_with("19:fabrikam")));
    assert!(h.frames(2) > 50, "the second account draws");
    h.act(Action::SwitchAccount(0));
    assert_eq!(h.app.chats[0].id, first);
    // The other account's unread chat shows as a dot on the avatar.
    assert!(h.app.other_unread() >= 1);
}

#[test]
fn starting_a_new_chat_from_the_directory() {
    let mut h = Harness::new();
    h.act(Action::OpenNewChat);
    assert_eq!(h.app.overlay, Some(Overlay::NewChat));
    h.app.new_chat.query = "barb".into();
    h.act(Action::SearchPeople("barb".into()));
    let barbara = h
        .app
        .people
        .results
        .iter()
        .find(|p| p.display_name == "Barbara Liskov")
        .cloned()
        .expect("found in the directory");
    h.frames(2);
    h.act(Action::CreateChat {
        members: vec![barbara],
        topic: None,
    });
    assert_eq!(h.app.overlay, None);
    let open = h.app.open.clone().expect("the new chat opens");
    assert_eq!(h.app.title_of(&open), "Barbara Liskov");
    h.act(Action::Send {
        conversation: open.clone(),
        text: "Hi Barbara".into(),
        reply_to: None,
        mentions: Vec::new(),
        files: Vec::new(),
        images: Vec::new(),
    });
    assert_eq!(h.app.conversation(&open).unwrap().messages.len(), 1);
    // Starting a one-on-one with someone again opens the same chat.
    let chats = h.app.chats.len();
    let ada = h
        .app
        .directory()
        .into_iter()
        .find(|p| p.display_name == "Ada Lovelace")
        .unwrap();
    h.act(Action::CreateChat {
        members: vec![ada],
        topic: None,
    });
    assert_eq!(h.app.chats.len(), chats);
    assert_eq!(
        h.app.open,
        Some(ConversationId::Chat("19:demo-ada@unq.gbl.spaces".into()))
    );
}

#[test]
fn mentions_and_files_in_the_demo() {
    let mut h = Harness::new();
    let chat = ConversationId::Chat("19:demo-launch@thread.v2".into());
    h.act(Action::OpenConversation(chat.clone()));
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("checklist.pdf");
    std::fs::write(&file, b"%PDF-1.7").unwrap();
    h.act(Action::AddFiles(vec![
        file.clone(),
        dir.path().to_path_buf(),
    ]));
    assert_eq!(
        h.app.drafts[&chat].files,
        vec![file.clone()],
        "folders are refused"
    );
    assert!(h.frames(2) > 50, "the composer draws its file chips");
    h.act(Action::Send {
        conversation: chat.clone(),
        text: "@Grace Hopper here it is".into(),
        reply_to: None,
        mentions: vec![MentionRef {
            user_id: "u-grace".into(),
            name: "Grace Hopper".into(),
        }],
        files: vec![file],
        images: Vec::new(),
    });
    let sent = h
        .app
        .conversation(&chat)
        .unwrap()
        .messages
        .last()
        .unwrap()
        .clone();
    assert_eq!(sent.attachments[0].name.as_deref(), Some("checklist.pdf"));
    let first = &sent.body.blocks[0];
    let chaat::html::Block::Paragraph(spans) = first else {
        panic!("paragraph expected")
    };
    assert!(spans[0].style.mention && spans[0].text == "Grace Hopper");
    assert!(!h.app.drafts.contains_key(&chat));
}

#[test]
fn setting_status_and_message() {
    let mut h = Harness::new();
    h.act(Action::SetAvailability(Some(Availability::DoNotDisturb)));
    assert_eq!(
        h.app.my_presence.as_ref().unwrap().availability,
        Availability::DoNotDisturb
    );
    h.act(Action::OpenStatusMessage);
    assert_eq!(h.app.overlay, Some(Overlay::StatusMessage));
    h.frames(2);
    h.act(Action::SetStatusMessage("Back at 3 pm".into()));
    assert_eq!(
        h.app
            .my_presence
            .as_ref()
            .unwrap()
            .status_message
            .as_deref(),
        Some("Back at 3 pm")
    );
    let me = h.app.me().to_owned();
    assert_eq!(h.app.presence[&me].availability, Availability::DoNotDisturb);
    h.act(Action::SetAvailability(None));
    assert_eq!(
        h.app.my_presence.as_ref().unwrap().availability,
        Availability::Available
    );
}

#[test]
fn out_of_office_from_the_account_menu_and_settings() {
    use chaat::model::{AutoReplies, ExternalAudience};
    for width in [480.0, 720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme;
            h.app.open_account_menu = true;
            h.frames(3);
            assert!(
                painted_texts(&h).iter().any(|t| t == "Out of office…"),
                "the account menu offers Out of Office at {width}"
            );

            h.act(Action::OpenOutOfOffice);
            assert_eq!(h.app.settings_page, Some(SettingsPage::OutOfOffice));
            assert_eq!(h.app.overlay, None, "the demo holds every scope");
            let painted = painted_texts(&h);
            assert!(painted.iter().any(|t| t == "Automatic replies are off."));
            assert!(!painted.iter().any(|t| t == "Starts"));

            // A scheduled window with a reply outside the organization.
            let form = &mut h.app.form_out_of_office;
            form.replies = AutoReplies::Scheduled;
            form.internal = "Away until Monday.".into();
            form.audience = ExternalAudience::Contacts;
            h.frames(3);
            let painted = painted_texts(&h);
            for text in ["Starts", "Ends", "Save", "Discard changes"] {
                assert!(
                    painted.iter().any(|t| t.trim() == text),
                    "{text} at {width}"
                );
            }
            let value = h.app.form_out_of_office.value().unwrap();
            h.act(Action::SetOutOfOffice(value));
            let painted = painted_texts(&h);
            assert!(
                painted
                    .iter()
                    .any(|t| t.starts_with("Automatic replies are scheduled from")),
                "the saved window shows at {width}"
            );
            assert!(!painted.iter().any(|t| t == "Discard changes"));

            // An end before the start can't be saved.
            h.app.form_out_of_office.end_date = "2000-01-01".into();
            h.frames(3);
            assert!(
                painted_texts(&h)
                    .iter()
                    .any(|t| t == "The end must be after the start.")
            );

            // Accounts without the permission, or without a mailbox.
            h.app.demo = false;
            h.app.granted = "User.Read".into();
            h.frames(3);
            assert!(painted_texts(&h).iter().any(|t| t.trim() == "Allow access"));
            h.app.granted = "User.Read MailboxSettings.ReadWrite".into();
            h.app.no_mailbox = true;
            h.frames(3);
            assert!(
                painted_texts(&h)
                    .iter()
                    .any(|t| t.contains("no Exchange Online mailbox"))
            );
        }
    }
}

#[test]
fn new_dialogs_draw() {
    let mut h = Harness::new();
    h.app.overlay = Some(Overlay::Consent(chaat::auth::FILES_SCOPE));
    assert!(h.frames(2) > 50);
    h.app.overlay = Some(Overlay::NewChat);
    h.app.new_chat.query = "a".into();
    assert!(h.frames(2) > 50);
    h.app.overlay = Some(Overlay::StatusMessage);
    assert!(h.frames(2) > 50);
}

#[test]
fn joining_a_meeting_follows_it_beside_its_chat() {
    let mut h = Harness::new();
    h.frames(2);
    let url = h
        .app
        .meetings
        .iter()
        .find(|m| m.id == "demo-design-review")
        .and_then(|m| m.join_url.clone())
        .unwrap();
    h.app.section = Section::Calendar;
    h.act(Action::JoinMeeting(url.clone()));
    let call = h.app.call.clone().expect("a call session");
    assert_eq!(call.title(), "Design review: Chaat 0.2");
    let chat = ConversationId::Chat("19:meeting_demo-design-review@thread.v2".into());
    assert_eq!(call.chat.as_ref(), Some(&chat), "the meeting chat is found");
    assert_eq!(h.app.open.as_ref(), Some(&chat));
    assert_eq!(h.app.section, Section::Chats);

    // Joining again brings the call forward instead of joining twice.
    h.act(Action::JoinMeeting(url));
    assert_eq!(h.app.call.as_ref().unwrap().opened_at, call.opened_at);

    // The bar along the bottom, then docked as a narrow panel.
    assert!(h.frames(2) > 50);
    h.app.pretend_docked();
    assert!(h.frames(2) > 30, "the docked panel draws");

    // The QR code to join on a phone.
    h.act(Action::JoinOnPhone);
    assert!(matches!(h.app.overlay, Some(Overlay::JoinOnPhone(_))));
    assert!(h.frames(2) > 200, "the QR code draws its modules");
    h.act(Action::CloseOverlay);

    h.act(Action::CallDone);
    assert!(h.app.call.is_none());
    assert!(h.app.docked.is_none(), "Done restores the window");
    assert!(h.frames(2) > 50);
}

#[test]
fn call_settings_draw_and_apply() {
    let mut h = Harness::new();
    h.act(Action::ShowSettingsPage(SettingsPage::Calls));
    assert!(h.frames(2) > 50);
    let mut calls = h.app.settings.calls.clone();
    assert_eq!(calls.open_in, chaat::calling::CallsOpenIn::AppWindow);
    assert!(calls.dock);
    calls.open_in = chaat::calling::CallsOpenIn::Browser;
    h.act(Action::SetCallSettings(calls));
    assert_eq!(
        h.app.settings.calls.open_in,
        chaat::calling::CallsOpenIn::Browser
    );
    assert_eq!(h.app.opener(), chaat::calling::Opener::Browser);
    // A call from the Calls page, with the bar along the bottom.
    h.act(Action::CloseSettings);
    h.app.section = Section::Calls;
    h.act(Action::StartCall {
        users: vec!["ada@contoso.com".into()],
        video: false,
    });
    assert!(h.app.call.is_some());
    assert!(h.frames(2) > 50);
}

#[test]
fn a_loading_conversation_settles_without_animation_repaints() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        let mut h = Harness::new();
        h.app.settings.theme = theme;
        h.app.loading.insert(chaat::sync::Resource::Chats);
        let open = h.app.open.clone().unwrap();
        h.app.loading.insert(chaat::sync::Resource::Messages(open));
        h.frames(40);
        assert!(
            h.repaint_after > std::time::Duration::from_millis(100),
            "waiting for Graph must not animate the whole window: {:?}",
            h.repaint_after
        );
        assert!(
            !h.ctx
                .repaint_causes()
                .iter()
                .any(|cause| format!("{cause:?}").contains("spinner.rs"))
        );
    }
}

#[test]
fn an_evicted_draft_image_reopens_at_original_resolution_without_losing_the_draft() {
    let mut h = Harness::new();
    h.frames(3);
    let chat = h.app.open.clone().unwrap();
    let image =
        chaat::clipboard::encode_rgba(1200, 800, &[15, 50, 90, 255].repeat(1200 * 800)).unwrap();
    h.app.handle(
        chaat::backend::AccountEvent::Pasted {
            conversation: chat.clone(),
            range: (0, 0),
            text: String::new(),
            image: Ok(Some(image.clone())),
        },
        &h.ctx,
    );
    h.wait_for_images();
    // Pasted images must not also enter egui's unbounded included-byte map.
    assert!(
        h.ctx.loaders().include.load(&h.ctx, &image.uri()).is_err(),
        "draft bypassed the bounded source cache"
    );
    h.ctx.forget_image(&image.uri());
    assert!(!chaat::images::contains(&h.ctx, &image.uri()));
    h.act(Action::PreviewImage(image.uri()));
    h.wait_for_images();
    assert!(chaat::images::contains(&h.ctx, &image.uri()));
    assert_eq!(h.app.drafts[&chat].images, vec![image.clone()]);
    assert!(
        h.ctx
            .tex_manager()
            .read()
            .allocated()
            .any(|(_, meta)| meta.name == image.uri() && meta.size == [1200, 800])
    );
    h.act(Action::CloseOverlay);
    h.frames(3);
    assert!(
        !h.ctx
            .tex_manager()
            .read()
            .allocated()
            .any(|(_, meta)| meta.name == image.uri() && meta.size == [1200, 800])
    );
    assert_eq!(h.app.drafts[&chat].images.len(), 1);
}

#[test]
fn settings_pages_fit_the_smallest_window() {
    // main.rs: with_min_inner_size([720.0, 480.0]).
    for demo in [true, false] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size = egui::vec2(720.0, 480.0);
            h.app.settings.theme = theme;
            h.app.demo = demo;
            h.app.settings.client_id = "00000000-0000-0000-0000-000000000001".into();
            h.app.form_client_id = h.app.settings.client_id.clone();
            for page in SettingsPage::ALL {
                h.act(Action::ShowSettingsPage(page));
                let mut clipped = Vec::new();
                for shape in &h.shapes {
                    let clip = shape.clip_rect;
                    let mut flat = Vec::new();
                    fn collect<'a>(
                        shape: &'a egui::epaint::Shape,
                        output: &mut Vec<&'a egui::epaint::Shape>,
                    ) {
                        if let egui::epaint::Shape::Vec(shapes) = shape {
                            for shape in shapes {
                                collect(shape, output);
                            }
                        } else {
                            output.push(shape);
                        }
                    }
                    collect(&shape.shape, &mut flat);
                    for part in flat {
                        let bounds = part.visual_bounding_rect();
                        // Backgrounds span the clip; content starts inside it.
                        if bounds.is_finite()
                            && bounds.left() > clip.left()
                            && bounds.left() < clip.right()
                            && bounds.right() > clip.right() + 1.0
                        {
                            let what = match part {
                                egui::epaint::Shape::Text(text) => text.galley.job.text.clone(),
                                other => format!("{:?}", std::mem::discriminant(other)),
                            };
                            clipped.push(format!("{what:?} to {:.0}", bounds.right()));
                        }
                    }
                }
                assert!(
                    clipped.is_empty(),
                    "{page:?} (demo {demo}) clips at 720 × 480: {clipped:#?}"
                );
            }
        }
    }
}

/// The right edge of the widest text painted right of the chat list.
fn widest_text(h: &Harness) -> f32 {
    let pane = h.app.settings.sidebar_width + 80.0;
    painted_shapes(h)
        .into_iter()
        .filter_map(|shape| match shape {
            egui::epaint::Shape::Text(text) if text.pos.x > pane => {
                Some(text.pos.x + text.galley.size().x)
            }
            _ => None,
        })
        .fold(0.0, f32::max)
}

#[test]
fn formatting_help_shows_only_while_typing() {
    let mut h = Harness::new();
    h.act(Action::OpenConversation(ConversationId::Chat(
        "19:demo-ada@unq.gbl.spaces".into(),
    )));
    h.frames(3);
    let help = |h: &Harness| {
        painted_shapes(h)
            .into_iter()
            .find_map(|shape| match shape {
                egui::epaint::Shape::Text(text) if text.galley.job.text.starts_with("**bold**") => {
                    Some((text.galley.job.sections[0].format.color, text.pos.y))
                }
                _ => None,
            })
            .expect("the help line keeps its place")
    };
    let (focused, at) = help(&h);
    assert_ne!(focused, egui::Color32::TRANSPARENT, "shown while typing");
    // Clicking the history takes focus from the editor.
    h.click(egui::pos2(h.app.settings.sidebar_width + 200.0, 200.0));
    let (unfocused, moved) = help(&h);
    assert_eq!(unfocused, egui::Color32::TRANSPARENT, "hidden otherwise");
    assert_eq!(at, moved, "the composer doesn't change height");
}

#[test]
fn calls_page_explains_where_calls_run_and_links_to_settings() {
    // At 380 the sidebar leaves the page no room; 760 is still narrow.
    for width in [760.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme.clone();
            h.act(Action::Navigate(Section::Calls));
            h.frames(5);
            let texts = painted_texts(&h);
            assert!(
                texts.iter().any(|t| t == "How calls work"),
                "{width} {theme:?}"
            );
            assert!(
                !texts.iter().any(|t| t == "What works today"),
                "the table is gone"
            );
            assert!(
                widest_text(&h) <= width,
                "fits at {width}: {}",
                widest_text(&h)
            );
            let button =
                ring_text_pos(&h, "Choose where calls open").expect("the button is painted");
            h.click(button);
            assert_eq!(
                h.app.settings_page,
                Some(SettingsPage::Calls),
                "{width} {theme:?}"
            );
        }
    }
}

#[test]
fn calls_and_calendar_pages_fit_a_narrow_window() {
    let width = 1000.0;
    let mut h = Harness::new();
    h.size.x = width;
    h.act(Action::Navigate(Section::Calls));
    h.frames(5);
    assert!(
        widest_text(&h) <= width,
        "the calls page fits: {}",
        widest_text(&h),
    );

    h.act(Action::Navigate(Section::Calendar));
    let meeting = &mut h.app.meetings[0];
    meeting.preview = Some(format!(
        "Join: https://teams.microsoft.com/meet/{}?p=kY7X9YXPLoHl0F1",
        "4596999827403".repeat(6)
    ));
    let id = meeting.id.clone();
    h.app.selected_meeting = Some(id);
    h.frames(5);
    assert!(
        widest_text(&h) <= width,
        "a meeting with a long link fits: {}",
        widest_text(&h)
    );
}

#[test]
fn membership_lines_name_people_graph_left_unnamed() {
    let ada = ConversationId::Chat("19:demo-ada@unq.gbl.spaces".into());
    let mut h = Harness::new();
    h.act(Action::OpenConversation(ada.clone()));
    let view = h.app.conversations.get_mut(&ada).unwrap();
    let at = view.messages.last().unwrap().created + 1_000;
    view.messages.push(chaat::model::Message {
        id: "added".into(),
        system: true,
        created: at,
        body: chaat::html::RichText::plain("Someone added someone to the chat."),
        membership: Some(chaat::model::Membership {
            change: chaat::model::MemberChange::Added,
            by: Some(chaat::model::EventPerson {
                id: "u-ada".into(),
                name: None,
            }),
            members: vec![chaat::model::EventPerson {
                id: "u-nobody".into(),
                name: None,
            }],
        }),
        ..Default::default()
    });
    view.layout.invalidate();
    h.frames(5);
    let texts = painted_texts(&h);
    assert!(
        texts
            .iter()
            .any(|t| t == "Ada Lovelace added someone to the chat."),
        "a chat member is named; someone unknown waits: {texts:?}"
    );
    assert!(
        !h.app.needs_name("u-nobody"),
        "the unknown name was asked for"
    );
}

#[test]
fn a_missed_call_offers_call_back_in_both_themes_and_widths() {
    for width in [720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let grace = ConversationId::Chat("19:demo-grace@unq.gbl.spaces".into());
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme.clone();
            // Older than yesterday, the call is dated by weekday and month
            // (as after midnight, when the demo's yesterday becomes two
            // days ago): the longest line Call back must still fit beside.
            let view = h.app.conversations.get_mut(&grace).unwrap();
            for message in &mut view.messages {
                message.created -= 4 * DAY_MS;
            }
            h.frames(2);
            h.act(Action::OpenConversation(grace));
            h.frames(2);
            let texts = painted_texts(&h);
            assert!(
                texts.iter().any(|t| t.ends_with("Missed call")),
                "the missed call line is painted"
            );
            let button = painted_shapes(&h)
                .iter()
                .find_map(|shape| match shape {
                    egui::epaint::Shape::Text(text) if text.galley.job.text == "Call back" => {
                        Some(text.pos + text.galley.size() / 2.0)
                    }
                    _ => None,
                })
                .expect("Call back is painted");
            assert!(
                button.x > 0.0 && button.x < width,
                "Call back fits at {width}"
            );
            assert!(h.app.call.is_none());
            h.click(button);
            assert!(h.app.call.is_some(), "Call back starts a call");
        }
    }
}

// -- Meeting recordings -------------------------------------------------------

const DESIGN_REVIEW: &str = "19:meeting_demo-design-review@thread.v2";
const DESIGN_REVIEW_URL: &str =
    "https://contoso-my.sharepoint.com/personal/demo/Recordings/design-review.mp4";

/// One frame without applying actions (a click on a link would open the
/// browser), returning what the views pushed.
fn captured_frame(h: &mut Harness, events: Vec<egui::Event>) -> Vec<Action> {
    h.time += 1.0 / 60.0;
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, h.size)),
        time: Some(h.time),
        events,
        ..Default::default()
    };
    let app = &mut h.app;
    let mut output = h.ctx.run_ui(input, |ui| {
        let ctx = ui.ctx().clone();
        app.observe_window(&ctx);
        app.background(&ctx);
        chaat::ui::show(app, ui);
    });
    output.textures_delta.clear();
    h.shapes = output.shapes;
    std::mem::take(&mut h.app.actions)
}

/// Clicks at `pos` and returns the actions the views pushed meanwhile.
fn captured_click(h: &mut Harness, pos: egui::Pos2) -> Vec<Action> {
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let mut actions = captured_frame(h, vec![egui::Event::PointerMoved(pos)]);
    actions.extend(captured_frame(h, vec![button(true)]));
    actions.extend(captured_frame(h, vec![button(false)]));
    actions.extend(captured_frame(h, vec![]));
    actions
}

fn open_design_review(width: f32, theme: &ThemeChoice) -> Harness {
    let mut h = Harness::new();
    h.size.x = width;
    h.app.settings.theme = theme.clone();
    h.frames(2);
    h.act(Action::OpenConversation(ConversationId::Chat(
        DESIGN_REVIEW.into(),
    )));
    h.frames(2);
    h
}

fn open_urls(actions: &[Action]) -> usize {
    actions
        .iter()
        .filter(|a| matches!(a, Action::OpenUrl(_)))
        .count()
}

#[test]
fn recording_card_draws_in_both_themes_and_widths() {
    for width in [480.0, 720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = open_design_review(width, &theme);
            let texts = painted_texts(&h);
            assert!(
                texts.iter().any(|t| t == "Design review"),
                "the card's title is painted at {width} in {theme:?}"
            );
            assert!(
                texts.iter().any(|t| t.ends_with(" · 58m")),
                "the card's time and length are painted at {width} in {theme:?}"
            );
            let open = find_text(&h, "Open")
                .first()
                .map(|t| t.pos.x + t.galley.size().x)
                .expect("Open is painted");
            assert!(open <= width, "Open ends at {open}, inside {width}");
            // At 480 the window is narrower than the sidebar and the chat
            // together, so the pane runs past the edge: only wider windows
            // take the clicks.
            if width < 720.0 {
                continue;
            }
            let title = ring_text_pos(&h, "Design review").unwrap();
            let actions = captured_click(&mut h, title);
            assert_eq!(open_urls(&actions), 1, "one action for the card body");
            assert!(
                actions.contains(&Action::OpenUrl(DESIGN_REVIEW_URL.into())),
                "the card opens the recording at {width} in {theme:?}"
            );
            let pos = ring_text_pos(&h, "Open").unwrap();
            let actions = captured_click(&mut h, pos);
            assert_eq!(open_urls(&actions), 1, "one action for Open");
            assert!(
                actions.contains(&Action::OpenUrl(DESIGN_REVIEW_URL.into())),
                "Open pushes OpenUrl at {width} in {theme:?}"
            );
        }
    }
}

#[test]
fn a_link_less_recording_event_is_hidden_when_a_link_follows() {
    let count = |h: &Harness| {
        painted_texts(h)
            .iter()
            .filter(|t| t.as_str() == "Call recorded")
            .count()
    };
    let h = open_design_review(1280.0, &ThemeChoice::Dark);
    let hidden = count(&h);
    let view = &h.app.conversations[&ConversationId::Chat(DESIGN_REVIEW.into())];
    assert!(
        view.messages
            .iter()
            .any(|m| m.recording_event && m.recording.is_none()),
        "the demo has a link-less event"
    );

    // Without the link, the same events show.
    let mut h = Harness::new();
    h.frames(2);
    let id = ConversationId::Chat(DESIGN_REVIEW.into());
    let view = h.app.conversations.get_mut(&id).unwrap();
    for message in &mut view.messages {
        message.recording = None;
    }
    view.layout.invalidate();
    h.act(Action::OpenConversation(id));
    h.frames(2);
    assert!(
        count(&h) > hidden,
        "a chat with only link-less events still shows Call recorded"
    );
}

#[test]
fn meeting_header_lists_recordings() {
    // The menu is open from the start, so its rows show only if the header
    // drew the Recordings button.
    let menu = |chat: &str| egui::Id::new(("recordings-menu", chat.to_owned()));
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme.clone();
            h.frames(2);
            h.act(Action::OpenConversation(ConversationId::Chat(
                DESIGN_REVIEW.into(),
            )));
            egui::Popup::open_id(&h.ctx, menu(DESIGN_REVIEW));
            h.frames(3);
            let row = painted_texts(&h)
                .into_iter()
                .find(|t| t.starts_with("Design review · ") && t.ends_with(" · 58m"))
                .unwrap_or_else(|| panic!("the recordings row shows at {width} in {theme:?}"));
            let pos = ring_text_pos(&h, &row).unwrap();
            let actions = captured_click(&mut h, pos);
            assert!(actions.contains(&Action::OpenUrl(DESIGN_REVIEW_URL.into())));

            // A chat without a recording has no such button.
            let grace = "19:demo-grace@unq.gbl.spaces";
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme.clone();
            h.frames(2);
            h.act(Action::OpenConversation(ConversationId::Chat(grace.into())));
            egui::Popup::open_id(&h.ctx, menu(grace));
            h.frames(3);
            assert!(
                !painted_texts(&h)
                    .iter()
                    .any(|t| t.starts_with("Design review · ")),
                "no recordings menu in a chat without one"
            );
        }
    }
}

fn open_recordings(width: f32, theme: &ThemeChoice) -> Harness {
    let mut h = Harness::new();
    h.size.x = width;
    h.app.settings.theme = theme.clone();
    h.frames(2);
    h.act(Action::Navigate(Section::Recordings));
    h.frames(2);
    h
}

#[test]
fn recordings_section_draws_in_both_themes_and_widths() {
    for width in [480.0, 720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let h = open_recordings(width, &theme);
            assert_eq!(h.app.recordings.len(), 1, "the demo fills the list");
            let texts = painted_texts(&h);
            assert!(
                texts.iter().any(|t| t == "Design review"),
                "the recording row shows at {width} in {theme:?}"
            );
            assert!(
                texts
                    .iter()
                    .any(|t| t.starts_with("Design review: Chaat 0.2 · ") && t.ends_with(" · 58m")),
                "the row's chat, time and length show at {width} in {theme:?}"
            );
        }
    }
}

#[test]
fn recordings_section_selects_and_shows_the_card() {
    let chat = ConversationId::Chat(DESIGN_REVIEW.into());
    for width in [480.0, 720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = open_recordings(width, &theme);
            assert_eq!(
                h.app.selected_recording.as_deref(),
                Some(DESIGN_REVIEW_URL),
                "the newest recording is selected at {width} in {theme:?}"
            );
            let titles = find_text(&h, "Design review").len();
            assert_eq!(titles, 2, "the row and the card paint the title");
            assert!(
                !find_text(&h, "Open recording").is_empty(),
                "the card paints Open recording at {width} in {theme:?}"
            );
            assert!(
                !find_text(&h, "in Design review: Chaat 0.2").is_empty(),
                "the card names the meeting chat at {width} in {theme:?}"
            );
            // At 480 the window is narrower than the rail and sidebar
            // together, so the main pane overlaps the list: only wider
            // windows take clicks.
            if width < 720.0 {
                continue;
            }
            // The demo's call-alerts sign-in card floats over the pane.
            dismiss_sign_in_card(&mut h);
            let pos = ring_text_pos(&h, "Design review").unwrap();
            let actions = captured_click(&mut h, pos);
            assert_eq!(open_urls(&actions), 0, "a row only selects");
            assert_eq!(h.app.selected_recording.as_deref(), Some(DESIGN_REVIEW_URL));

            let pos = ring_text_pos(&h, "Open recording").unwrap();
            let actions = captured_click(&mut h, pos);
            assert_eq!(open_urls(&actions), 1, "one action for Open recording");
            assert!(
                actions.contains(&Action::OpenUrl(DESIGN_REVIEW_URL.into())),
                "Open recording pushes OpenUrl at {width} in {theme:?}"
            );
            let pos = ring_text_pos(&h, "Copy link").unwrap();
            let actions = captured_click(&mut h, pos);
            assert!(
                actions.contains(&Action::CopyText(DESIGN_REVIEW_URL.into())),
                "Copy link pushes CopyText at {width} in {theme:?}"
            );
            let pos = ring_text_pos(&h, "Open meeting chat").unwrap();
            let actions = captured_click(&mut h, pos);
            assert!(
                actions.contains(&Action::OpenConversation(chat.clone())),
                "Open meeting chat pushes OpenConversation at {width} in {theme:?}"
            );
        }
    }
}

#[test]
fn recording_rows_have_no_chat_button() {
    let h = open_recordings(1280.0, &ThemeChoice::Dark);
    assert!(
        find_text(&h, "Chat").is_empty(),
        "no Chat pill is painted in the list"
    );
}

// -- People panel -------------------------------------------------------------

const LAUNCH: &str = "19:demo-launch@thread.v2";

fn launch_ids(h: &Harness) -> Vec<String> {
    h.app
        .group_members(LAUNCH)
        .into_iter()
        .map(|m| m.user_id)
        .collect()
}

fn open_launch(h: &mut Harness) {
    h.act(Action::OpenConversation(ConversationId::Chat(
        LAUNCH.into(),
    )));
    h.frames(5);
}

/// The bounds of the rightmost painted text equal to `text` in the header
/// row (the sidebar can paint the same text lower down).
fn header_text(h: &Harness, text: &str) -> Option<egui::Rect> {
    painted_shapes(h)
        .into_iter()
        .filter_map(|shape| match shape {
            egui::epaint::Shape::Text(t) if t.galley.job.text == text => {
                let rect = shape.visual_bounding_rect();
                (rect.center().y < chaat::theme::HEADER_HEIGHT).then_some(rect)
            }
            _ => None,
        })
        .max_by(|a, b| a.center().x.total_cmp(&b.center().x))
}

fn open_people_panel(h: &mut Harness) {
    open_launch(h);
    let button = header_text(h, "5").expect("the people button shows the count");
    h.click(button.center());
}

fn press(h: &mut Harness, key: egui::Key) {
    h.frame_with(vec![egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    h.frames(3);
}

#[test]
fn people_button_opens_panel() {
    let mut h = Harness::new();
    open_people_panel(&mut h);
    let texts = painted_texts(&h);
    for expected in ["People (5)", "You", "Ada Lovelace", "Add people", "Leave"] {
        assert!(
            texts.iter().any(|t| t == expected),
            "the panel shows {expected}"
        );
    }
}

#[test]
fn people_panel_draws_in_both_themes_and_widths() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in [720.0, 1280.0] {
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme.clone();
            open_launch(&mut h);
            // The button fits beside the title without overlapping it.
            let title = header_text(&h, "Launch crew").expect("the title shows");
            let button = header_text(&h, "5").expect("the people button shows");
            assert!(
                title.right() <= button.left(),
                "title and people button overlap ({theme:?}, {width})"
            );
            h.click(button.center());
            h.frames(3);
            assert!(
                painted_texts(&h).iter().any(|t| t == "People (5)"),
                "the panel draws ({theme:?}, {width})"
            );
        }
    }
}

#[test]
fn people_panel_shows_for_meeting_chats() {
    const MEETING: &str = "19:meeting_demo-design-review@thread.v2";
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in [720.0, 1280.0] {
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme.clone();
            h.act(Action::OpenConversation(ConversationId::Chat(
                MEETING.into(),
            )));
            h.frames(5);
            let title = header_text(&h, "Design review: Chaat 0.2").expect("the title shows");
            let join = header_text(&h, "Join").expect("the Join pill stays");
            // Narrow headers show the people button as an icon only: it
            // sits right of the Join pill (pill padding 9 + spacing 4).
            let button = header_text(&h, "4").unwrap_or_else(|| {
                egui::Rect::from_min_size(
                    join.right_top() + egui::vec2(13.0, 0.0),
                    egui::vec2(30.0, 9.0),
                )
            });
            assert!(
                title.right() <= join.left() && join.right() <= button.left(),
                "title, people button and Join overlap ({theme:?}, {width}): {title:?} {join:?} {button:?}"
            );
            h.click(button.center());
            let texts = painted_texts(&h);
            for expected in ["People (4)", "You", "Ada Lovelace", "Add people", "Leave"] {
                assert!(
                    texts.iter().any(|t| t == expected),
                    "the panel shows {expected} ({theme:?}, {width})"
                );
            }
        }
    }
}

#[test]
fn people_panel_never_for_one_on_one() {
    let mut h = Harness::new();
    h.act(Action::OpenConversation(ConversationId::Chat(
        "19:demo-ada@unq.gbl.spaces".into(),
    )));
    h.frames(5);
    assert!(
        header_text(&h, "2").is_none(),
        "no people button in a 1:1 chat"
    );
}

#[test]
fn add_people_flow() {
    let mut h = Harness::new();
    open_launch(&mut h);
    h.act(Action::OpenAddPeople(LAUNCH.into()));
    h.app.add_people.query = "al".into();
    h.frames(3);
    let alan = h
        .app
        .directory()
        .into_iter()
        .find(|m| m.user_id == "u-alan")
        .expect("Alan is a known person");
    assert!(
        painted_texts(&h).contains(&alan.display_name),
        "the picker offers Alan"
    );
    h.app.add_people.picked.push(alan.clone());
    h.app.add_people.history_step = true;
    h.frames(3);
    assert!(
        painted_texts(&h)
            .iter()
            .any(|t| t == "Include all chat history")
    );
    assert!(
        painted_texts(&h)
            .iter()
            .any(|t| t == "Don't include chat history")
    );
    h.act(Action::AddMembers {
        chat: LAUNCH.into(),
        members: vec![alan],
        history: true,
    });
    assert_eq!(launch_ids(&h).len(), 6);
    assert!(launch_ids(&h).contains(&"u-alan".to_owned()));
    assert_eq!(h.app.overlay, None);
}

#[test]
fn add_people_excludes_members() {
    let mut h = Harness::new();
    open_launch(&mut h);
    h.act(Action::OpenAddPeople(LAUNCH.into()));
    let count = |h: &Harness, text: &str| painted_texts(h).iter().filter(|t| *t == text).count();
    let before = count(&h, "Ada Lovelace");
    h.app.add_people.query = "ada".into();
    h.frames(3);
    assert_eq!(
        count(&h, "Ada Lovelace"),
        before,
        "an existing member is not offered"
    );
    assert!(
        !painted_texts(&h).iter().any(|t| t.starts_with("ada@")),
        "no result row for an existing member"
    );
    // The picker does offer people who are not members.
    h.app.add_people.query = "alan".into();
    h.frames(3);
    assert!(painted_texts(&h).iter().any(|t| t.contains("alan@")));
}

#[test]
fn leave_confirm_and_leave() {
    let mut h = Harness::new();
    open_launch(&mut h);
    h.act(Action::OpenLeaveChat(LAUNCH.into()));
    assert!(painted_texts(&h).iter().any(|t| t == "Leave the chat?"));
    h.act(Action::LeaveChat(LAUNCH.into()));
    assert!(h.app.chats.iter().all(|c| c.id != LAUNCH));
    assert_eq!(h.app.open, None);
    assert_eq!(h.app.overlay, None);
}

#[test]
fn leave_without_scope_shows_admin_dialog() {
    let mut h = Harness::new();
    h.app.demo = false;
    h.app.granted = "User.Read Chat.ReadWrite".into();
    h.act(Action::OpenLeaveChat(LAUNCH.into()));
    let texts = painted_texts(&h);
    assert!(texts.iter().any(|t| t == "Leaving needs admin approval"));
    assert!(texts.iter().any(|t| t == "Open in Teams"));
    assert!(texts.iter().any(|t| t.trim() == "Request access"));
}

#[test]
fn escape_closes_dialogs() {
    let mut h = Harness::new();
    open_launch(&mut h);
    for action in [
        Action::OpenAddPeople(LAUNCH.into()),
        Action::OpenLeaveChat(LAUNCH.into()),
    ] {
        h.act(action);
        assert!(h.app.overlay.is_some());
        press(&mut h, egui::Key::Escape);
        assert_eq!(h.app.overlay, None, "Escape closes the dialog");
    }
}

#[test]
fn escape_closes_the_people_panel() {
    let mut h = Harness::new();
    open_people_panel(&mut h);
    assert!(painted_texts(&h).iter().any(|t| t == "People (5)"));
    press(&mut h, egui::Key::Escape);
    assert!(!painted_texts(&h).iter().any(|t| t == "People (5)"));
}

// -- Hover feedback -----------------------------------------------------------

/// Whether `a` is `b`, allowing for a popup's fade (it scales every channel).
fn same_fill(a: egui::Color32, b: egui::Color32) -> bool {
    let scale = f32::from(a.a()) / f32::from(b.a().max(1));
    a.a() > 128
        && [(a.r(), b.r()), (a.g(), b.g()), (a.b(), b.b())]
            .into_iter()
            .all(|(x, y)| (f32::from(x) - f32::from(y) * scale).abs() <= 3.0)
}

/// Whether a small rect filled with `color` is painted under `point`.
fn filled_under(h: &Harness, point: egui::Pos2, color: egui::Color32) -> bool {
    painted_shapes(h).into_iter().any(|shape| match shape {
        egui::epaint::Shape::Rect(rect) => {
            same_fill(rect.fill, color)
                && rect.rect.contains(point)
                && rect.rect.width() < 400.0
                && rect.rect.height() < 60.0
        }
        _ => false,
    })
}

#[test]
fn people_panel_rows_light_up_on_hover() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        let mut h = Harness::new();
        h.app.settings.theme = theme.clone();
        open_people_panel(&mut h);
        for label in ["Add people", "Leave"] {
            let row = find_text(&h, label)
                .first()
                .map(|t| t.visual_bounding_rect())
                .unwrap_or_else(|| panic!("{label} is in the panel"));
            let fill = h.app.palette.surface_hover;
            // Resting: no highlight, the default arrow.
            h.frame_with(vec![egui::Event::PointerMoved(egui::pos2(2.0, 2.0))]);
            h.frames(2);
            assert!(
                !filled_under(&h, row.center(), fill),
                "{label} rests ({theme:?})"
            );
            h.frame_with(vec![egui::Event::PointerMoved(row.center())]);
            h.frames(2);
            assert!(
                filled_under(&h, row.center(), fill),
                "{label} lights up under the pointer ({theme:?})"
            );
            assert_eq!(
                h.cursor,
                egui::CursorIcon::PointingHand,
                "{label} ({theme:?})"
            );
        }
    }
}

#[test]
fn quick_reactions_light_up_on_hover() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        let mut h = Harness::new();
        h.app.settings.theme = theme.clone();
        h.act(Action::OpenConversation(ConversationId::Chat(
            "19:demo-ada@unq.gbl.spaces".into(),
        )));
        h.frames(5);
        let (own, _) = bubbles(&h);
        let bubble = *own.last().expect("an own bubble is on screen");
        h.frame_with(vec![egui::Event::PointerMoved(bubble.center())]);
        h.frames(25);
        let heart = egui::pos2(bubble.left() + 3.0 + 28.0 + 14.0, bubble.top() - 16.0);
        let fill = h.app.palette.surface_hover;
        assert!(!filled_under(&h, heart, fill), "resting ({theme:?})");
        h.frame_with(vec![egui::Event::PointerMoved(heart)]);
        h.frames(2);
        assert!(
            filled_under(&h, heart, fill),
            "the emoji lights up under the pointer ({theme:?})"
        );
        assert_eq!(h.cursor, egui::CursorIcon::PointingHand, "{theme:?}");
    }
}

#[test]
fn dialog_buttons_change_fill_and_cursor_on_hover() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        let mut h = Harness::new();
        h.app.settings.theme = theme.clone();
        open_launch(&mut h);
        h.act(Action::OpenLeaveChat(LAUNCH.into()));
        let palette = h.app.palette;
        // (label, the fill it rests in, the fill under the pointer)
        for (label, rest, hover) in [
            ("  Leave  ", palette.accent, palette.accent_hover),
            ("Cancel", palette.surface, palette.surface_hover),
        ] {
            let text = find_text(&h, label)
                .first()
                .map(|t| t.visual_bounding_rect())
                .unwrap_or_else(|| panic!("{label} is in the dialog"));
            h.frame_with(vec![egui::Event::PointerMoved(egui::pos2(2.0, 2.0))]);
            h.frames(2);
            assert!(
                filled_under(&h, text.center(), rest),
                "{label} rests ({theme:?})"
            );
            h.frame_with(vec![egui::Event::PointerMoved(text.center())]);
            h.frames(2);
            assert!(
                filled_under(&h, text.center(), hover),
                "{label} hovers ({theme:?})"
            );
            assert_eq!(
                h.cursor,
                egui::CursorIcon::PointingHand,
                "{label} ({theme:?})"
            );
        }
    }
}

/// Where a painted text is, to click it.
fn ring_text_pos(h: &Harness, wanted: &str) -> Option<egui::Pos2> {
    find_text(h, wanted)
        .first()
        .map(|text| text.pos + text.galley.rect.center().to_vec2())
}

#[test]
fn a_ring_toast_offers_place_call_and_open_chat_in_both_themes_and_widths() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme.clone();
            h.app.section = Section::Calls;
            h.frames(2);
            let ctx = h.ctx.clone();
            h.app.handle_service(
                chaat::backend::ServiceEvent::Plugin {
                    id: "demo-alerts".into(),
                    generation: 0,
                    event: chaat::plugin::event::PluginEvent::IncomingCall(
                        chaat::calling::RingAlert {
                            id: "call-1".into(),
                            caller: "Ada Lovelace".into(),
                            caller_object_id: Some("u-ada".into()),
                            ring_timeout: None,
                            video: true,
                            group: false,
                            thread_id: None,
                            account: None,
                            username: None,
                        },
                    ),
                },
                &ctx,
            );
            h.frames(2);
            let painted = painted_texts(&h);
            for text in ["Ada Lovelace", "Place call", "Open chat"] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} shows at {width} in {theme:?}"
                );
            }
            // The demo has several accounts, so the card names the one it rang on.
            assert!(
                painted
                    .iter()
                    .any(|t| t.starts_with("Incoming video call for ")),
                "the account shows at {width} in {theme:?}"
            );
            let open = ring_text_pos(&h, "Open chat").expect("Open chat is painted");
            h.click(open);
            h.frames(2);
            assert!(h.app.rings.is_empty(), "the toast closes at {width}");
            assert_eq!(h.app.section, Section::Chats);
            assert!(h.app.open.is_some(), "the caller's chat opens at {width}");
        }
    }
}

#[test]
fn the_call_alerts_sign_in_panel_shows_the_code_in_both_themes_and_widths() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme.clone();
            // Only this test's plugin, not the demo's.
            h.app.settings.plugins.clear();
            h.app.plugins.clear();
            h.frames(2);
            let ctx = h.ctx.clone();
            h.app.settings.plugins.push(chaat::settings::PluginEntry {
                id: "ca".into(),
                path: "plugin.exe".into(),
                enabled: true,
                ..Default::default()
            });
            {
                let state = h.app.plugins.entry("ca".into()).or_default();
                state.running = true;
                state.offers = vec![chaat::plugin::registry::Capability::CallAlerts];
            }
            h.app.handle_service(
                chaat::backend::ServiceEvent::Plugin {
                    id: "ca".into(),
                    generation: 0,
                    event: chaat::plugin::event::PluginEvent::SignIn {
                        account: None,
                        verification_uri: "https://microsoft.com/devicelogin".into(),
                        user_code: "AB12CD34".into(),
                    },
                },
                &ctx,
            );
            h.frames(2);
            let painted = painted_texts(&h);
            for text in [
                "Sign in for call alerts",
                "AB12CD34",
                "Open sign-in page",
                "Copy code",
            ] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} shows at {width} in {theme:?}"
                );
            }
            assert!(!h.app.plugins["ca"].sign_in.is_empty());
            h.act(Action::Plugin(PluginAction::HideSignIn {
                plugin: "ca".into(),
                account: None,
            }));
            assert!(!painted_texts(&h).iter().any(|t| t == "AB12CD34"));
        }
    }
}

#[test]
fn the_calls_settings_page_points_to_plugins() {
    for width in [480.0, 1280.0] {
        let mut h = Harness::new();
        h.size.x = width;
        h.act(Action::ShowSettingsPage(SettingsPage::Calls));
        let painted = painted_texts(&h);
        for text in ["CALL ALERTS", "Open"] {
            assert!(
                painted.iter().any(|t| t == text),
                "{text:?} shows at {width}"
            );
        }
        assert!(painted.iter().any(|t| t.contains("not part of Chaat")));
        // The plugin that provides call alerts has its own page.
        let button = ring_text_pos(&h, "Open").expect("the button is painted");
        h.click(button);
        h.frames(2);
        assert_eq!(h.app.settings_page, Some(SettingsPage::Plugins));
        assert_eq!(
            h.app.plugins_view,
            PluginsView::Plugin("demo-alerts".into())
        );
        // With no such plugin, the button opens the list.
        let mut h = Harness::new();
        h.size.x = width;
        h.app.plugins.get_mut("demo-alerts").unwrap().running = false;
        h.act(Action::ShowSettingsPage(SettingsPage::Calls));
        let button = ring_text_pos(&h, "Open Plugins").expect("the list button");
        h.click(button);
        h.frames(2);
        assert_eq!(h.app.settings_page, Some(SettingsPage::Plugins));
        assert_eq!(h.app.plugins_view, PluginsView::List);
    }
}

/// One frame of just the Plugins page, so the actions it pushes can be read
/// before the app applies them.
fn plugins_page_frame(h: &mut Harness, events: Vec<egui::Event>) -> Vec<Action> {
    h.time += 1.0 / 60.0;
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, h.size)),
        time: Some(h.time),
        events,
        ..Default::default()
    };
    let mut actions = Vec::new();
    let app = &h.app;
    let mut output = h.ctx.run_ui(input, |ui| {
        chaat::ui::plugins::page(app, ui, &mut actions);
    });
    output.textures_delta.clear();
    h.shapes = output.shapes;
    actions
}

fn plugins_page_click(h: &mut Harness, wanted: &str) -> Vec<Action> {
    let pos = ring_text_pos(h, wanted).unwrap_or_else(|| panic!("{wanted:?} is painted"));
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let mut actions = plugins_page_frame(h, vec![egui::Event::PointerMoved(pos)]);
    actions.extend(plugins_page_frame(h, vec![button(true)]));
    actions.extend(plugins_page_frame(h, vec![button(false)]));
    actions.extend(plugins_page_frame(h, vec![]));
    actions
}

fn plugins_page_harness() -> Harness {
    let mut h = Harness::new();
    h.size = egui::vec2(900.0, 3000.0);
    h.app.plugins_view = PluginsView::Plugin("demo-light".into());
    plugins_page_frame(&mut h, vec![]);
    plugins_page_frame(&mut h, vec![]);
    h
}

/// The page of one plugin, drawn directly (no Settings frame).
fn plugins_page_harness_for(id: &str) -> Harness {
    let mut h = Harness::new();
    h.size = egui::vec2(900.0, 3000.0);
    h.app.plugins_view = PluginsView::Plugin(id.into());
    plugins_page_frame(&mut h, vec![]);
    plugins_page_frame(&mut h, vec![]);
    h
}

/// The plugins page at its default view: the list.
fn plugins_list_harness(width: f32, theme: ThemeChoice) -> Harness {
    let mut h = Harness::new();
    h.size = egui::vec2(width, 3000.0);
    h.app.settings.theme = theme;
    h.act(Action::ShowSettingsPage(SettingsPage::Plugins));
    h.frames(3);
    h
}

#[test]
fn plugins_list_shows_rows_and_needs_you_first() {
    for width in [380.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let h = plugins_list_harness(width, theme.clone());
            let painted = painted_texts(&h);
            for text in [
                "NEEDS YOU",
                "Status light",
                "Asks for your presence",
                "Ring alerts",
            ] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} shows at {width} in {theme:?}: {painted:?}"
                );
            }
            assert!(
                texts_in_order(&h, &["NEEDS YOU", "Status light", "Ring alerts"]),
                "the plugin that needs the person comes first at {width} in {theme:?}"
            );
            // Chaat's words and the names only: no plugin description, path
            // or option on the list.
            for text in [
                "Tells Chaat who is calling you in Teams.",
                "Flash on calls",
                "Access token",
            ] {
                assert!(
                    !painted.iter().any(|t| t == text),
                    "{text:?} stays off the list"
                );
            }
        }
    }
}

#[test]
fn plugins_list_empty_state() {
    for width in [380.0, 1280.0] {
        let mut h = Harness::new();
        h.size = egui::vec2(width, 3000.0);
        h.app.settings.plugins.clear();
        h.act(Action::ShowSettingsPage(SettingsPage::Plugins));
        h.frames(3);
        let painted = painted_texts(&h);
        assert!(painted.iter().any(|t| t == "No plugins yet"), "{painted:?}");
        assert!(painted.iter().any(|t| t.contains("such as call alerts")));
        let add = if width < 520.0 {
            "Add\u{2026}"
        } else {
            "Add plugin\u{2026}"
        };
        assert!(painted.iter().any(|t| t == add), "{add:?} in header");
        assert!(
            painted.iter().any(|t| t == "Add plugin\u{2026}"),
            "the empty state's own button"
        );
    }
}

#[test]
fn add_plugin_button_stays_in_the_header() {
    let mut h = plugins_list_harness(1280.0, ThemeChoice::Dark);
    let with_plugins = ring_text_pos(&h, "Add plugin\u{2026}").expect("button");
    h.app.settings.plugins.clear();
    h.frames(3);
    let without = ring_text_pos(&h, "Add plugin\u{2026}").expect("button");
    assert_eq!(with_plugins, without, "it does not move with the list");
    let mut h = Harness::new();
    h.size = egui::vec2(380.0, 3000.0);
    plugins_page_frame(&mut h, vec![]);
    plugins_page_frame(&mut h, vec![]);
    let actions = plugins_page_click(&mut h, "Add\u{2026}");
    assert!(
        actions.contains(&Action::Plugin(PluginAction::Add)),
        "{actions:?}"
    );
}

#[test]
fn list_row_click_opens_the_plugin() {
    let mut h = Harness::new();
    h.size = egui::vec2(900.0, 3000.0);
    plugins_page_frame(&mut h, vec![]);
    plugins_page_frame(&mut h, vec![]);
    let actions = plugins_page_click(&mut h, "Asks for your presence");
    assert!(
        actions.contains(&Action::Plugin(PluginAction::Open("demo-light".into()))),
        "{actions:?}"
    );
    let actions = plugins_page_click(&mut h, "Ring alerts");
    assert!(actions.contains(&Action::Plugin(PluginAction::Open("demo-alerts".into()))));
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::Plugin(PluginAction::SetEnabled(..)))),
        "{actions:?}"
    );
}

#[test]
fn list_switch_does_not_open() {
    let mut h = Harness::new();
    h.size = egui::vec2(900.0, 3000.0);
    plugins_page_frame(&mut h, vec![]);
    plugins_page_frame(&mut h, vec![]);
    // The first row is the one that needs the person.
    let actions = plugins_page_click(&mut h, "Turn on");
    assert!(
        actions.contains(&Action::Plugin(PluginAction::SetEnabled(
            "demo-light".into(),
            false
        ))),
        "{actions:?}"
    );
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::Plugin(PluginAction::Open(_)))),
        "{actions:?}"
    );
}

#[test]
fn plugins_page_draws_in_both_themes_narrow_and_wide() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size = egui::vec2(width, 3000.0);
            h.app.settings.theme = theme.clone();
            h.act(Action::ShowSettingsPage(SettingsPage::Plugins));
            assert!(h.frames(2) > 20, "draws at {width} in {theme:?}");
            let painted = painted_texts(&h);
            for text in ["Ring alerts", "Status light", "Desk lamp"] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} listed at {width} in {theme:?}"
                );
            }
            h.app.plugins_view = PluginsView::Plugin("demo-light".into());
            h.frames(2);
            let painted = painted_texts(&h);
            for text in ["Status light", "Flash on calls", "Access token", "Saved"] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} shows at {width} in {theme:?}"
                );
            }
        }
    }
}

#[test]
fn plugin_text_is_quoted_under_chaats_line() {
    // On the list, Chaat's line comes before any plugin's name.
    let h = plugins_list_harness(900.0, ThemeChoice::Dark);
    let intro = "Separate programs you add. Each runs as you and can reach what you can; Chaat sends it only what you allow.";
    assert!(texts_in_order(&h, &[intro, "Ring alerts"]));
    let mut h = plugins_page_harness_for("demo-alerts");
    let painted = painted_texts(&h);
    assert!(painted.iter().any(|t| t == "Ring alerts"), "the name shows");
    assert!(
        painted
            .iter()
            .any(|t| t == "Tells Chaat who is calling you in Teams.")
    );
    assert_eq!(
        painted.iter().filter(|t| *t == "About this plugin").count(),
        1,
        "only the plugin with a link shows one"
    );
    let actions = plugins_page_click(&mut h, "About this plugin");
    assert!(actions.contains(&Action::OpenUrl("https://example.com/call-alerts".into())));
}

#[test]
fn each_account_row_says_whether_the_plugin_listens_waits_or_connects() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size = egui::vec2(width, 3000.0);
            h.app.settings.theme = theme.clone();
            h.act(Action::ShowSettingsPage(SettingsPage::Plugins));
            h.app.plugins_view = PluginsView::Plugin("demo-alerts".into());
            h.frames(2);
            let painted = painted_texts(&h);
            for text in [
                "alex@contoso.com",
                "Listening for calls",
                "alex@fabrikam.com",
                "Waiting for sign-in",
                "New code",
            ] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} shows at {width} in {theme:?}"
                );
            }
            // Nothing heard yet from the plugin about either account.
            let state = h.app.plugins.get_mut("demo-alerts").unwrap();
            state.listening.clear();
            state.sign_in.clear();
            h.frames(2);
            let painted = painted_texts(&h);
            assert_eq!(
                painted
                    .iter()
                    .filter(|t| *t == "Connecting\u{2026}")
                    .count(),
                2,
                "both accounts are connecting at {width} in {theme:?}"
            );
            assert!(!painted.iter().any(|t| t == "New code"));
        }
    }
    let mut h = plugins_page_harness_for("demo-alerts");
    let second = h.app.plugin_accounts()[1].id.clone();
    let actions = plugins_page_click(&mut h, "New code");
    assert!(actions.contains(&Action::Plugin(PluginAction::SignIn {
        plugin: "demo-alerts".into(),
        account: second,
    })));
}

#[test]
fn the_plugins_own_name_and_description_show_apart_from_chaats_words() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size = egui::vec2(width, 3000.0);
            h.app.settings.theme = theme.clone();
            h.act(Action::ShowSettingsPage(SettingsPage::Plugins));
            h.app.plugins_view = PluginsView::Plugin("demo-alerts".into());
            h.frames(2);
            let painted = painted_texts(&h);
            for text in [
                "Ring alerts",
                "Tells Chaat who is calling you in Teams.",
                "About this plugin",
            ] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} shows at {width} in {theme:?}"
                );
            }
            // The link comes from the plugin's docs_url, and only when set.
            h.app.plugins.get_mut("demo-alerts").unwrap().info.docs_url = None;
            h.frames(2);
            assert!(!painted_texts(&h).iter().any(|t| t == "About this plugin"));
        }
    }
}

/// The Plugins page in settings, drawn in one theme at one width.
fn plugins_settings_harness(width: f32, theme: ThemeChoice) -> Harness {
    plugins_settings_harness_for("demo-light", width, theme)
}

/// The same, with the page of plugin `id` open.
fn plugins_settings_harness_for(id: &str, width: f32, theme: ThemeChoice) -> Harness {
    let mut h = Harness::new();
    h.size = egui::vec2(width, 3000.0);
    h.app.settings.theme = theme;
    h.act(Action::ShowSettingsPage(SettingsPage::Plugins));
    h.app.plugins_view = PluginsView::Plugin(id.into());
    h.frames(2);
    h
}

#[test]
fn a_pending_need_shows_allow_and_dont_allow() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = plugins_settings_harness(width, theme.clone());
            let painted = painted_texts(&h);
            for text in [
                "Plugin \u{201C}Status light\u{201D} asks for:",
                "your presence (availability and activity)",
                "Allow",
                "Don't allow",
            ] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} shows at {width} in {theme:?}"
                );
            }
            // Once answered (nothing pending), the notice is gone.
            h.app
                .plugins
                .get_mut("demo-light")
                .unwrap()
                .pending_needs
                .clear();
            h.frames(2);
            let painted = painted_texts(&h);
            assert!(!painted.iter().any(|t| t == "Allow" || t == "Don't allow"));
            assert!(!painted.iter().any(|t| t.ends_with("asks for:")));
        }
    }
    let mut h = plugins_page_harness();
    let actions = plugins_page_click(&mut h, "Allow");
    assert!(actions.contains(&Action::Plugin(PluginAction::ApproveNeeds(
        "demo-light".into()
    ))));
    assert!(!actions.contains(&Action::Plugin(PluginAction::DenyNeeds(
        "demo-light".into()
    ))));
    let mut h = plugins_page_harness();
    let actions = plugins_page_click(&mut h, "Don't allow");
    assert!(actions.contains(&Action::Plugin(PluginAction::DenyNeeds(
        "demo-light".into()
    ))));
    assert!(
        !actions.contains(&Action::Plugin(PluginAction::ApproveNeeds(
            "demo-light".into()
        )))
    );
}

#[test]
fn the_gets_from_chaat_list_names_all_that_a_plugin_gets() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = plugins_settings_harness(width, theme.clone());
            // Allowing presence and accounts adds both, in plain words.
            let entry = h
                .app
                .settings
                .plugins
                .iter_mut()
                .find(|e| e.id == "demo-light")
                .unwrap();
            entry.needs = vec!["presence".into(), "accounts".into()];
            entry.approved_needs = vec!["presence".into(), "accounts".into()];
            h.frames(2);
            assert!(
                texts_in_order(
                    &h,
                    &[
                        "\u{2022} your accounts' IDs and sign-in names",
                        "\u{2022} your presence (availability and activity)",
                        "\u{2022} its own settings below",
                    ]
                ),
                "all three show at {width} in {theme:?}"
            );
        }
    }
}

#[test]
fn a_changed_file_offers_run_or_keep_off() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = plugins_settings_harness_for("demo-changed", width, theme.clone());
            let painted = painted_texts(&h);
            for text in [
                "The plugin file changed since you added it. Run the new file only if you trust it. It gets the settings and secrets you saved for this plugin, and asks again for anything else.",
                "Run the new file",
                "Keep it off",
                "New file fingerprint: 3f9a1c07be52",
            ] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} shows at {width} in {theme:?}"
                );
            }
            // Only the first 12 characters of the fingerprint are shown.
            assert!(!painted.iter().any(|t| t.contains("3f9a1c07be52d")));
            h.app.plugins.get_mut("demo-changed").unwrap().file_changed = None;
            h.frames(2);
            let painted = painted_texts(&h);
            assert!(
                !painted
                    .iter()
                    .any(|t| t == "Run the new file" || t == "Keep it off")
            );
        }
    }
    let mut h = plugins_page_harness_for("demo-changed");
    let actions = plugins_page_click(&mut h, "Run the new file");
    assert!(actions.contains(&Action::Plugin(PluginAction::TrustFile(
        "demo-changed".into()
    ))));
    assert!(!actions.contains(&Action::Plugin(PluginAction::KeepOff(
        "demo-changed".into()
    ))));
    let mut h = plugins_page_harness_for("demo-changed");
    let actions = plugins_page_click(&mut h, "Keep it off");
    assert!(actions.contains(&Action::Plugin(PluginAction::KeepOff(
        "demo-changed".into()
    ))));
    assert!(!actions.contains(&Action::Plugin(PluginAction::TrustFile(
        "demo-changed".into()
    ))));
}

#[test]
fn the_docs_link_shows_its_host() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = plugins_settings_harness_for("demo-alerts", width, theme.clone());
            let y = |h: &Harness, wanted: &str| ring_text_pos(h, wanted).map(|p| p.y);
            assert!(painted_texts(&h).iter().any(|t| t == "example.com"));
            assert_eq!(
                y(&h, "example.com"),
                y(&h, "About this plugin"),
                "the host sits beside the link at {width} in {theme:?}"
            );
            // Port, path and query are left out.
            h.app.plugins.get_mut("demo-alerts").unwrap().info.docs_url =
                Some("https://docs.example.org:8443/a/b?c=d#e".into());
            h.frames(2);
            let painted = painted_texts(&h);
            assert!(painted.iter().any(|t| t == "docs.example.org"));
            assert!(!painted.iter().any(|t| t == "example.com"));
        }
    }
}

#[test]
fn a_secret_buffer_does_not_survive_leaving_the_page() {
    let secret = |value: &str| {
        Action::Plugin(PluginAction::SetSecret {
            plugin: "demo-light".into(),
            option: "token".into(),
            value: Some(value.into()),
        })
    };
    let type_secret = |h: &mut Harness| {
        plugins_page_click(h, "New value");
        plugins_page_frame(h, vec![egui::Event::Text("hunter2".into())]);
        plugins_page_frame(h, vec![]);
    };
    let any_secret_save = |actions: &[Action]| {
        actions.iter().any(|a| {
            matches!(
                a,
                Action::Plugin(PluginAction::SetSecret { value: Some(_), .. })
            )
        })
    };

    // Control: what was typed saves, and saving empties the buffer.
    let mut h = plugins_page_harness();
    type_secret(&mut h);
    let actions = plugins_page_click(&mut h, "Save");
    assert!(actions.contains(&secret("hunter2")), "{actions:?}");
    let actions = plugins_page_click(&mut h, "Save");
    assert!(!any_secret_save(&actions), "Save emptied the buffer");

    // Typed, then the page is not drawn for a frame: nothing is left.
    let mut h = plugins_page_harness();
    type_secret(&mut h);
    h.frames(1);
    plugins_page_frame(&mut h, vec![]);
    plugins_page_frame(&mut h, vec![]);
    let actions = plugins_page_click(&mut h, "Save");
    assert!(
        !any_secret_save(&actions),
        "the page forgot the typed secret"
    );

    // Typed, then Clear: the buffer goes with the saved value.
    let mut h = plugins_page_harness();
    type_secret(&mut h);
    let actions = plugins_page_click(&mut h, "Clear");
    assert!(actions.contains(&Action::Plugin(PluginAction::SetSecret {
        plugin: "demo-light".into(),
        option: "token".into(),
        value: None,
    })));
    let actions = plugins_page_click(&mut h, "Save");
    assert!(!any_secret_save(&actions), "Clear emptied the buffer");

    // The same call Settings can make when it closes.
    let mut h = plugins_page_harness();
    type_secret(&mut h);
    chaat::ui::plugins::forget_secret_buffers(&h.ctx);
    plugins_page_frame(&mut h, vec![]);
    let actions = plugins_page_click(&mut h, "Save");
    assert!(
        !any_secret_save(&actions),
        "forget_secret_buffers dropped it"
    );
}

fn secret_memory_is_empty(h: &Harness) -> bool {
    let field = egui::Id::new(("plugin-option", "secret", "demo-light", "token"));
    let buffers = h.ctx.data(|d| {
        d.get_temp::<std::collections::HashMap<(String, String), String>>(egui::Id::new(
            "plugin-secret-buffers",
        ))
    });
    // A redraw stores a fresh state; it must hold no typed text to undo.
    let empty = (egui::text::CCursorRange::default(), String::new());
    buffers.is_none_or(|b| b.is_empty())
        && egui::TextEdit::load_state(&h.ctx, field)
            .is_none_or(|state| !state.undoer().has_undo(&empty))
}

fn type_demo_secret(h: &mut Harness) {
    plugins_page_click(h, "New value");
    plugins_page_frame(h, vec![egui::Event::Text("hunter2".into())]);
    plugins_page_frame(h, vec![]);
}

#[test]
fn a_secret_buffer_is_gone_after_closing_settings() {
    let mut h = plugins_settings_harness(900.0, ThemeChoice::Dark);
    let pos = ring_text_pos(&h, "New value").expect("field");
    h.frame_with(vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    h.frame_with(vec![egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    h.frame_with(vec![egui::Event::Text("hunter2".into())]);
    h.frames(2);
    assert!(!secret_memory_is_empty(&h), "control: the secret is held");
    h.act(Action::CloseSettings);
    h.frames(2);
    assert!(secret_memory_is_empty(&h));
}

#[test]
fn a_secret_buffer_is_gone_after_switching_page() {
    let mut h = plugins_settings_harness(900.0, ThemeChoice::Dark);
    let pos = ring_text_pos(&h, "New value").expect("field");
    h.frame_with(vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    h.frame_with(vec![egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    h.frame_with(vec![egui::Event::Text("hunter2".into())]);
    h.frames(2);
    assert!(!secret_memory_is_empty(&h), "control: the secret is held");
    h.act(Action::ShowSettingsPage(SettingsPage::General));
    h.frames(2);
    assert!(secret_memory_is_empty(&h));
}

#[test]
fn saving_clearing_or_forgetting_leaves_no_text_edit_state() {
    let mut h = plugins_page_harness();
    type_demo_secret(&mut h);
    assert!(!secret_memory_is_empty(&h), "control: the secret is held");
    plugins_page_click(&mut h, "Save");
    assert!(secret_memory_is_empty(&h), "after Save");

    let mut h = plugins_page_harness();
    type_demo_secret(&mut h);
    plugins_page_click(&mut h, "Clear");
    assert!(secret_memory_is_empty(&h), "after Clear");

    let mut h = plugins_page_harness();
    type_demo_secret(&mut h);
    chaat::ui::plugins::forget_secret_buffers(&h.ctx);
    assert!(secret_memory_is_empty(&h), "after forget");
}

#[test]
fn account_rows_need_approved_accounts() {
    let mut h = plugins_settings_harness_for("demo-alerts", 900.0, ThemeChoice::Dark);
    assert!(painted_texts(&h).iter().any(|t| t == "Listening for calls"));
    let entry = h
        .app
        .settings
        .plugins
        .iter_mut()
        .find(|e| e.id == "demo-alerts")
        .unwrap();
    entry.approved_needs.clear();
    h.frames(2);
    let painted = painted_texts(&h);
    for text in [
        "Listening for calls",
        "Waiting for sign-in",
        "Connecting\u{2026}",
    ] {
        assert!(!painted.iter().any(|t| t == text), "{text:?} is hidden");
    }
}

#[test]
fn a_plugin_sign_in_card_names_chaats_account_not_the_plugins_claim() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme.clone();
            // Only the plugin with the sign-in, so its card is the one shown.
            h.app
                .plugins
                .get_mut("demo-alerts")
                .unwrap()
                .sign_in
                .clear();
            let me = h.app.plugin_accounts()[0].id.clone();
            let ctx = h.ctx.clone();
            h.app.handle_service(
                chaat::backend::ServiceEvent::Plugin {
                    id: "demo-alerts".into(),
                    generation: 0,
                    event: chaat::plugin::event::PluginEvent::SignIn {
                        account: Some(me),
                        verification_uri: "https://microsoft.com/devicelogin".into(),
                        user_code: "ZZ99YY88".into(),
                    },
                },
                &ctx,
            );
            h.frames(3);
            let painted = painted_texts(&h);
            for text in [
                "Sign in for call alerts",
                "ZZ99YY88",
                "Plugin: \u{201C}Ring alerts\u{201D}",
            ] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} shows at {width} in {theme:?}"
                );
            }
            assert!(
                painted
                    .iter()
                    .any(|t| t.ends_with("sign in as alex@contoso.com:")),
                "the card names Chaat's own account at {width} in {theme:?}"
            );
        }
    }
}

#[test]
fn toggle_choice_and_text_options_push_set_plugin_option() {
    let mut h = plugins_page_harness();
    let set = |option: &str, value: serde_json::Value| {
        Action::Plugin(PluginAction::SetOption {
            plugin: "demo-light".into(),
            option: option.into(),
            value,
        })
    };
    let actions = plugins_page_click(&mut h, "Flash on calls");
    assert!(actions.contains(&set("flash", false.into())), "{actions:?}");

    // The choice opens a list; picking the other entry sets it.
    plugins_page_click(&mut h, "High");
    plugins_page_frame(&mut h, vec![]);
    let actions = plugins_page_click(&mut h, "Low");
    assert!(
        actions.contains(&set("brightness", "low".into())),
        "{actions:?}"
    );

    // Text commits on Enter, not on each key.
    let mut h = plugins_page_harness();
    let field = ring_text_pos(&h, "192.168.1.20").expect("the saved text shows");
    let press = |pressed| egui::Event::PointerButton {
        pos: field,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    plugins_page_frame(&mut h, vec![egui::Event::PointerMoved(field)]);
    plugins_page_frame(&mut h, vec![press(true)]);
    plugins_page_frame(&mut h, vec![press(false)]);
    let typed = plugins_page_frame(&mut h, vec![egui::Event::Text("9".into())]);
    assert!(typed.is_empty(), "typing alone sets nothing");
    let enter = egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: Some(egui::Key::Enter),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let mut actions = plugins_page_frame(&mut h, vec![enter]);
    actions.extend(plugins_page_frame(&mut h, vec![]));
    let value = actions
        .iter()
        .find_map(|a| match a {
            Action::Plugin(PluginAction::SetOption { option, value, .. }) if option == "ip" => {
                value.as_str().map(str::to_owned)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("Enter commits the text: {actions:?}"));
    assert!(value.contains('9') && value.len() == "192.168.1.20".len() + 1);
}

#[test]
fn a_secret_option_never_shows_its_value() {
    let mut h = plugins_page_harness();
    let painted = painted_texts(&h);
    assert!(
        painted.iter().any(|t| t == "Saved"),
        "a stored secret says so"
    );
    assert!(
        !painted.iter().any(|t| t.contains("s3cret")),
        "the stored value is never drawn"
    );
    let field = ring_text_pos(&h, "New value").expect("the empty field shows its hint");
    let press = |pressed| egui::Event::PointerButton {
        pos: field,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    plugins_page_frame(&mut h, vec![egui::Event::PointerMoved(field)]);
    plugins_page_frame(&mut h, vec![press(true)]);
    plugins_page_frame(&mut h, vec![press(false)]);
    plugins_page_frame(&mut h, vec![egui::Event::Text("hunter2".into())]);
    plugins_page_frame(&mut h, vec![]);
    assert!(
        !painted_texts(&h).iter().any(|t| t.contains("hunter2")),
        "what is typed is masked"
    );
    let actions = plugins_page_click(&mut h, "Save");
    assert!(actions.contains(&Action::Plugin(PluginAction::SetSecret {
        plugin: "demo-light".into(),
        option: "token".into(),
        value: Some("hunter2".into()),
    })));
    let actions = plugins_page_click(&mut h, "Clear");
    assert!(actions.contains(&Action::Plugin(PluginAction::SetSecret {
        plugin: "demo-light".into(),
        option: "token".into(),
        value: None,
    })));
}

// Settings › Plugins: the rich options of a plugin.

/// The demo presence light's entry, to change what its options show.
fn demo_light(h: &mut Harness) -> &mut chaat::settings::PluginEntry {
    h.app
        .settings
        .plugins
        .iter_mut()
        .find(|e| e.id == "demo-light")
        .expect("the demo has a presence light")
}

/// Where the n-th painted text equal to `wanted` is (top to bottom as drawn).
fn text_pos_nth(h: &Harness, wanted: &str, n: usize) -> Option<egui::Pos2> {
    find_text(h, wanted)
        .get(n)
        .map(|text| text.pos + text.galley.rect.center().to_vec2())
}

/// The color a painted text was drawn in.
fn text_color(h: &Harness, wanted: &str) -> Option<egui::Color32> {
    find_text(h, wanted)
        .first()
        .and_then(|text| text.galley.job.sections.first())
        .map(|section| section.format.color)
}

fn plugins_page_press(h: &mut Harness, pos: egui::Pos2) -> Vec<Action> {
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let mut actions = plugins_page_frame(h, vec![egui::Event::PointerMoved(pos)]);
    actions.extend(plugins_page_frame(h, vec![button(true)]));
    actions.extend(plugins_page_frame(h, vec![button(false)]));
    actions.extend(plugins_page_frame(h, vec![]));
    actions
}

fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// Clicks a painted text, replaces what the field holds and returns the
/// actions of the typing frames (nothing should commit yet).
fn plugins_page_replace_text(h: &mut Harness, field: &str, text: &str) -> Vec<Action> {
    let pos = ring_text_pos(h, field).unwrap_or_else(|| panic!("{field:?} is painted"));
    let mut actions = plugins_page_press(h, pos);
    actions.extend(plugins_page_frame(
        h,
        vec![key_event(egui::Key::A, egui::Modifiers::COMMAND)],
    ));
    actions.extend(plugins_page_frame(
        h,
        vec![egui::Event::Text(text.to_owned())],
    ));
    actions.extend(plugins_page_frame(h, vec![]));
    actions
}

fn plugins_page_enter(h: &mut Harness) -> Vec<Action> {
    let enter = key_event(egui::Key::Enter, egui::Modifiers::NONE);
    let mut actions = plugins_page_frame(h, vec![enter]);
    actions.extend(plugins_page_frame(h, vec![]));
    actions
}

fn set_light(option: &str, value: serde_json::Value) -> Action {
    Action::Plugin(PluginAction::SetOption {
        plugin: "demo-light".into(),
        option: option.into(),
        value,
    })
}

fn texts_in_order(h: &Harness, wanted: &[&str]) -> bool {
    let ys: Vec<Option<f32>> = wanted
        .iter()
        .map(|t| ring_text_pos(h, t).map(|p| p.y))
        .collect();
    ys.iter().all(Option::is_some) && ys.windows(2).all(|w| w[0] < w[1])
}

fn no_option_set(actions: &[Action]) -> bool {
    !actions
        .iter()
        .any(|a| matches!(a, Action::Plugin(PluginAction::SetOption { .. })))
}

#[test]
fn options_sit_under_section_headings_in_the_order_the_sections_appear() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let h = plugins_settings_harness(width, theme.clone());
            assert!(
                texts_in_order(
                    &h,
                    &[
                        "Bulb",
                        "Bulb address",
                        "Colors",
                        "Busy color",
                        "Other settings",
                        "Color wheel"
                    ]
                ),
                "sections in first-appearance order at {width} in {theme:?}"
            );
            let painted = painted_texts(&h);
            let resets = painted.iter().filter(|t| *t == "Reset to defaults").count();
            assert_eq!(resets, 2, "one per section with something to reset");
        }
    }
}

#[test]
fn reset_to_defaults_pushes_the_reset_of_that_section() {
    let mut h = plugins_page_harness();
    let first = text_pos_nth(&h, "Reset to defaults", 0).expect("Bulb's button");
    let actions = plugins_page_press(&mut h, first);
    assert!(
        actions.contains(&Action::Plugin(PluginAction::ResetSection {
            plugin: "demo-light".into(),
            section: Some("Bulb".into()),
        })),
        "{actions:?}"
    );
    let second = text_pos_nth(&h, "Reset to defaults", 1).expect("Colors' button");
    let actions = plugins_page_press(&mut h, second);
    assert!(
        actions.contains(&Action::Plugin(PluginAction::ResetSection {
            plugin: "demo-light".into(),
            section: Some("Colors".into()),
        })),
        "{actions:?}"
    );
}

#[test]
fn advanced_options_stay_folded_until_the_sections_advanced_area_opens() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let h = plugins_settings_harness(width, theme.clone());
            let painted = painted_texts(&h);
            // The settings menu has an Advanced page of its own.
            assert!(
                painted.iter().filter(|t| *t == "Advanced").count() >= 2,
                "an Advanced area per section with advanced options"
            );
            for hidden in ["Device ID", "Turn off when Chaat quits"] {
                assert!(
                    !painted.iter().any(|t| t == hidden),
                    "{hidden:?} is folded at {width} in {theme:?}"
                );
            }
        }
    }
    let mut h = plugins_page_harness();
    let folded = painted_texts(&h);
    assert_eq!(
        folded.iter().filter(|t| *t == "Advanced").count(),
        2,
        "one Advanced area per section with advanced options"
    );
    let advanced = text_pos_nth(&h, "Advanced", 0).expect("Bulb's Advanced");
    plugins_page_press(&mut h, advanced);
    let painted = painted_texts(&h);
    assert!(painted.iter().any(|t| t == "Device ID"), "opened");
    assert!(
        !painted.iter().any(|t| t == "Turn off when Chaat quits"),
        "the other section's area stays folded"
    );
}

#[test]
fn an_option_shows_its_label_control_help_placeholder_and_required_marker() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = plugins_settings_harness(width, theme.clone());
            demo_light(&mut h).options.remove("ip");
            h.frames(2);
            let painted = painted_texts(&h);
            for text in [
                "Bulb address",
                "The bulb's address on your network.",
                "Flash on calls",
                "Blinks the bulb when a call rings.",
                // The empty field shows its placeholder.
                "192.168.1.20",
                "Anything to remember",
            ] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} shows at {width} in {theme:?}"
                );
            }
            assert_eq!(
                painted.iter().filter(|t| *t == "Required").count(),
                2,
                "the address and the token are required"
            );
            assert!(
                texts_in_order(&h, &["Bulb address", "The bulb's address on your network."]),
                "help sits under the field"
            );
            let palette = h.app.palette;
            assert_eq!(
                text_color(&h, "The bulb's address on your network."),
                Some(palette.dim)
            );
        }
    }
}

#[test]
fn an_option_with_visible_when_shows_only_while_its_condition_holds() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = plugins_settings_harness(width, theme.clone());
            assert!(
                painted_texts(&h).iter().any(|t| t == "Flash color"),
                "shown while flashing is on, at {width} in {theme:?}"
            );
            demo_light(&mut h)
                .options
                .insert("flash".into(), serde_json::json!(false));
            h.frames(2);
            let painted = painted_texts(&h);
            assert!(!painted.iter().any(|t| t == "Flash color"), "hidden");
            assert!(painted.iter().any(|t| t == "Busy color"), "others stay");
        }
    }
}

#[test]
fn a_choice_whose_stored_value_is_not_offered_says_not_available() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = plugins_settings_harness(width, theme.clone());
            assert!(
                painted_texts(&h).iter().any(|t| t == "(not available)"),
                "at {width} in {theme:?}"
            );
            demo_light(&mut h)
                .options
                .insert("mode".into(), serde_json::json!("blink"));
            h.frames(2);
            let painted = painted_texts(&h);
            assert!(!painted.iter().any(|t| t == "(not available)"));
            assert!(painted.iter().any(|t| t == "Blink"));
        }
    }
    // Picking an offered entry replaces it at once.
    let mut h = plugins_page_harness();
    plugins_page_click(&mut h, "(not available)");
    plugins_page_frame(&mut h, vec![]);
    let actions = plugins_page_click(&mut h, "Blink");
    assert!(
        actions.contains(&set_light("mode", "blink".into())),
        "{actions:?}"
    );
}

#[test]
fn text_commits_on_focus_loss_and_leaves_no_buffer_behind() {
    let mut h = plugins_page_harness();
    let typed = plugins_page_replace_text(&mut h, "192.168.1.20", "10.0.0.7");
    assert!(typed.is_empty(), "typing alone sets nothing: {typed:?}");
    let buffer = egui::Id::new(("plugin-option", "text", "demo-light", "ip"));
    assert!(
        h.ctx.data(|d| d.get_temp::<String>(buffer)).is_some(),
        "control: the typed text waits in memory"
    );
    // Clicking another control ends the edit.
    let away = ring_text_pos(&h, "Bulb address").expect("a label");
    let actions = plugins_page_press(&mut h, away);
    assert!(
        actions.contains(&set_light("ip", "10.0.0.7".into())),
        "{actions:?}"
    );
    assert!(
        h.ctx.data(|d| d.get_temp::<String>(buffer)).is_none(),
        "the buffer is cleared on commit"
    );
    // An edit that ends where it began commits nothing.
    let mut h = plugins_page_harness();
    plugins_page_replace_text(&mut h, "192.168.1.20", "192.168.1.20");
    let actions = plugins_page_enter(&mut h);
    assert!(no_option_set(&actions), "{actions:?}");
}

#[test]
fn number_options_show_their_unit_and_commit_a_number() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let h = plugins_settings_harness(width, theme.clone());
            let painted = painted_texts(&h);
            for text in ["Dim level", "80", "%"] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} shows at {width} in {theme:?}"
                );
            }
        }
    }
    let mut h = plugins_page_harness();
    let typed = plugins_page_replace_text(&mut h, "80", "55");
    assert!(typed.is_empty(), "{typed:?}");
    let actions = plugins_page_enter(&mut h);
    assert!(
        actions.contains(&set_light("level", serde_json::json!(55))),
        "{actions:?}"
    );
    // Text that is no number goes on as text, so the app can say why.
    let mut h = plugins_page_harness();
    plugins_page_replace_text(&mut h, "80", "abc");
    let actions = plugins_page_enter(&mut h);
    assert!(
        actions.contains(&set_light("level", "abc".into())),
        "{actions:?}"
    );
}

#[test]
fn a_refused_value_shows_its_error_in_the_error_color_and_stays_in_the_field() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let h = plugins_settings_harness(width, theme.clone());
            let message = "Enter a number from 10 to 100.";
            assert!(
                texts_in_order(&h, &["Dim level", message]),
                "under its field at {width} in {theme:?}"
            );
            assert_eq!(text_color(&h, message), Some(h.app.palette.danger));
        }
    }
    let mut h = plugins_page_harness();
    plugins_page_replace_text(&mut h, "80", "5");
    let actions = plugins_page_enter(&mut h);
    assert!(
        actions.contains(&set_light("level", serde_json::json!(5))),
        "{actions:?}"
    );
    // The app refused it (the error is still there): the field keeps the
    // refused text, not the saved one.
    let painted = painted_texts(&h);
    assert!(painted.iter().any(|t| t == "5"), "{painted:?}");
    assert!(!painted.iter().any(|t| t == "80"));
    // Without an error the field shows the saved value again.
    h.app
        .plugin_option_errors
        .remove(&("demo-light".to_owned(), "level".to_owned()));
    plugins_page_frame(&mut h, vec![]);
    let painted = painted_texts(&h);
    assert!(painted.iter().any(|t| t == "80"));
}

#[test]
fn multiline_commits_on_ctrl_enter_or_focus_loss_but_not_on_enter() {
    let mut h = plugins_page_harness();
    let typed = plugins_page_replace_text(&mut h, "Anything to remember", "hi");
    assert!(typed.is_empty(), "{typed:?}");
    let actions = plugins_page_enter(&mut h);
    assert!(no_option_set(&actions), "Enter adds a line: {actions:?}");
    let ctrl_enter = key_event(egui::Key::Enter, egui::Modifiers::CTRL);
    let mut actions = plugins_page_frame(&mut h, vec![ctrl_enter]);
    actions.extend(plugins_page_frame(&mut h, vec![]));
    assert!(
        actions.contains(&set_light("notes", "hi\n".into())),
        "{actions:?}"
    );
    let mut h = plugins_page_harness();
    plugins_page_replace_text(&mut h, "Anything to remember", "hi");
    let away = ring_text_pos(&h, "Dim level").expect("a label");
    let actions = plugins_page_press(&mut h, away);
    assert!(
        actions.contains(&set_light("notes", "hi".into())),
        "{actions:?}"
    );
}

#[test]
fn a_color_option_shows_its_swatch_and_commits_the_hex_on_enter() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let h = plugins_settings_harness(width, theme.clone());
            let painted = painted_texts(&h);
            assert!(painted.iter().any(|t| t == "Busy color"));
            assert!(
                painted.iter().any(|t| t == "#e5484d"),
                "the hex field shows the value at {width} in {theme:?}"
            );
            let swatch = egui::Color32::from_rgb(0xe5, 0x48, 0x4d);
            assert!(
                painted_shapes(&h).iter().any(|shape| matches!(
                    shape,
                    egui::epaint::Shape::Rect(rect) if rect.fill == swatch
                )),
                "a swatch in the color"
            );
        }
    }
    let mut h = plugins_page_harness();
    plugins_page_replace_text(&mut h, "#e5484d", "#00AAFF");
    let actions = plugins_page_enter(&mut h);
    assert!(
        actions.contains(&set_light("busy", "#00AAFF".into())),
        "the app lowercases it: {actions:?}"
    );
}

#[test]
fn closing_the_color_picker_commits_the_picked_color() {
    let mut h = plugins_page_harness();
    let swatch = painted_shapes(&h)
        .into_iter()
        .find_map(|shape| match shape {
            egui::epaint::Shape::Rect(rect)
                if rect.fill == egui::Color32::from_rgb(0xe5, 0x48, 0x4d) =>
            {
                Some(rect.rect.center())
            }
            _ => None,
        })
        .expect("the busy swatch");
    let actions = plugins_page_press(&mut h, swatch);
    assert!(actions.is_empty(), "opening commits nothing: {actions:?}");
    // Pick a color inside the picker, below the swatch.
    let inside = swatch + egui::vec2(60.0, 90.0);
    let actions = plugins_page_press(&mut h, inside);
    assert!(actions.is_empty(), "nothing commits while it is open");
    // Clicking away closes it.
    let away = egui::pos2(850.0, 20.0);
    let actions = plugins_page_press(&mut h, away);
    let picked = actions.iter().find_map(|a| match a {
        Action::Plugin(PluginAction::SetOption { option, value, .. }) if option == "busy" => {
            value.as_str()
        }
        _ => None,
    });
    let picked = picked.unwrap_or_else(|| panic!("closing commits it: {actions:?}"));
    assert!(picked.starts_with('#') && picked.len() == 7 && picked != "#e5484d");
}

#[test]
fn an_action_option_runs_on_click_and_waits_while_the_plugin_works() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let h = plugins_settings_harness(width, theme.clone());
            let painted = painted_texts(&h);
            assert!(
                painted.iter().any(|t| t == "Test the light"),
                "at {width} in {theme:?}"
            );
            assert!(
                painted.iter().any(|t| t == "Working\u{2026}"),
                "Find bulbs is working"
            );
            assert!(!painted.iter().any(|t| t == "Find bulbs"));
        }
    }
    let mut h = plugins_page_harness();
    let actions = plugins_page_click(&mut h, "Test the light");
    assert!(
        actions.contains(&Action::Plugin(PluginAction::RunAction {
            plugin: "demo-light".into(),
            action: "test".into(),
        })),
        "{actions:?}"
    );
    let actions = plugins_page_click(&mut h, "Working\u{2026}");
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::Plugin(PluginAction::RunAction { .. }))),
        "a button that is working is disabled: {actions:?}"
    );
    // Once the plugin answers, the button is a button again.
    h.app
        .plugins
        .get_mut("demo-light")
        .unwrap()
        .statuses
        .remove("scan");
    plugins_page_frame(&mut h, vec![]);
    let actions = plugins_page_click(&mut h, "Find bulbs");
    assert!(
        actions.contains(&Action::Plugin(PluginAction::RunAction {
            plugin: "demo-light".into(),
            action: "scan".into(),
        })),
        "{actions:?}"
    );
}

#[test]
fn statuses_show_the_plugins_words_quoted_in_the_palette_colors() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let h = plugins_settings_harness(width, theme.clone());
            let palette = h.app.palette;
            for (text, color) in [
                ("\u{201C}The light turned on\u{201D}", palette.success),
                ("\u{201C}No answer from the bulb\u{201D}", palette.danger),
                ("\u{201C}Searching\u{201D}\u{2026}", palette.dim),
            ] {
                assert_eq!(
                    text_color(&h, text),
                    Some(color),
                    "{text:?} at {width} in {theme:?}"
                );
            }
            assert!(
                texts_in_order(
                    &h,
                    &["Test the light", "\u{201C}The light turned on\u{201D}"]
                ),
                "an action's status sits under its button"
            );
            assert!(
                texts_in_order(
                    &h,
                    &["Bulb address", "\u{201C}No answer from the bulb\u{201D}"]
                ),
                "an option's status sits under its field"
            );
        }
    }
}

#[test]
fn an_option_of_an_unknown_kind_says_it_needs_a_newer_chaat() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let h = plugins_settings_harness(width, theme.clone());
            let painted = painted_texts(&h);
            for text in ["Color wheel", "This setting needs a newer Chaat."] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} at {width} in {theme:?}"
                );
            }
        }
    }
}

/// The demo light with its required settings unset.
fn light_needing_setup(h: &mut Harness) {
    let entry = demo_light(h);
    entry.secrets.clear();
    entry.options.remove("ip");
}

#[test]
fn a_plugin_with_unset_required_options_shows_needs_setup_on_its_page_and_its_row() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = plugins_settings_harness(width, theme.clone());
            let marked = "Fill in the settings marked Required.";
            let painted = painted_texts(&h);
            assert!(
                !painted.iter().any(|t| t == marked),
                "nothing is missing yet"
            );
            light_needing_setup(&mut h);
            h.frames(2);
            let painted = painted_texts(&h);
            assert!(
                painted.iter().any(|t| t == marked),
                "the page says so at {width} in {theme:?}"
            );
        }
    }
    let mut h = Harness::new();
    h.size = egui::vec2(900.0, 3000.0);
    light_needing_setup(&mut h);
    // The pending question comes first; once answered, the row says it.
    h.app
        .plugins
        .get_mut("demo-light")
        .unwrap()
        .pending_needs
        .clear();
    plugins_page_frame(&mut h, vec![]);
    plugins_page_frame(&mut h, vec![]);
    let actions = plugins_page_click(&mut h, "Needs setup");
    assert!(
        actions.contains(&Action::Plugin(PluginAction::Open("demo-light".into()))),
        "{actions:?}"
    );
}

#[test]
fn the_setup_view_lists_required_options_first_and_ends_with_done() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = plugins_settings_harness(width, theme.clone());
            light_needing_setup(&mut h);
            // Running and past its `ready`, so the page is the setup stage.
            let state = h.app.plugins.entry("demo-light".into()).or_default();
            state.running = true;
            state.ready_seen = true;
            h.app.plugins_view = PluginsView::Adding("demo-light".into());
            h.frames(3);
            let painted = painted_texts(&h);
            assert!(
                texts_in_order(
                    &h,
                    &[
                        "Plugin: \u{201C}Status light\u{201D}",
                        "Bulb address",
                        "Access token",
                        "Flash on calls",
                        "Busy color",
                        "Done",
                    ]
                ),
                "required first, then the rest, then Done at {width} in {theme:?}: {painted:?}"
            );
            assert!(
                !painted
                    .iter()
                    .any(|t| t == "Add plugin\u{2026}" || t == "Listening for calls"),
                "the setup view replaces the list"
            );
        }
    }
}

#[test]
fn settings_opens_the_plugins_page_once_when_a_setup_starts() {
    let mut h = Harness::new();
    h.act(Action::ShowSettingsPage(SettingsPage::General));
    h.frames(2);
    assert_eq!(h.app.settings_page, Some(SettingsPage::General));
    h.app.plugins_view = PluginsView::Adding("demo-light".into());
    h.frames(2);
    assert_eq!(h.app.settings_page, Some(SettingsPage::Plugins));
    // The person may leave the page while the setup stays open.
    h.act(Action::ShowSettingsPage(SettingsPage::General));
    h.frames(2);
    assert_eq!(h.app.settings_page, Some(SettingsPage::General));
}

/// Settings › Push updates with the demo call-alerts plugin also offering
/// `push`. `allowed` is whether the person allowed the `push` need.
fn push_plugin_harness(width: f32, theme: ThemeChoice, allowed: bool) -> Harness {
    let mut h = Harness::new();
    h.size = egui::vec2(width, 3000.0);
    h.app.settings.theme = theme;
    h.app.demo = false;
    let entry = h
        .app
        .settings
        .plugins
        .iter_mut()
        .find(|e| e.id == "demo-alerts")
        .unwrap();
    entry.needs.push("push".into());
    if allowed {
        entry.approved_needs.push("push".into());
    }
    let state = h.app.plugins.get_mut("demo-alerts").unwrap();
    state.offers.push(chaat::plugin::registry::Capability::Push);
    state.needs.push(chaat::plugin::registry::Need::Push);
    if !allowed {
        state.pending_needs.push("push");
    }
    h.act(Action::ShowSettingsPage(SettingsPage::Push));
    h.frames(2);
    h
}

fn push_plugin_event(h: &mut Harness, event: chaat::plugin::event::PushEvent) {
    let ctx = h.ctx.clone();
    h.app.handle_service(
        chaat::backend::ServiceEvent::Plugin {
            id: "demo-alerts".into(),
            generation: 0,
            event: chaat::plugin::event::PluginEvent::Push(event),
        },
        &ctx,
    );
    h.frames(2);
}

#[test]
fn push_settings_show_the_plugin_tunnel() {
    use chaat::plugin::event::PushEvent;
    let mut h = push_plugin_harness(900.0, ThemeChoice::Dark, true);
    push_plugin_event(
        &mut h,
        PushEvent::Ready {
            url: "https://a.trycloudflare.com".into(),
        },
    );
    let painted = painted_texts(&h);
    assert!(
        painted.iter().any(|t| t == "Tunnel at a.trycloudflare.com"),
        "{painted:?}"
    );
    assert!(
        painted
            .iter()
            .any(|t| t == "Plugin: \u{201C}Ring alerts\u{201D}")
    );
}

#[test]
fn push_settings_say_relay_wins() {
    let mut h = push_plugin_harness(900.0, ThemeChoice::Dark, true);
    h.app.settings.relay_url = Some("https://relay.example.com".into());
    h.frames(2);
    let painted = painted_texts(&h);
    assert!(
        painted
            .iter()
            .any(|t| t.contains("relay address is used instead")),
        "{painted:?}"
    );
    // The relay field stays available.
    assert!(painted.iter().any(|t| t == "RELAY"));
}

#[test]
fn plugins_page_lists_the_push_need() {
    let mut h = plugins_settings_harness(900.0, ThemeChoice::Dark);
    h.app
        .plugins
        .get_mut("demo-light")
        .unwrap()
        .pending_needs
        .push("push");
    h.frames(2);
    let painted = painted_texts(&h);
    assert!(
        painted.iter().any(
            |t| t == "your update notifications: which chats changed and when, no message text"
        ),
        "{painted:?}"
    );
}

#[test]
fn push_plugin_states_draw() {
    use chaat::plugin::event::PushEvent;
    for width in [380.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let want = |h: &Harness, text: &str| {
                let painted = painted_texts(h);
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} shows at {width} in {theme:?}: {painted:?}"
                );
            };
            let mut h = push_plugin_harness(width, theme.clone(), false);
            want(&h, "Allow it in Settings \u{203A} Plugins");
            let mut h2 = push_plugin_harness(width, theme.clone(), true);
            want(&h2, "Waiting for the plugin's tunnel");
            push_plugin_event(
                &mut h2,
                PushEvent::Ready {
                    url: "https://a.trycloudflare.com".into(),
                },
            );
            want(&h2, "Tunnel at a.trycloudflare.com");
            push_plugin_event(
                &mut h2,
                PushEvent::Down {
                    reason: Some("cloudflared not found".into()),
                },
            );
            want(&h2, "The plugin's tunnel is down");
            want(&h2, "\u{201C}cloudflared not found\u{201D}");
            push_plugin_event(&mut h2, PushEvent::Down { reason: None });
            want(&h2, "The plugin's tunnel is down");
            h.frames(1);
        }
    }
}

#[test]
fn push_updates_links_to_the_plugin() {
    let mut h = push_plugin_harness(900.0, ThemeChoice::Dark, false);
    let pos = text_pos_nth(&h, "Open", 0).expect("button shows");
    h.click(pos);
    // The view only queued the action; the page changes when it is applied.
    assert!(
        h.app
            .actions
            .contains(&Action::Plugin(PluginAction::Open("demo-alerts".into())))
            || h.app.plugins_view == PluginsView::Plugin("demo-alerts".into())
    );
}

#[test]
fn push_settings_relay_note_hides_the_plugin_state() {
    let mut h = push_plugin_harness(900.0, ThemeChoice::Light, false);
    h.app.settings.relay_url = Some("https://relay.example.com".into());
    h.frames(2);
    let painted = painted_texts(&h);
    assert!(
        painted
            .iter()
            .any(|t| t.contains("relay address is used instead"))
    );
    assert!(
        !painted
            .iter()
            .any(|t| t == "Allow it in Settings \u{203A} Plugins")
    );
    assert!(!painted.iter().any(|t| t == "Open Plugins"));
}

#[test]
fn push_settings_wrap_long_plugin_words_at_380() {
    use chaat::plugin::event::PushEvent;
    let mut h = push_plugin_harness(380.0, ThemeChoice::Dark, true);
    let name = "N".repeat(40) + " " + &"name ".repeat(30);
    h.app.plugins.get_mut("demo-alerts").unwrap().info.name = Some(name);
    let reason = "reason ".repeat(60);
    push_plugin_event(
        &mut h,
        PushEvent::Down {
            reason: Some(reason),
        },
    );
    let mut seen = 0;
    for shape in painted_shapes(&h) {
        if let egui::epaint::Shape::Text(text) = shape {
            let t = &text.galley.job.text;
            // Not the sign-in banner's truncated "Plugin:" line.
            if !text.galley.elided
                && (t.starts_with("Plugin: \u{201C}N") || t.starts_with("\u{201C}reason"))
            {
                seen += 1;
                assert!(text.galley.rows.len() > 1, "wraps: {t}");
                assert!(
                    text.pos.x + text.galley.rect.right() <= 380.5,
                    "inside the window: {t}"
                );
            }
        }
    }
    assert_eq!(seen, 2);
}

/// Both themes at 380 and 1280 wide, the page of `id` drawn directly.
fn plugin_page_cases(id: &str, check: impl Fn(&mut Harness, &str)) {
    for width in [380.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size = egui::vec2(width, 3000.0);
            h.app.settings.theme = theme.clone();
            h.app.plugins_view = PluginsView::Plugin(id.into());
            plugins_page_frame(&mut h, vec![]);
            plugins_page_frame(&mut h, vec![]);
            check(&mut h, &format!("{width} in {theme:?}"));
        }
    }
}

#[test]
fn plugin_page_sections_in_order() {
    plugin_page_cases("demo-light", |h, at| {
        assert!(
            texts_in_order(
                h,
                &[
                    "Status light",
                    "Shows your Teams presence on a bulb.",
                    "NEEDS YOU",
                    "STATUS",
                    "GETS FROM CHAAT",
                    "SETTINGS \u{00B7} FROM THE PLUGIN",
                    "FILE",
                ]
            ),
            "demo-light sections at {at}: {:?}",
            painted_texts(h)
        );
    });
    plugin_page_cases("demo-alerts", |h, at| {
        assert!(
            texts_in_order(
                h,
                &[
                    "Ring alerts",
                    "STATUS",
                    "GETS FROM CHAAT",
                    "ACCOUNTS",
                    "FILE"
                ]
            ),
            "demo-alerts sections at {at}: {:?}",
            painted_texts(h)
        );
        // Nothing to ask: no callout, no settings card.
        let painted = painted_texts(h);
        assert!(!painted.iter().any(|t| t == "NEEDS YOU"));
        assert!(!painted.iter().any(|t| t.starts_with("SETTINGS")));
    });
}

#[test]
fn needs_you_callout_stacks_file_then_needs_then_setup() {
    plugin_page_cases("demo-light", |h, at| {
        h.app.plugins.get_mut("demo-light").unwrap().file_changed = Some("abcdef0123456789".into());
        let entry = h
            .app
            .settings
            .plugins
            .iter_mut()
            .find(|e| e.id == "demo-light")
            .unwrap();
        entry.options.clear();
        entry.secrets.clear();
        assert!(!h.app.plugin_missing_required("demo-light").is_empty());
        plugins_page_frame(h, vec![]);
        plugins_page_frame(h, vec![]);
        assert!(
            texts_in_order(
                h,
                &[
                    "NEEDS YOU",
                    "The plugin file changed since you added it. Run the new file only if you trust it. It gets the settings and secrets you saved for this plugin, and asks again for anything else.",
                    "Plugin \u{201C}Status light\u{201D} asks for:",
                    "Fill in the settings marked Required.",
                    "STATUS",
                ]
            ),
            "callout order at {at}: {:?}",
            painted_texts(h)
        );
        assert_eq!(
            painted_texts(h)
                .iter()
                .filter(|t| *t == "NEEDS YOU")
                .count(),
            1,
            "one callout at {at}"
        );
    });
}

#[test]
fn gets_from_chaat_bullets_use_chaat_words() {
    plugin_page_cases("demo-alerts", |h, at| {
        let painted = painted_texts(h);
        assert!(
            painted
                .iter()
                .any(|t| t == "\u{2022} your accounts' IDs and sign-in names"),
            "{at}: {painted:?}"
        );
    });
    plugin_page_cases("demo-light", |h, at| {
        let painted = painted_texts(h);
        assert!(
            painted
                .iter()
                .any(|t| t == "\u{2022} its own settings below"),
            "{at}: {painted:?}"
        );
        // Presence is asked, not yet allowed.
        assert!(
            !painted
                .iter()
                .any(|t| t.starts_with('\u{2022}') && t.contains("your presence"))
        );
    });
    plugin_page_cases("demo-changed", |h, at| {
        assert!(
            painted_texts(h).iter().any(|t| t == "\u{2022} Nothing"),
            "{at}"
        );
    });
}

#[test]
fn remove_asks_in_place() {
    let mut h = plugins_page_harness_for("demo-alerts");
    // The file section is folded.
    assert!(ring_text_pos(&h, "Remove plugin\u{2026}").is_none());
    plugins_page_click(&mut h, "FILE");
    // The fold opens with a short animation.
    for _ in 0..30 {
        plugins_page_frame(&mut h, vec![]);
    }
    assert!(ring_text_pos(&h, "Remove plugin\u{2026}").is_some());
    assert!(
        painted_texts(&h)
            .iter()
            .any(|t| t == "Checked against the file you added." || t == "Not checked yet.")
    );
    let actions = plugins_page_click(&mut h, "Remove plugin\u{2026}");
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::Plugin(PluginAction::Remove(_))))
    );
    let ask = "Remove \u{201C}Ring alerts\u{201D}? Chaat stops it, deletes its saved secrets and tells it to forget your sign-ins. Chaat stops showing incoming calls from this plugin.";
    assert!(painted_texts(&h).iter().any(|t| t == ask), "asked in place");
    // Cancel puts the question away without removing.
    let actions = plugins_page_click(&mut h, "Cancel");
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::Plugin(PluginAction::Remove(_))))
    );
    assert!(!painted_texts(&h).iter().any(|t| t == ask));
    // Asking again and confirming sends today's action.
    plugins_page_click(&mut h, "Remove plugin\u{2026}");
    let actions = plugins_page_click(&mut h, "Remove");
    assert!(
        actions.contains(&Action::Plugin(PluginAction::Remove("demo-alerts".into()))),
        "{actions:?}"
    );
}

#[test]
fn back_link_returns_to_the_list() {
    let mut h = plugins_page_harness_for("demo-alerts");
    let actions = plugins_page_click(&mut h, "\u{2039} Plugins");
    assert!(
        actions.contains(&Action::Plugin(PluginAction::BackToPlugins)),
        "{actions:?}"
    );
}

#[test]
fn plugin_page_wraps_long_words_at_380() {
    let mut h = Harness::new();
    h.size = egui::vec2(380.0, 3000.0);
    h.app.plugins_view = PluginsView::Plugin("demo-alerts".into());
    let state = h.app.plugins.get_mut("demo-alerts").unwrap();
    state.info.name = Some("N".repeat(30) + " " + &"name ".repeat(30));
    state.info.description = Some("D".repeat(30) + " " + &"words ".repeat(40));
    plugins_page_frame(&mut h, vec![]);
    plugins_page_frame(&mut h, vec![]);
    let mut seen = 0;
    for shape in painted_shapes(&h) {
        if let egui::epaint::Shape::Text(text) = shape {
            let t = &text.galley.job.text;
            if t.starts_with("NNN") || t.starts_with("DDD") {
                seen += 1;
                assert!(text.galley.rows.len() > 1, "wraps: {t}");
                assert!(
                    text.pos.x + text.galley.rect.right() <= 380.5,
                    "inside: {t}"
                );
            }
        }
    }
    assert_eq!(seen, 2);
}

fn escape_event() -> egui::Event {
    egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn plugin_ids(h: &Harness) -> Vec<String> {
    h.app
        .settings
        .plugins
        .iter()
        .map(|e| e.id.clone())
        .collect()
}

/// The full app on the Plugins page in reorder mode.
fn reorder_cases(check: impl Fn(&mut Harness, &str)) {
    for width in [380.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size = egui::vec2(width, 3000.0);
            h.app.settings.theme = theme.clone();
            // The demo's ring toast would sit over the header at narrow widths.
            h.app.rings.clear();
            h.app
                .plugins
                .get_mut("demo-alerts")
                .unwrap()
                .sign_in
                .clear();
            h.act(Action::ShowSettingsPage(SettingsPage::Plugins));
            h.act(Action::Plugin(PluginAction::StartReorder));
            h.frames(3);
            check(&mut h, &format!("{width} in {theme:?}"));
        }
    }
}

#[test]
fn reorder_mode_shows_true_order_without_needs_you() {
    reorder_cases(|h, at| {
        // Normal order is demo-alerts, demo-light, demo-changed; the plain
        // list would put the ones that need the person first.
        assert!(
            texts_in_order(h, &["Ring alerts", "Status light", "Desk lamp"]),
            "true order at {at}: {:?}",
            painted_texts(h)
        );
        let painted = painted_texts(h);
        assert!(!painted.iter().any(|t| t == "NEEDS YOU"), "{at}");
        assert!(painted.iter().any(|t| t == "Provides call alerts"), "{at}");
        assert!(painted.iter().any(|t| t == "Done"), "{at}");
        assert!(!painted.iter().any(|t| t == "Reorder"), "{at}");
        assert!(
            !painted.iter().any(|t| t == "Turn on"),
            "no switches at {at}"
        );
    });
}

#[test]
fn move_buttons_hidden_at_the_ends() {
    reorder_cases(|h, at| {
        let count = |text: &str| painted_texts(h).iter().filter(|t| *t == text).count();
        assert_eq!(count("\u{2191}"), 2, "first row has no Move up at {at}");
        assert_eq!(count("\u{2193}"), 2, "last row has no Move down at {at}");
        // The first row's only button is Move down.
        let up = text_pos_nth(h, "\u{2191}", 0).unwrap();
        let alerts = ring_text_pos(h, "Ring alerts").unwrap();
        assert!(
            up.y > alerts.y + 10.0,
            "the first Move up is on row two at {at}"
        );
        // Moving the second row up puts it first.
        h.click(up);
        h.frames(2);
        assert_eq!(plugin_ids(h)[..2], ["demo-light", "demo-alerts"], "{at}");
    });
}

#[test]
fn drag_by_handle_moves() {
    reorder_cases(|h, at| {
        let name = {
            let mut found = None;
            for shape in painted_shapes(h) {
                if let egui::epaint::Shape::Text(text) = shape
                    && text.galley.job.text == "Ring alerts"
                {
                    found = Some(text.pos + vec2_y(text.galley.rect.center().y));
                }
            }
            found.expect("name painted")
        };
        // The handle sits one gap to the left of the name.
        let handle = egui::pos2(name.x - 16.0, name.y);
        let last = ring_text_pos(h, "Desk lamp").unwrap();
        h.frame_with(vec![egui::Event::PointerMoved(handle)]);
        h.frame_with(vec![egui::Event::PointerButton {
            pos: handle,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        for step in 1..=4 {
            let y = handle.y + (last.y + 4.0 - handle.y) * step as f32 / 4.0;
            h.frame_with(vec![egui::Event::PointerMoved(egui::pos2(handle.x, y))]);
        }
        let to = egui::pos2(handle.x, last.y + 4.0);
        h.frame_with(vec![egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        h.frames(2);
        assert_eq!(
            plugin_ids(h),
            ["demo-light", "demo-changed", "demo-alerts"],
            "dragged to the end at {at}"
        );
    });
}

fn vec2_y(y: f32) -> egui::Vec2 {
    egui::vec2(0.0, y)
}

#[test]
fn reorder_ends_on_done_and_escape() {
    reorder_cases(|h, at| {
        let done = ring_text_pos(h, "Done").unwrap();
        h.click(done);
        h.frames(2);
        assert!(!h.app.reordering, "Done ends it at {at}");
        assert_eq!(h.app.settings_page, Some(SettingsPage::Plugins));
        h.act(Action::Plugin(PluginAction::StartReorder));
        h.frames(2);
        assert!(h.app.reordering);
        h.frame_with(vec![escape_event()]);
        h.frames(2);
        assert!(!h.app.reordering, "Esc ends it at {at}");
        assert_eq!(
            h.app.settings_page,
            Some(SettingsPage::Plugins),
            "settings stay open at {at}"
        );
    });
}

#[test]
fn escape_on_plugin_page_goes_back_not_closing_settings() {
    let mut h = Harness::new();
    h.act(Action::ShowSettingsPage(SettingsPage::Plugins));
    h.act(Action::Plugin(PluginAction::Open("demo-alerts".into())));
    h.frames(3);
    h.frame_with(vec![escape_event()]);
    h.frames(2);
    assert_eq!(h.app.plugins_view, PluginsView::List);
    assert_eq!(h.app.settings_page, Some(SettingsPage::Plugins));
    h.frame_with(vec![escape_event()]);
    h.frames(2);
    assert_eq!(h.app.settings_page, None, "Esc on the list closes settings");
}

#[test]
fn turn_off_asks_for_a_provider_only() {
    let ask = "Turn off \u{201C}Ring alerts\u{201D}? Chaat stops showing incoming calls from this plugin.";
    for width in [380.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            for page in [false, true] {
                let mut h = Harness::new();
                h.size = egui::vec2(width, 3000.0);
                h.app.settings.theme = theme.clone();
                h.app.settings.plugins.retain(|e| e.id == "demo-alerts");
                h.app.plugins_view = if page {
                    PluginsView::Plugin("demo-alerts".into())
                } else {
                    PluginsView::List
                };
                plugins_page_frame(&mut h, vec![]);
                plugins_page_frame(&mut h, vec![]);
                let at = format!("{width} in {theme:?}, page {page}");
                let actions = plugins_page_click(&mut h, "Turn on");
                assert!(
                    !actions
                        .iter()
                        .any(|a| matches!(a, Action::Plugin(PluginAction::SetEnabled(..)))),
                );
                assert!(painted_texts(&h).iter().any(|t| t == ask), "{at}");
                assert!(
                    painted_texts(&h).iter().any(|t| t == ask),
                    "{at}: {:?} {:?}",
                    painted_texts(&h),
                    actions
                );
                // Cancel keeps it on.
                let actions = plugins_page_click(&mut h, "Cancel");
                assert!(
                    !actions
                        .iter()
                        .any(|a| matches!(a, Action::Plugin(PluginAction::SetEnabled(..)))),
                    "{at}"
                );
                assert!(!painted_texts(&h).iter().any(|t| t == ask), "{at}");
                plugins_page_click(&mut h, "Turn on");
                let actions = plugins_page_click(&mut h, "Turn off");
                assert!(
                    actions.contains(&Action::Plugin(PluginAction::SetEnabled(
                        "demo-alerts".into(),
                        false
                    ))),
                    "{at}: {actions:?}"
                );
            }
        }
    }
    // A plugin that provides nothing turns off at once, on the list and page.
    for page in [false, true] {
        let mut h = Harness::new();
        h.size = egui::vec2(900.0, 3000.0);
        h.app.settings.plugins.retain(|e| e.id == "demo-light");
        h.app.plugins_view = if page {
            PluginsView::Plugin("demo-light".into())
        } else {
            PluginsView::List
        };
        plugins_page_frame(&mut h, vec![]);
        plugins_page_frame(&mut h, vec![]);
        let actions = plugins_page_click(&mut h, "Turn on");
        assert!(
            actions.contains(&Action::Plugin(PluginAction::SetEnabled(
                "demo-light".into(),
                false
            ))),
            "{actions:?}"
        );
    }
}

#[test]
fn removing_a_provider_says_what_stops() {
    let mut h = plugins_page_harness_for("demo-alerts");
    plugins_page_click(&mut h, "FILE");
    for _ in 0..30 {
        plugins_page_frame(&mut h, vec![]);
    }
    plugins_page_click(&mut h, "Remove plugin\u{2026}");
    assert!(
        painted_texts(&h)
            .iter()
            .any(|t| t.starts_with("Remove \u{201C}Ring alerts\u{201D}?")
                && t.ends_with("Chaat stops showing incoming calls from this plugin.")),
        "{:?}",
        painted_texts(&h)
    );
}

/// The demo light as a plugin in its add flow.
fn adding_light(h: &mut Harness, running: bool) {
    demo_light(h).setup_pending = true;
    let state = h.app.plugins.entry("demo-light".into()).or_default();
    state.running = running;
    state.ready_seen = running;
    state.pending_needs = vec!["accounts", "presence"];
    h.app.plugins_view = PluginsView::Adding("demo-light".into());
}

#[test]
fn add_flow_starting_setup_failed_states() {
    for width in [380.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = plugins_settings_harness(width, theme.clone());
            adding_light(&mut h, false);
            h.app.plugins.get_mut("demo-light").unwrap().message = None;
            h.frames(3);
            let painted = painted_texts(&h);
            assert!(
                painted.iter().any(|t| t == "Starting the plugin\u{2026}"),
                "starting at {width} in {theme:?}: {painted:?}"
            );
            assert!(!painted.iter().any(|t| t == "Done"));
            assert!(h.repaint_after < std::time::Duration::from_secs(60));

            adding_light(&mut h, true);
            h.frames(3);
            let painted = painted_texts(&h);
            assert!(
                painted
                    .iter()
                    .any(|t| t == "IT ASKS FOR" || t == "It asks for"),
                "setup at {width} in {theme:?}: {painted:?}"
            );
            assert!(painted.iter().any(|t| t == "Done"));

            adding_light(&mut h, false);
            h.app.plugins.get_mut("demo-light").unwrap().message = Some("exit 1".into());
            h.frames(3);
            let painted = painted_texts(&h);
            for text in ["Stopped: exit 1", "Try again", "Remove"] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} at {width} in {theme:?}: {painted:?}"
                );
            }
        }
    }
    let mut h = plugins_page_harness();
    adding_light(&mut h, false);
    h.app.plugins.get_mut("demo-light").unwrap().message = Some("exit 1".into());
    plugins_page_frame(&mut h, vec![]);
    let actions = plugins_page_click(&mut h, "Try again");
    assert!(actions.contains(&Action::Plugin(PluginAction::RetryStart(
        "demo-light".into()
    ))));
}

#[test]
fn a_reopened_add_flow_past_its_limit_fails_without_a_short_repaint() {
    let limit = chaat::app::ADD_START_LIMIT;
    // Freshly started: one repaint at the limit, not a short one.
    let mut h = plugins_settings_harness(900.0, ThemeChoice::Dark);
    adding_light(&mut h, false);
    h.app.plugins.get_mut("demo-light").unwrap().message = None;
    h.app
        .pretend_plugin_started("demo-light", std::time::Duration::from_secs(1));
    h.frames(30);
    assert!(
        painted_texts(&h)
            .iter()
            .any(|t| t == "Starting the plugin\u{2026}")
    );
    assert!(
        h.repaint_after > std::time::Duration::from_secs(5),
        "{:?}",
        h.repaint_after
    );
    assert!(h.repaint_after <= limit + std::time::Duration::from_millis(100));
    // Left earlier and reopened past the limit: Failed, and nothing short.
    let mut h = plugins_settings_harness(900.0, ThemeChoice::Dark);
    adding_light(&mut h, false);
    h.app.plugins.get_mut("demo-light").unwrap().message = None;
    h.app
        .pretend_plugin_started("demo-light", limit + std::time::Duration::from_secs(5));
    h.frames(30);
    let painted = painted_texts(&h);
    assert!(
        painted.iter().any(|t| t == "Try again"),
        "failed: {painted:?}"
    );
    assert!(!painted.iter().any(|t| t == "Starting the plugin\u{2026}"));
    assert!(
        h.repaint_after > std::time::Duration::from_secs(5),
        "{:?}",
        h.repaint_after
    );
}

#[test]
fn a_running_plugin_without_ready_is_still_starting() {
    let mut h = plugins_page_harness();
    adding_light(&mut h, true);
    h.app.plugins.get_mut("demo-light").unwrap().ready_seen = false;
    plugins_page_frame(&mut h, vec![]);
    let painted = painted_texts(&h);
    assert!(painted.iter().any(|t| t == "Starting the plugin\u{2026}"));
    assert!(!painted.iter().any(|t| t == "Done"), "{painted:?}");
}

#[test]
fn escape_on_the_add_flow_does_nothing() {
    let mut h = Harness::new();
    h.size = egui::vec2(900.0, 3000.0);
    adding_light(&mut h, true);
    h.app.settings_page = Some(SettingsPage::Plugins);
    h.frames(3);
    h.frame_with(vec![escape_event()]);
    h.frames(2);
    assert_eq!(h.app.settings_page, Some(SettingsPage::Plugins));
    assert_eq!(h.app.plugins_view, PluginsView::Adding("demo-light".into()));
    assert!(h.app.settings.plugins.iter().any(|e| e.id == "demo-light"));
}

#[test]
fn remove_confirm_and_turn_off_question_do_not_outlive_settings() {
    let ask = "Turn off \u{201C}Ring alerts\u{201D}? Chaat stops showing incoming calls from this plugin.";
    let mut h = Harness::new();
    h.size = egui::vec2(900.0, 3000.0);
    h.app.settings.plugins.retain(|e| e.id == "demo-alerts");
    h.act(Action::ShowSettingsPage(SettingsPage::Plugins));
    h.act(Action::Plugin(PluginAction::Open("demo-alerts".into())));
    plugins_page_frame(&mut h, vec![]);
    plugins_page_frame(&mut h, vec![]);
    plugins_page_click(&mut h, "Turn on");
    assert!(painted_texts(&h).iter().any(|t| t == ask));
    // Settings closes, then opens on the same plugin again.
    h.act(Action::CloseSettings);
    h.frames(3);
    h.act(Action::ShowSettingsPage(SettingsPage::Plugins));
    h.act(Action::Plugin(PluginAction::Open("demo-alerts".into())));
    h.frames(3);
    assert!(
        !painted_texts(&h).iter().any(|t| t == ask),
        "question is gone"
    );
}

#[test]
fn done_disabled_until_required_options_set() {
    let mut h = plugins_page_harness();
    light_needing_setup(&mut h);
    adding_light(&mut h, true);
    plugins_page_frame(&mut h, vec![]);
    let actions = plugins_page_click(&mut h, "Done");
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::Plugin(PluginAction::FinishSetup { .. }))),
        "{actions:?}"
    );
    let mut h = plugins_page_harness();
    adding_light(&mut h, true);
    plugins_page_frame(&mut h, vec![]);
    let actions = plugins_page_click(&mut h, "Done");
    assert!(
        actions.contains(&Action::Plugin(PluginAction::FinishSetup {
            id: "demo-light".into(),
            allow: Vec::new()
        })),
        "{actions:?}"
    );
}

#[test]
fn need_boxes_start_unticked() {
    let mut h = plugins_page_harness();
    adding_light(&mut h, true);
    plugins_page_frame(&mut h, vec![]);
    assert!(
        painted_texts(&h)
            .iter()
            .any(|t| t == "(nothing is sent until you tick it)")
    );
    let actions = plugins_page_click(&mut h, "Done");
    assert!(
        actions.contains(&Action::Plugin(PluginAction::FinishSetup {
            id: "demo-light".into(),
            allow: Vec::new()
        })),
        "nothing ticked sends nothing: {actions:?}"
    );
    adding_light(&mut h, true);
    plugins_page_frame(&mut h, vec![]);
    plugins_page_click(&mut h, "your presence (availability and activity)");
    let actions = plugins_page_click(&mut h, "Done");
    assert!(
        actions.contains(&Action::Plugin(PluginAction::FinishSetup {
            id: "demo-light".into(),
            allow: vec!["presence".into()]
        })),
        "only the ticked one: {actions:?}"
    );
}

#[test]
fn cancel_removes_the_new_plugin() {
    let mut h = Harness::new();
    h.size = egui::vec2(900.0, 3000.0);
    adding_light(&mut h, true);
    h.frames(3);
    let before = h.app.settings.plugins.len();
    h.act(Action::Plugin(PluginAction::CancelSetup(
        "demo-light".into(),
    )));
    assert_eq!(h.app.settings.plugins.len(), before - 1);
    assert_eq!(h.app.plugins_view, PluginsView::List);
}

#[test]
fn finish_setup_row_reopens_setup() {
    let mut h = Harness::new();
    h.size = egui::vec2(900.0, 3000.0);
    demo_light(&mut h).setup_pending = true;
    h.app.plugins.entry("demo-light".into()).or_default();
    plugins_page_frame(&mut h, vec![]);
    plugins_page_frame(&mut h, vec![]);
    let actions = plugins_page_click(&mut h, "Finish setup");
    assert!(
        actions.contains(&Action::Plugin(PluginAction::Open("demo-light".into()))),
        "{actions:?}"
    );
    h.act(Action::Plugin(PluginAction::Open("demo-light".into())));
    assert_eq!(h.app.plugins_view, PluginsView::Adding("demo-light".into()));
}

/// The widest right edge among everything the page painted.
fn painted_right_edge(h: &Harness) -> f32 {
    painted_shapes(h)
        .into_iter()
        .map(|shape| shape.visual_bounding_rect().right())
        .fold(0.0, f32::max)
}

#[test]
fn plugins_list_stays_inside_the_panel_at_380_with_long_names() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        let mut h = Harness::new();
        h.size = egui::vec2(380.0, 3000.0);
        h.app.settings.theme = theme.clone();
        let long = "L".repeat(60) + " plugin with a very long name that keeps going";
        for id in ["demo-light", "demo-alerts"] {
            h.app.plugins.get_mut(id).unwrap().info.name = Some(long.clone());
        }
        plugins_page_frame(&mut h, vec![]);
        plugins_page_frame(&mut h, vec![]);
        let painted = painted_texts(&h);
        assert!(painted.iter().any(|t| t == "Add\u{2026}"), "{painted:?}");
        assert!(painted.iter().any(|t| t == "Reorder"), "{painted:?}");
        let right = painted_right_edge(&h);
        assert!(right <= 380.5, "list reaches {right} in {theme:?}");
        // The long name is cut with an ellipsis, not wrapped over the switch.
        for shape in painted_shapes(&h) {
            if let egui::epaint::Shape::Text(text) = shape
                && text.galley.job.text.starts_with("LLL")
            {
                assert_eq!(text.galley.rows.len(), 1, "one line in {theme:?}");
            }
        }
        // The two header buttons do not overlap.
        let rect_of = |wanted: &str| {
            painted_shapes(&h).into_iter().find_map(|s| match s {
                egui::epaint::Shape::Text(t) if t.galley.job.text == wanted => {
                    Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
                }
                _ => None,
            })
        };
        let add = rect_of("Add\u{2026}").unwrap();
        let reorder = rect_of("Reorder").unwrap();
        assert!(!add.intersects(reorder), "{add:?} {reorder:?}");
    }
}

#[test]
fn plugin_page_stays_inside_the_panel_at_380() {
    for id in ["demo-light", "demo-alerts", "demo-changed"] {
        let mut h = Harness::new();
        h.size = egui::vec2(380.0, 3000.0);
        h.app.plugins_view = PluginsView::Plugin(id.into());
        h.app.plugins.get_mut(id).unwrap().info.name =
            Some("N".repeat(40) + " " + &"name ".repeat(20));
        plugins_page_frame(&mut h, vec![]);
        plugins_page_frame(&mut h, vec![]);
        let right = painted_right_edge(&h);
        assert!(right <= 380.5, "{id} page reaches {right}");
    }
}

#[test]
fn account_status_goes_under_the_name_when_narrow() {
    let mut h = Harness::new();
    h.size = egui::vec2(380.0, 3000.0);
    h.app.plugins_view = PluginsView::Plugin("demo-alerts".into());
    plugins_page_frame(&mut h, vec![]);
    plugins_page_frame(&mut h, vec![]);
    let pos = |wanted: &str| {
        painted_shapes(&h).into_iter().find_map(|s| match s {
            egui::epaint::Shape::Text(t) if t.galley.job.text == wanted => Some(t.pos),
            _ => None,
        })
    };
    let name = h.app.plugin_accounts()[0].username.clone();
    let name = pos(&name).expect("account name");
    let status = pos("Listening for calls")
        .or_else(|| pos("Waiting for sign-in"))
        .or_else(|| pos("Connecting\u{2026}"))
        .expect("account status");
    assert!(
        status.y > name.y,
        "status under the name: {status:?} {name:?}"
    );
}

#[test]
fn plugin_status_names_starting_and_stopped() {
    let mut h = plugins_page_harness_for("demo-alerts");
    let state = h.app.plugins.get_mut("demo-alerts").unwrap();
    state.running = false;
    state.message = None;
    plugins_page_frame(&mut h, vec![]);
    assert!(painted_texts(&h).iter().any(|t| t == "Starting\u{2026}"));
    h.app.plugins.get_mut("demo-alerts").unwrap().message = Some("exit 1".into());
    plugins_page_frame(&mut h, vec![]);
    assert!(painted_texts(&h).iter().any(|t| t == "Stopped: exit 1"));
}

#[test]
fn setup_page_has_chaats_title_and_the_plugins_caption() {
    let mut h = plugins_settings_harness(900.0, ThemeChoice::Dark);
    adding_light(&mut h, true);
    h.frames(3);
    assert!(
        texts_in_order(
            &h,
            &[
                "Set up a new plugin",
                "Plugin: \u{201C}Status light\u{201D}",
                "SETTINGS \u{00B7} FROM THE PLUGIN",
                "Done"
            ]
        ),
        "{:?}",
        painted_texts(&h)
    );
}

/// Presses `key` for one frame on the bare Plugins page.
fn plugins_page_key(h: &mut Harness, key: egui::Key) -> Vec<Action> {
    let event = |pressed| egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let mut actions = plugins_page_frame(h, vec![event(true)]);
    actions.extend(plugins_page_frame(h, vec![event(false)]));
    actions
}

#[test]
fn list_rows_open_with_tab_and_enter_and_the_switch_toggles_with_space() {
    let mut h = Harness::new();
    h.size = egui::vec2(900.0, 3000.0);
    plugins_page_frame(&mut h, vec![]);
    plugins_page_frame(&mut h, vec![]);
    let mut opened = false;
    let mut toggled = false;
    for _ in 0..12 {
        plugins_page_key(&mut h, egui::Key::Tab);
        let actions = plugins_page_key(&mut h, egui::Key::Enter);
        opened |= actions.contains(&Action::Plugin(PluginAction::Open("demo-light".into())));
        let actions = plugins_page_key(&mut h, egui::Key::Space);
        toggled |= actions.iter().any(
            |a| matches!(a, Action::Plugin(PluginAction::SetEnabled(id, _)) if id == "demo-light"),
        ) && !actions
            .iter()
            .any(|a| matches!(a, Action::Plugin(PluginAction::Open(_))));
    }
    assert!(opened, "a row opens its plugin from the keyboard");
    assert!(toggled, "Space on the switch toggles without opening");
}

#[test]
fn new_plugin_files_keep_to_the_type_scale_and_never_repaint_at_once() {
    let scale = ["11", "12", "13", "14", "15", "18", "20", "24", "30"];
    for (name, source) in [
        ("mod", include_str!("../src/ui/plugins/mod.rs")),
        ("options", include_str!("../src/ui/plugins/options.rs")),
        ("list", include_str!("../src/ui/plugins/list.rs")),
        ("page", include_str!("../src/ui/plugins/page.rs")),
        ("setup", include_str!("../src/ui/plugins/setup.rs")),
    ] {
        assert!(
            !source.contains("request_repaint()"),
            "{name} repaints at once"
        );
        for font in ["regular(", "medium(", "semibold(", "bold("] {
            for (at, _) in source.match_indices(&format!("theme::{font}")) {
                let rest = &source[at + "theme::".len() + font.len()..];
                let size = rest.split(".0)").next().unwrap_or("");
                assert!(
                    scale.contains(&size),
                    "{name}: font size {size:?} is off the scale"
                );
            }
        }
    }
}

// -- Local composer, reading and navigation ------------------------------------
//
// Each test runs in both themes and at a narrow (760) and a wide (1400) window.

/// Runs `check` on a fresh harness for each theme and width.
fn each_view(mut check: impl FnMut(&mut Harness)) {
    for width in [760.0, 1400.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = themed(&theme, width);
            check(&mut h);
        }
    }
}

fn ada() -> ConversationId {
    ConversationId::Chat("19:demo-ada@unq.gbl.spaces".into())
}

/// The first one-on-one chat, the one the demo opens.
fn first_chat(h: &Harness) -> ConversationId {
    let chat = h
        .app
        .chats
        .iter()
        .find(|c| c.kind == chaat::model::ChatKind::OneOnOne)
        .expect("the demo has a one-on-one chat");
    ConversationId::Chat(chat.id.clone())
}

fn key_with(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// Opens a chat, puts `text` in its composer with the caret at `caret` (in
/// chars), and focuses it.
fn compose_in(h: &mut Harness, chat: &ConversationId, text: &str, caret: usize) {
    h.act(Action::OpenConversation(chat.clone()));
    let draft = h.app.drafts.entry(chat.clone()).or_default();
    draft.text = text.into();
    draft.paste_cursor = Some(caret);
    h.app.focus_composer = true;
    h.frames(4);
}

/// The composer's box: the lowest large surface-filled rectangle on the
/// conversation side.
fn composer_box(h: &Harness) -> egui::Rect {
    let pane = h.app.settings.sidebar_width;
    painted_shapes(h)
        .into_iter()
        .filter_map(|shape| match shape {
            egui::epaint::Shape::Rect(rect)
                if rect.fill == h.app.palette.surface
                    && rect.rect.left() > pane
                    && rect.rect.width() > 250.0
                    && rect.stroke.width > 0.0 =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .max_by(|a, b| a.bottom().total_cmp(&b.bottom()))
        .expect("the composer box is painted")
}

/// Where the composer's right-hand buttons are, counted from the right:
/// 0 is Send, 1 Attach, 2 Emoji, 3 Formatting. Valid for a one-line editor.
fn composer_button(boxed: egui::Rect, from_right: usize) -> egui::Pos2 {
    // 1 point of stroke and 8 of margin, then 30-point buttons 2 apart.
    let right = boxed.right() - 9.0;
    egui::pos2(right - 15.0 - from_right as f32 * 32.0, boxed.center().y)
}

fn text_center(h: &Harness, wanted: &str) -> Option<egui::Pos2> {
    find_text(h, wanted)
        .into_iter()
        .map(|text| text.pos + text.galley.size() / 2.0)
        // The lowest one: a picker's grid sits below its tabs.
        .max_by(|a, b| a.y.total_cmp(&b.y))
}

fn has_text(h: &Harness, wanted: &str) -> bool {
    !find_text(h, wanted).is_empty()
}

#[test]
fn the_formatting_toggle_shows_and_hides_the_toolbar_inside_the_composer() {
    each_view(|h| {
        compose_in(h, &ada(), "", 0);
        // At the end of the history, where the newest message is in view.
        h.frame_with(vec![
            egui::Event::PointerMoved(egui::pos2(h.size.x * 0.7, 300.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -5000.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            },
        ]);
        h.frames(30);
        let off = composer_box(h);
        assert!(!chaat::ui::composer_tools::format_on(&h.ctx));

        let lowest_bubble = |h: &Harness| {
            let (own, others) = bubbles(h);
            own.iter()
                .chain(&others)
                .map(|r| r.bottom())
                .fold(0.0, f32::max)
        };
        let gap_off = off.top() - lowest_bubble(h);
        let toggle = composer_button(off, 3);
        h.click(toggle);
        assert!(chaat::ui::composer_tools::format_on(&h.ctx));
        h.frames(10);
        let on = composer_box(h);
        assert!(
            on.height() > off.height() + 20.0,
            "the toolbar row adds height inside the box: {off:?} then {on:?}"
        );
        assert!(
            (on.bottom() - off.bottom()).abs() < 1.0,
            "the box grows upward"
        );
        // The history follows the box: its newest bubble keeps its distance
        // and nothing is resized.
        let gap_on = on.top() - lowest_bubble(h);
        assert!(
            (gap_on - gap_off).abs() < 1.0,
            "the history keeps its distance to the box: {gap_off} then {gap_on} (width {})",
            h.size.x
        );

        // The same spot turns it off again.
        h.click(composer_button(on, 3));
        assert!(!chaat::ui::composer_tools::format_on(&h.ctx));
        let back = composer_box(h);
        assert!((back.height() - off.height()).abs() < 1.0);
    });
}

#[test]
fn ctrl_b_wraps_the_selection_with_or_without_the_toolbar() {
    for toolbar in [false, true] {
        each_view(|h| {
            chaat::ui::composer_tools::set_format_on(&h.ctx, toolbar);
            compose_in(h, &ada(), "make bold", 9);
            h.frame_with(vec![key_with(egui::Key::A, egui::Modifiers::COMMAND)]);
            h.frame_with(vec![key_with(egui::Key::B, egui::Modifiers::COMMAND)]);
            h.frames(2);
            assert_eq!(
                h.app.drafts[&ada()].text,
                "**make bold**",
                "toolbar {toolbar}"
            );
        });
    }
}

#[test]
fn the_emoji_picker_opens_from_the_composer_and_inserts_at_the_caret() {
    each_view(|h| {
        compose_in(h, &ada(), "ab", 1);
        let boxed = composer_box(h);
        h.click(composer_button(boxed, 2));
        assert!(has_text(h, "Search emoji"), "the picker is open");
        let first = text_center(h, "\u{1F600}").expect("the first emoji is drawn");
        h.click(first);
        h.frames(3);
        assert_eq!(h.app.drafts[&ada()].text, "a\u{1F600}b");
    });
}

#[test]
fn more_reactions_opens_the_same_picker() {
    each_view(|h| {
        h.act(Action::OpenConversation(ada()));
        h.frames(5);
        let (own, _) = bubbles(h);
        let bubble = *own.last().expect("an own bubble is on screen");
        h.frame_with(vec![egui::Event::PointerMoved(bubble.center())]);
        h.frames(27);
        assert!(!has_text(h, "Search emoji"));
        // The sixth quick reaction (angry face) is the last before the More
        // button, which sits half a reaction and half a button further on.
        let sixth = text_center(h, "\u{1F621}").expect("the hover bar shows");
        h.click(egui::pos2(sixth.x + 16.0 + 14.0, sixth.y));
        assert!(has_text(h, "Search emoji"), "the picker is open");
    });
}

#[test]
fn shortcodes_convert_as_typed_and_an_emoticon_converts_on_send() {
    each_view(|h| {
        compose_in(h, &ada(), "", 0);
        h.frame_with(vec![egui::Event::Text(":smile:".into())]);
        h.frames(2);
        assert_eq!(h.app.drafts[&ada()].text, "\u{1F604}");

        // An emoticon waits for its space, or for Enter.
        h.app.drafts.get_mut(&ada()).unwrap().text.clear();
        h.frames(2);
        h.frame_with(vec![egui::Event::Text("ok :)".into())]);
        h.frames(2);
        assert_eq!(h.app.drafts[&ada()].text, "ok :)");
        let count = h.app.conversations[&ada()].messages.len();
        h.frame_with(vec![key_with(egui::Key::Enter, egui::Modifiers::NONE)]);
        h.frames(3);
        let messages = &h.app.conversations[&ada()].messages;
        assert_eq!(messages.len(), count + 1);
        assert_eq!(messages.last().unwrap().body.to_plain(), "ok \u{1F642}");
    });
}

/// Hits for `query` in a chat's messages, newest first.
fn hits_for(h: &Harness, id: &ConversationId, query: &str) -> Vec<chaat::model::SearchHit> {
    h.app
        .conversation(id)
        .unwrap()
        .messages
        .iter()
        .rev()
        .filter(|m| m.body.to_plain().to_lowercase().contains(query))
        .map(|m| chaat::model::SearchHit {
            conversation: id.clone(),
            message_id: m.id.clone(),
            author: m.author.clone(),
            snippet: m.body.summary(60),
            at: m.created,
        })
        .collect()
}

#[test]
fn find_opens_with_ctrl_f_counts_hits_and_closes_with_escape() {
    each_view(|h| {
        let chat = first_chat(h);
        h.act(Action::OpenConversation(chat.clone()));
        h.frames(3);
        h.frame_with(vec![key_with(egui::Key::F, egui::Modifiers::COMMAND)]);
        h.frames(3);
        assert!(h.app.find.is_some(), "Ctrl+F opens the bar");
        assert_eq!(h.app.overlay, None, "and not the switcher");
        assert!(has_text(h, "Find in this conversation"));

        // Escape closes it from the field.
        assert_eq!(
            h.ctx.memory(|m| m.focused()),
            Some(egui::Id::new("chaat-find-field")),
            "the field has the keyboard"
        );
        press(h, egui::Key::Escape);
        assert!(h.app.find.is_none(), "Esc closes the bar");

        // Hits: the demo does not search, so answer the query here.
        let hits = hits_for(h, &chat, "the");
        assert!(hits.len() >= 2, "the demo chat has two matches");
        let count = hits.len();
        h.app.find = Some(chaat::model::FindState {
            query: "the".into(),
            hits: hits.clone(),
            current: 0,
            searched: "the".into(),
        });
        h.frames(3);
        assert!(has_text(h, &format!("1 of {count}")));
        h.app.apply(Action::FindNext(1), &h.ctx);
        assert_eq!(h.app.jump_to.as_deref(), Some(hits[1].message_id.as_str()));
        h.frames(3);
        assert!(has_text(h, &format!("2 of {count}")));

        h.app.find = Some(chaat::model::FindState {
            query: "zzzqqq".into(),
            hits: Vec::new(),
            current: 0,
            searched: "zzzqqq".into(),
        });
        h.frames(3);
        assert!(has_text(h, "No matches"));
    });
}

#[test]
fn ctrl_f_without_a_conversation_still_opens_the_switcher() {
    each_view(|h| {
        h.act(Action::Navigate(Section::Calendar));
        h.frames(2);
        h.frame_with(vec![key_with(egui::Key::F, egui::Modifiers::COMMAND)]);
        h.frames(3);
        assert_eq!(h.app.overlay, Some(Overlay::Switcher));
        assert!(h.app.find.is_none());
    });
}

/// How many "jump to latest" buttons are painted: a 36-point round button.
fn jump_buttons(h: &Harness) -> usize {
    painted_shapes(h)
        .into_iter()
        .filter(|shape| {
            matches!(shape, egui::epaint::Shape::Circle(c)
                if (c.radius - 18.0).abs() < 0.1
                    && (c.fill == h.app.palette.surface || c.fill == h.app.palette.surface_hover))
        })
        .count()
}

#[test]
fn jump_to_latest_shows_when_scrolled_up_and_returns_to_the_end() {
    each_view(|h| {
        let chat = first_chat(h);
        h.act(Action::OpenConversation(chat.clone()));
        h.frames(3);
        let entry = h.app.conversations.get_mut(&chat).unwrap();
        let template = entry.messages.last().unwrap().clone();
        let count = 120;
        let mut all: Vec<_> = (0..count)
            .map(|i| {
                let mut m = template.clone();
                m.id = format!("long-{i}");
                m.created = template.created - (count - i) as i64 * 60_000;
                m
            })
            .collect();
        all.append(&mut entry.messages);
        entry.messages = all;
        entry.layout.invalidate();
        h.app.scroll_to_end = 4;
        h.frames(8);
        assert_eq!(jump_buttons(h), 0, "hidden at the bottom");

        let at = egui::pos2(h.size.x * 0.7, 300.0);
        let mut events = vec![egui::Event::PointerMoved(at)];
        for _ in 0..6 {
            events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, 700.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            });
        }
        h.frame_with(events);
        h.frames(40);
        assert_eq!(jump_buttons(h), 1, "shown after scrolling up a screen");

        let button = painted_shapes(h)
            .into_iter()
            .find_map(|shape| match shape {
                egui::epaint::Shape::Circle(c) if (c.radius - 18.0).abs() < 0.1 => Some(c.center),
                _ => None,
            })
            .unwrap();
        h.click(button);
        h.frames(10);
        assert_eq!(jump_buttons(h), 0, "back at the end, the button is gone");
    });
}

#[test]
fn opening_a_pinned_message_in_the_open_chat_scrolls_to_it_and_keeps_focus() {
    each_view(|h| {
        let chat = first_chat(h);
        h.act(Action::OpenConversation(chat.clone()));
        h.frames(3);
        let entry = h.app.conversations.get_mut(&chat).unwrap();
        let template = entry.messages.last().unwrap().clone();
        let count = 120;
        let mut all: Vec<_> = (0..count)
            .map(|i| {
                let mut m = template.clone();
                m.id = format!("long-{i}");
                m.created = template.created - (count - i) as i64 * 60_000;
                m
            })
            .collect();
        all.append(&mut entry.messages);
        entry.messages = all;
        entry.layout.invalidate();
        h.app.scroll_to_end = 4;
        h.frames(8);
        assert_eq!(jump_buttons(h), 0, "starts at the bottom");
        h.ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
        h.frames(1);

        h.act(Action::OpenMessage {
            conversation: chat.clone(),
            message: "long-5".into(),
        });
        assert_eq!(
            h.ctx.memory(|m| m.focused()),
            None,
            "the composer does not take focus"
        );
        h.frames(40);
        assert_eq!(jump_buttons(h), 1, "scrolled up to the pinned message");
    });
}

#[test]
fn a_draft_previews_in_its_chat_row_but_not_in_the_open_chat() {
    each_view(|h| {
        let other = ConversationId::Chat("19:demo-grace@unq.gbl.spaces".into());
        h.app.drafts.insert(
            other.clone(),
            chaat::app::Draft {
                text: "Can we move the review?".into(),
                ..Default::default()
            },
        );
        h.act(Action::OpenConversation(ada()));
        h.frames(3);
        assert!(has_text(h, "Draft: Can we move the review?"));
        h.act(Action::OpenConversation(other));
        h.frames(3);
        assert!(
            !painted_texts(h).iter().any(|t| t.starts_with("Draft: ")),
            "the open chat shows its draft in the composer instead"
        );
    });
}

/// A copy of the newest message from someone else, with `html` as its body.
fn injected(h: &Harness, id: &ConversationId, key: &str, html: &str) -> chaat::model::Message {
    let me = h.app.me().to_owned();
    let mut message = h
        .app
        .conversation(id)
        .unwrap()
        .messages
        .iter()
        .rev()
        .find(|m| !m.is_from(&me) && !m.system)
        .unwrap()
        .clone();
    message.id = format!("test-{key}");
    message.created += 1_000;
    message.reactions.clear();
    message.attachments.clear();
    message.body = chaat::html::parse(html);
    message
}

fn push_message(h: &mut Harness, id: &ConversationId, message: chaat::model::Message) {
    let entry = h.app.conversations.get_mut(id).unwrap();
    entry.messages.push(message);
    entry.layout.invalidate();
    h.app.scroll_to_end = 4;
    h.frames(6);
}

#[test]
fn a_failed_send_offers_retry_and_delete() {
    each_view(|h| {
        let chat = first_chat(h);
        h.act(Action::OpenConversation(chat.clone()));
        h.frames(3);
        let mut message = injected(h, &chat, "failed", "<p>This one did not go out</p>");
        message.author_id = Some(h.app.me().to_owned());
        message.delivery = chaat::model::Delivery::Failed;
        push_message(h, &chat, message);
        for word in ["Not sent.", "Retry", "Delete"] {
            assert!(has_text(h, word), "{word} is shown");
        }
        let delete = text_center(h, "Delete").unwrap();
        h.click(delete);
        h.frames(3);
        assert!(
            !h.app.conversations[&chat]
                .messages
                .iter()
                .any(|m| m.id == "test-failed"),
            "Delete removes the message"
        );
        assert!(!has_text(h, "Not sent."));
    });
}

#[test]
fn a_rich_message_draws_code_tables_sup_sub_and_sizes() {
    let html = concat!(
        "<p>Here is the fix, and the numbers:</p>",
        "<codeblock class=\"language-rust\"><code>fn main() {\n    println!(\"hi\");\n}</code></codeblock>",
        "<table><thead><tr><th>Name</th><th>Total</th></tr></thead>",
        "<tbody><tr><td>North</td><td>52</td></tr></tbody></table>",
        "<p>E = mc<sup>2</sup> and H<sub>2</sub>O, then <span style=\"font-size:24px\">big text</span> and normal again.</p>",
    );
    each_view(|h| {
        let chat = first_chat(h);
        h.act(Action::OpenConversation(chat.clone()));
        h.frames(3);
        let message = injected(h, &chat, "rich", html);
        push_message(h, &chat, message);
        assert!(has_text(h, "Rust"), "the code block names its language");
        assert!(has_text(h, "North"), "the table's cells are drawn");
    });
}

#[test]
fn escape_closes_the_mention_list_without_freezing() {
    let mut h = Harness::new();
    h.act(Action::OpenConversation(ConversationId::Chat(
        "19:demo-launch@thread.v2".into(),
    )));
    let chat = h.app.open.clone().unwrap();
    let first = h.app.conversations[&chat].messages[0].id.clone();
    h.act(Action::StartReply(first));
    h.app.focus_composer = true;
    h.frames(3);
    h.frame_with(vec![egui::Event::Text("@".into())]);
    h.frames(2);
    // Esc used to deadlock here: the dismissal wrote egui memory while
    // the input lock was held.
    h.frame_with(vec![egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    h.frames(3);
    assert_eq!(h.app.drafts[&chat].text, "@", "Esc keeps the typed text");
    assert!(
        h.app.drafts[&chat].reply_to.is_some(),
        "Esc with the mention list open closes the list, not the reply"
    );
}

#[test]
fn a_dismissed_mention_list_opens_again_for_a_new_at_sign() {
    let key = |key| egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let mut h = Harness::new();
    h.act(Action::OpenConversation(ConversationId::Chat(
        "19:demo-launch@thread.v2".into(),
    )));
    let chat = h.app.open.clone().unwrap();
    h.app.focus_composer = true;
    h.frames(3);
    h.frame_with(vec![egui::Event::Text("@".into())]);
    h.frames(2);
    h.frame_with(vec![key(egui::Key::Escape)]);
    h.frames(2);
    // Esc took focus; click back in by asking for it, as a person would.
    h.app.focus_composer = true;
    h.frames(2);
    h.frame_with(vec![key(egui::Key::Backspace)]);
    h.frames(2);
    assert!(h.app.drafts.get(&chat).is_none_or(|d| d.text.is_empty()));
    h.frame_with(vec![egui::Event::Text("@".into())]);
    h.frames(2);
    h.frame_with(vec![key(egui::Key::Tab)]);
    assert_eq!(
        h.app.drafts[&chat].text, "@Ada Lovelace ",
        "a new @ in the same place shows the list again"
    );
}

#[test]
fn enter_in_a_list_adds_items_and_an_empty_item_ends_it() {
    let key = |modifiers| egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: Some(egui::Key::Enter),
        pressed: true,
        repeat: false,
        modifiers,
    };
    for (width, theme) in [(720.0, ThemeChoice::Dark), (1280.0, ThemeChoice::Light)] {
        let mut h = Harness::new();
        h.size.x = width;
        h.app.settings.theme = theme;
        h.act(Action::OpenConversation(ConversationId::Chat(
            "19:demo-ada@unq.gbl.spaces".into(),
        )));
        let chat = h.app.open.clone().unwrap();
        h.app.focus_composer = true;
        h.frames(3);
        let count = h.app.conversations[&chat].messages.len();
        h.frame_with(vec![egui::Event::Text("- one".into())]);
        h.frames(1);
        h.frame_with(vec![key(egui::Modifiers::NONE)]);
        h.frames(1);
        assert_eq!(h.app.drafts[&chat].text, "- one\n- ", "Enter adds an item");
        h.frame_with(vec![egui::Event::Text("two".into())]);
        h.frames(1);
        h.frame_with(vec![key(egui::Modifiers::SHIFT)]);
        h.frames(1);
        assert_eq!(h.app.drafts[&chat].text, "- one\n- two\n- ");
        h.frame_with(vec![key(egui::Modifiers::NONE)]);
        h.frames(1);
        assert_eq!(
            h.app.drafts[&chat].text, "- one\n- two\n",
            "Enter on an empty item ends the list"
        );
        assert_eq!(h.app.conversations[&chat].messages.len(), count);
        h.frame_with(vec![key(egui::Modifiers::NONE)]);
        h.frames(2);
        assert_eq!(
            h.app.conversations[&chat].messages.len(),
            count + 1,
            "Enter after the list sends"
        );
    }
}

#[test]
fn escape_in_the_composer_cancels_a_reply() {
    let mut h = Harness::new();
    h.app.focus_composer = true;
    h.frames(3);
    let chat = h.app.open.clone().unwrap();
    let first = h.app.conversations[&chat].messages[0].id.clone();
    h.act(Action::StartReply(first));
    h.frames(3);
    let editor = h
        .ctx
        .memory(|m| m.focused())
        .expect("the composer has focus");
    assert!(h.app.drafts[&chat].reply_to.is_some());
    h.frame_with(vec![egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    h.frames(3);
    let _ = editor;
    assert!(
        h.app.drafts.get(&chat).is_none_or(|d| d.reply_to.is_none()),
        "Esc in the composer cancels the reply"
    );
}

// -- Chat actions: hide, mark unread, pins, links, cards, status, meeting IDs --

/// The narrowest window Chaat allows (`with_min_inner_size` in main.rs) and a
/// wide one.
const CHAT_ACTION_WIDTHS: [f32; 2] = [720.0, 1600.0];

const GRACE_CHAT: &str = "19:demo-grace@unq.gbl.spaces";
const OFFSITE_CHAT: &str = "19:demo-offsite@thread.v2";

fn themed(theme: &ThemeChoice, width: f32) -> Harness {
    let mut h = Harness::new();
    h.size.x = width;
    h.app.settings.theme = theme.clone();
    h.frames(3);
    h
}

/// The center of the leftmost painted text equal to `wanted` (the sidebar
/// paints before anything beside it).
fn leftmost_text(h: &Harness, wanted: &str) -> Option<egui::Pos2> {
    find_text(h, wanted)
        .into_iter()
        .map(|text| text.pos + text.galley.rect.center().to_vec2())
        .min_by(|a, b| a.x.total_cmp(&b.x))
}

fn right_click(h: &mut Harness, pos: egui::Pos2) {
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Secondary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    h.frame_with(vec![egui::Event::PointerMoved(pos), button(true)]);
    h.frame_with(vec![button(false)]);
    h.frames(3);
}

fn right_click_on(h: &mut Harness, wanted: &str) {
    let pos = leftmost_text(h, wanted).unwrap_or_else(|| panic!("{wanted} is painted"));
    right_click(h, pos);
}

/// Opens the More menu of the lowest message bubble from someone else (clear
/// of the floating cards at the top right on narrow windows).
fn open_more_menu(h: &mut Harness) {
    h.frames(3);
    let bar = 6.0 * 32.0 + 28.0 + 9.0 + 2.0 * 28.0 + 6.0;
    let (_, others) = bubbles(h);
    let bubble = others
        .into_iter()
        .max_by(|a, b| a.bottom().total_cmp(&b.bottom()))
        .expect("a bubble from someone else is on screen");
    h.frame_with(vec![egui::Event::PointerMoved(bubble.center())]);
    h.frames(25); // past the hover delay
    // The bar is painted as a surface-filled rect of its own width; More is
    // its last button.
    let surface = h.app.palette.surface;
    let bar_rect = painted_shapes(h)
        .into_iter()
        .filter_map(|shape| match shape {
            egui::epaint::Shape::Rect(r)
                if same_fill(r.fill, surface) && (r.rect.width() - bar).abs() < 4.0 =>
            {
                Some(r.rect)
            }
            _ => None,
        })
        .next()
        .expect("the hover bar is painted");
    h.click(egui::pos2(bar_rect.right() - 17.0, bar_rect.center().y));
}

#[test]
fn chat_row_menu_hides_with_undo() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in CHAT_ACTION_WIDTHS {
            let mut h = themed(&theme, width);
            let row = leftmost_text(&h, "Katherine Johnson").expect("the row is listed");
            right_click(&mut h, row);
            assert!(
                has_text(&h, "Hide"),
                "the menu offers Hide ({theme:?}, {width})"
            );
            // The menu is as wide as its items need, not as wide as the
            // window (a context menu offers all of it as room).
            let hide = text_rect(&h, "Hide").unwrap();
            // The menu's frame and shadow are the narrowest wide rects
            // under its first item (its fill is still fading in here).
            let menu = painted_shapes(&h)
                .into_iter()
                .filter_map(|shape| match shape {
                    egui::epaint::Shape::Rect(r)
                        if r.rect.contains(hide.center()) && r.rect.width() >= 150.0 =>
                    {
                        Some(r.rect)
                    }
                    _ => None,
                })
                .min_by(|a, b| a.width().total_cmp(&b.width()))
                .expect("the menu is painted");
            assert!(
                menu.width() < 260.0,
                "the menu is narrow ({theme:?}, {width}): {menu:?}"
            );
            h.click(leftmost_text(&h, "Hide").unwrap());
            let id = "19:demo-katherine@unq.gbl.spaces";
            assert!(
                h.app.chat(id).unwrap().hidden,
                "hidden ({theme:?}, {width})"
            );
            assert!(
                leftmost_text(&h, "Katherine Johnson").is_none(),
                "the row leaves the list ({theme:?}, {width})"
            );
            assert!(has_text(&h, "Chat hidden"), "the toast shows");
            // Undo stands out from the message: the link color.
            let link = h.app.palette.link;
            assert!(
                painted_shapes(&h).into_iter().any(|shape| matches!(
                    shape,
                    egui::epaint::Shape::Text(t)
                        if t.galley.job.text == "Undo"
                            && t.galley.job.sections[0].format.color == link
                )),
                "Undo is in the link color ({theme:?}, {width})"
            );
            h.click(leftmost_text(&h, "Undo").expect("the toast offers Undo"));
            assert!(
                !h.app.chat(id).unwrap().hidden,
                "Undo unhides ({theme:?}, {width})"
            );
            assert!(leftmost_text(&h, "Katherine Johnson").is_some());
        }
    }
}

#[test]
fn undo_toast_holds_while_hovered() {
    let mut h = themed(&ThemeChoice::Dark, 1280.0);
    let id = "19:demo-katherine@unq.gbl.spaces";
    h.act(Action::HideChat(id.into()));
    let undo = leftmost_text(&h, "Undo").expect("the toast offers Undo");
    let toast_shown = |h: &Harness| h.app.toasts.iter().any(|t| t.text == "Chat hidden");
    assert!(toast_shown(&h));
    h.frame_with(vec![egui::Event::PointerMoved(undo)]);
    // Past the 6 s limit with the pointer on the toast: still there.
    for _ in 0..4 {
        h.time += 3.0;
        h.frame_with(Vec::new());
    }
    assert!(toast_shown(&h), "the toast holds while hovered");
    // Pointer away: it expires after its 6 s.
    h.frame_with(vec![egui::Event::PointerMoved(egui::pos2(5.0, 5.0))]);
    for _ in 0..4 {
        h.time += 3.0;
        h.frame_with(Vec::new());
    }
    h.frames(3);
    assert!(!toast_shown(&h), "the toast leaves after the pointer does");
}

#[test]
fn hidden_filter_lists_and_unhides() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in CHAT_ACTION_WIDTHS {
            let mut h = themed(&theme, width);
            h.act(Action::ShowAccount(0));
            assert!(
                leftmost_text(&h, "Offsite planning").is_none(),
                "hidden chats stay out of the list ({theme:?}, {width})"
            );
            // At narrow widths the demo's floating sign-in card covers the chip.
            dismiss_sign_in_card(&mut h);
            h.click(hidden_chip_pos(&h));
            assert!(h.app.hidden_only);
            assert!(leftmost_text(&h, "Offsite planning").is_some());
            assert!(leftmost_text(&h, "Launch crew").is_none());
            right_click_on(&mut h, "Offsite planning");
            assert!(has_text(&h, "Unhide"), "the menu offers Unhide");
            h.click(leftmost_text(&h, "Unhide").unwrap());
            assert!(!h.app.chat(OFFSITE_CHAT).unwrap().hidden);
            assert!(
                has_text(&h, "No hidden chats"),
                "the empty state shows ({theme:?}, {width})"
            );
            h.click(hidden_chip_pos(&h));
            // The chat is back in the full list; the oldest rows can sit
            // below the fold, so filter to bring it into view.
            h.app.chat_filter = "Offsite".into();
            h.frames(3);
            assert!(
                leftmost_text(&h, "Offsite planning").is_some(),
                "the chat is listed again ({theme:?}, {width})"
            );
        }
    }
}

#[test]
fn mark_unread_from_menu() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in CHAT_ACTION_WIDTHS {
            let mut h = themed(&theme, width);
            let me = h.app.me().to_owned();
            assert!(!h.app.chat(GRACE_CHAT).unwrap().is_unread(&me));
            right_click_on(&mut h, "Grace Hopper");
            h.click(leftmost_text(&h, "Mark as unread").expect("the menu offers it"));
            let chat = h.app.chat(GRACE_CHAT).unwrap();
            assert!(
                chat.marked_unread && chat.is_unread(&me),
                "({theme:?}, {width})"
            );
            right_click_on(&mut h, "Grace Hopper");
            assert!(
                has_text(&h, "Mark as read"),
                "the label flips ({theme:?}, {width})"
            );
            h.click(leftmost_text(&h, "Mark as read").unwrap());
            assert!(!h.app.chat(GRACE_CHAT).unwrap().is_unread(&me));
        }
    }
}

#[test]
fn pin_banner_shows_and_unpins() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in CHAT_ACTION_WIDTHS {
            let mut h = themed(&theme, width);
            h.act(Action::OpenConversation(ConversationId::Chat(
                LAUNCH.into(),
            )));
            h.frames(3);
            assert!(has_text(&h, "1 of 2"), "two pins ({theme:?}, {width})");
            assert!(
                painted_texts(&h)
                    .iter()
                    .any(|t| t.contains("I've drafted the rollback plan")),
                "the banner shows the pin's preview"
            );
            for left in [2, 1] {
                let unpin = banner_text(&h, "Unpin")
                    .unwrap_or_else(|| panic!("the banner offers Unpin ({theme:?}, {width})"));
                h.click(unpin.center());
                h.frames(2);
                assert_eq!(
                    h.app.pins[LAUNCH].len(),
                    left - 1,
                    "a click on Unpin unpins ({theme:?}, {width})"
                );
            }
            assert!(
                leftmost_text(&h, "Unpin").is_none(),
                "no banner without pins ({theme:?}, {width})"
            );
        }
    }
}

/// The bounds of a painted text in the pin banner's row, under the header.
fn banner_text(h: &Harness, wanted: &str) -> Option<egui::Rect> {
    let band = chaat::theme::HEADER_HEIGHT..chaat::theme::HEADER_HEIGHT + 34.0;
    painted_shapes(h).into_iter().find_map(|shape| match shape {
        egui::epaint::Shape::Text(t) if t.galley.job.text == wanted => {
            let rect = shape.visual_bounding_rect();
            band.contains(&rect.center().y).then_some(rect)
        }
        _ => None,
    })
}

/// The pin banner's author and preview label.
fn banner_preview(h: &Harness) -> Option<egui::Rect> {
    let band = chaat::theme::HEADER_HEIGHT..chaat::theme::HEADER_HEIGHT + 34.0;
    painted_shapes(h).into_iter().find_map(|shape| match shape {
        egui::epaint::Shape::Text(t) if t.galley.job.text.contains("I've drafted") => {
            let rect = shape.visual_bounding_rect();
            band.contains(&rect.center().y).then_some(rect)
        }
        _ => None,
    })
}

#[test]
fn a_click_on_the_painted_unpin_text_unpins_under_a_floating_card() {
    // The demo shows the call-alerts sign-in card at the top right. It used
    // to start right under the header and cover the banner's Unpin button,
    // so a click on the text landed on the card.
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in CHAT_ACTION_WIDTHS {
            let mut h = themed(&theme, width);
            h.act(Action::OpenConversation(ConversationId::Chat(
                LAUNCH.into(),
            )));
            h.frames(3);
            assert!(
                has_text(&h, "Sign in for call alerts"),
                "the floating card shows ({theme:?}, {width})"
            );
            let unpin = banner_text(&h, "Unpin").expect("the banner offers Unpin");
            h.frame_with(vec![egui::Event::PointerMoved(unpin.center())]);
            h.frames(2);
            assert_eq!(
                h.cursor,
                egui::CursorIcon::PointingHand,
                "the Unpin text is clickable ({theme:?}, {width})"
            );
            h.click(unpin.center());
            h.frames(2);
            assert_eq!(
                h.app.pins[LAUNCH].len(),
                1,
                "a click on the Unpin text unpins ({theme:?}, {width})"
            );
        }
    }
}

#[test]
fn the_filter_chips_keep_the_sidebar_at_its_set_width() {
    // The Hidden filter is an icon after the kind chips, so all four chips
    // sit in one row at the default 300 points (a wrapped row costs the list
    // a line; a row that did not wrap widened the sidebar).
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in CHAT_ACTION_WIDTHS {
            let mut h = themed(&theme, width);
            h.act(Action::ShowAccount(0));
            h.frames(3);
            assert_eq!(h.app.settings.sidebar_width, 300.0, "({theme:?}, {width})");
            let mut rows = Vec::new();
            for chip in ["Unread", "Groups", "Meetings"] {
                rows.push(text_rect(&h, chip).unwrap_or_else(|| panic!("the {chip} chip shows")));
            }
            let hidden = hidden_chip_pos(&h);
            assert!(
                rows.iter().all(|r| (r.center().y - hidden.y).abs() < 3.0),
                "one row ({theme:?}, {width}): {rows:?} {hidden:?}"
            );
            assert!(
                hidden.x + 14.0 <= 64.0 + 300.0,
                "the icon stays in the sidebar"
            );
            // The icon toggles the filter (the demo's sign-in card floats over
            // it at narrow widths, so remove that first).
            dismiss_sign_in_card(&mut h);
            h.click(hidden);
            assert!(h.app.hidden_only, "the icon turns the filter on");
            h.click(hidden);
            assert!(!h.app.hidden_only, "and off again");
            // Nothing hidden: no icon, and a click where it was does nothing.
            for chat in &mut h.app.chats {
                chat.hidden = false;
            }
            h.frames(3);
            h.click(hidden);
            assert!(
                !h.app.hidden_only,
                "no Hidden filter without hidden chats ({theme:?}, {width})"
            );
        }
    }
}

/// Removes the demo's call-alerts sign-in card, which floats over the top
/// right of a narrow window.
fn dismiss_sign_in_card(h: &mut Harness) {
    h.app
        .plugins
        .get_mut("demo-alerts")
        .unwrap()
        .sign_in
        .clear();
    h.frames(3);
}

/// The bounds of the leftmost painted text equal to `wanted`.
fn text_rect(h: &Harness, wanted: &str) -> Option<egui::Rect> {
    find_text(h, wanted)
        .into_iter()
        .map(|text| text.visual_bounding_rect())
        .min_by(|a, b| a.left().total_cmp(&b.left()))
}

/// Where the Hidden icon chip is when it shows: after the Meetings chip.
fn hidden_chip_pos(h: &Harness) -> egui::Pos2 {
    let meetings = text_rect(h, "Meetings").expect("the Meetings chip shows");
    // The chip pads its text by 12 on each side; the row spaces chips by 6.
    egui::pos2(
        meetings.right() + 12.0 + 6.0 + chaat::ui::widgets::ICON_CHIP_WIDTH / 2.0,
        meetings.center().y,
    )
}

#[test]
fn pin_banner_preview_stops_before_its_controls() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in [640.0, 720.0, 1600.0] {
            let mut h = themed(&theme, width);
            h.act(Action::OpenConversation(ConversationId::Chat(
                LAUNCH.into(),
            )));
            h.frames(3);
            let preview = banner_preview(&h).expect("the banner shows the preview");
            let count = banner_text(&h, "1 of 2").expect("the banner shows the count");
            let unpin = banner_text(&h, "Unpin").expect("the banner offers Unpin");
            // The previous-pin arrow (24 points) sits left of the count.
            assert!(
                preview.right() <= count.left() - 24.0,
                "the preview stops before the arrows ({theme:?}, {width}): {preview:?} {count:?}"
            );
            assert!(unpin.right() <= width, "Unpin stays in the window");
        }
    }
}

#[test]
fn copy_link_in_more_menu() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in CHAT_ACTION_WIDTHS {
            let mut h = themed(&theme, width);
            // The demo opens on Ada's chat, whose messages are wide enough.
            open_more_menu(&mut h);
            for item in ["Copy text", "Pin", "Copy link"] {
                assert!(
                    has_text(&h, item),
                    "the menu offers {item} ({theme:?}, {width})"
                );
            }
            h.click(leftmost_text(&h, "Copy link").unwrap());
            assert!(has_text(&h, "Link copied"), "({theme:?}, {width})");
        }
    }
}

#[test]
fn people_panel_shows_activity() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        // The people button shows its count (the test finds it by that text)
        // unless the header would leave the title under 80 points; both
        // widths here leave more, so the count shows.
        for width in [900.0, 1600.0] {
            let mut h = themed(&theme, width);
            h.act(Action::OpenConversation(ConversationId::Chat(
                "19:demo-oncall@thread.v2".into(),
            )));
            h.frames(5);
            let button = header_text(&h, "4").expect("the people button shows the count");
            h.click(button.center());
            for expected in ["People (4)", "In a call"] {
                assert!(
                    has_text(&h, expected),
                    "the panel shows {expected} ({theme:?}, {width})"
                );
            }
        }
    }
}

#[test]
fn status_dialog_clear_after() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in CHAT_ACTION_WIDTHS {
            let mut h = themed(&theme, width);
            h.act(Action::OpenStatusMessage);
            h.frames(3);
            assert!(has_text(&h, "Clear after"), "({theme:?}, {width})");
            for label in ["Never", "Today", "1 hour", "4 hours", "This week"] {
                assert!(has_text(&h, label), "the dialog offers {label}");
            }
            // All five choices share one row, under their label.
            let label = text_rect(&h, "Clear after").unwrap();
            let rows: Vec<f32> = ["Never", "Today", "1 hour", "4 hours", "This week"]
                .into_iter()
                .map(|t| text_rect(&h, t).unwrap().center().y)
                .collect();
            assert!(
                rows.iter().all(|y| (y - rows[0]).abs() < 3.0),
                "one row of choices ({theme:?}, {width}): {rows:?}"
            );
            assert!(label.bottom() <= rows[0], "the label is above the choices");
            h.click(leftmost_text(&h, "4 hours").unwrap());
            h.click(leftmost_text(&h, "  Save  ").unwrap());
            assert_eq!(
                h.app.status_expiry,
                chaat::model::StatusExpiry::FourHours,
                "Save keeps the choice ({theme:?}, {width})"
            );
        }
    }
}

#[test]
fn join_box_accepts_meeting_id() {
    use chaat::backend::AccountEvent;
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in CHAT_ACTION_WIDTHS {
            let mut h = themed(&theme, width);
            h.act(Action::Navigate(Section::Calls));
            h.frames(3);
            // A narrow page stacks Join under the field and shortens the
            // hint, so it is not cut off.
            let hint = if width < 1000.0 {
                "Meeting link or ID"
            } else {
                "Paste a Teams meeting link or meeting ID"
            };
            assert!(
                has_text(&h, hint),
                "the hint names meeting IDs ({theme:?}, {width})"
            );
            if width < 1000.0 {
                let field = text_rect(&h, hint).unwrap();
                let join = text_rect(&h, "Join").unwrap();
                assert!(
                    join.center().y > field.bottom(),
                    "Join sits under the field"
                );
            }
            assert!(App::is_meeting_id("123 456 789 012"));
            assert!(!App::is_meeting_id("12 34"));
            // A lookup that finds nothing says so and keeps the page.
            let ctx = h.ctx.clone();
            h.app.handle(
                AccountEvent::MeetingById {
                    id: "123456789012".into(),
                    join_url: None,
                },
                &ctx,
            );
            h.frames(3);
            assert!(
                h.app
                    .toasts
                    .iter()
                    .any(|t| t.text.starts_with("No meeting found")),
                "({theme:?}, {width})"
            );
            assert!(h.app.call.is_none());
        }
    }
}

#[test]
fn adaptive_card_renders() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in CHAT_ACTION_WIDTHS {
            let mut h = themed(&theme, width);
            // The pin banner has its own test; keep it out of the width check.
            h.app.pins.clear();
            h.act(Action::OpenConversation(ConversationId::Chat(
                LAUNCH.into(),
            )));
            h.frames(5);
            for expected in [
                "Launch readiness",
                "Build",
                "0.2 preview",
                "Tests",
                "Passing",
                "Burndown chart",
                "Open checklist",
            ] {
                assert!(
                    has_text(&h, expected),
                    "the card shows {expected} ({theme:?}, {width})"
                );
            }
            assert!(
                widest_text(&h) <= width,
                "the card fits at {width}: {}",
                widest_text(&h)
            );
        }
    }
}

#[test]
fn card_facts_keep_titles_whole_in_tight_rows() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in CHAT_ACTION_WIDTHS {
            let mut h = themed(&theme, width);
            h.app.pins.clear();
            open_launch(&mut h);
            // A wrapping title in a grid column broke "Owner" into "Owne" and
            // "r", and every row was a full control tall.
            let build = text_rect(&h, "Build").expect("the first fact title shows");
            let owner = text_rect(&h, "Owner").expect("the second fact title shows whole");
            assert!(
                owner.height() < 20.0,
                "the title stays on one line ({theme:?}, {width}): {owner:?}"
            );
            assert!(
                width < 1000.0 || owner.center().y - build.center().y < 30.0, // narrow: values wrap
                "fact rows are tight ({theme:?}, {width}): {build:?} {owner:?}"
            );
            assert!(
                has_text(&h, "Open image"),
                "the image link says what it opens ({theme:?}, {width})"
            );
        }
    }
}

#[test]
fn pin_banner_shows_the_message_when_there_is_little_room() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for (width, with_author) in [(640.0, false), (1600.0, true)] {
            let mut h = themed(&theme, width);
            open_launch(&mut h);
            let band = chaat::theme::HEADER_HEIGHT..chaat::theme::HEADER_HEIGHT + 34.0;
            let label = painted_shapes(&h)
                .into_iter()
                .find_map(|shape| match shape {
                    egui::epaint::Shape::Text(t)
                        if t.galley.job.text.contains("I've drafted")
                            && band.contains(&shape.visual_bounding_rect().center().y) =>
                    {
                        Some(t.galley.job.text.clone())
                    }
                    _ => None,
                })
                .expect("the banner shows the pin");
            assert_eq!(
                label.starts_with("Grace Hopper"),
                with_author,
                "the author shows only with room ({theme:?}, {width}): {label}"
            );
        }
    }
}

#[test]
fn notifications_page_both_themes_narrow_and_wide() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme;
            h.act(Action::ShowSettingsPage(SettingsPage::Notifications));
            assert!(h.frames(2) > 20, "draws at {width}");
            let painted = painted_texts(&h);
            assert!(
                painted
                    .iter()
                    .any(|t| t == SettingsPage::Notifications.label())
            );
            assert!(
                !painted.iter().any(|t| t.starts_with("Paused until")),
                "not paused at {width}"
            );

            // Paused, with one followed person.
            h.app.settings.notify.paused_until = Some(chaat::time::now() + 3_600_000);
            h.app
                .settings
                .notify
                .follow_available
                .insert("u-ada".into(), "Ada Lovelace".into());
            h.frames(3);
            let painted = painted_texts(&h);
            assert!(
                painted.iter().any(|t| t.starts_with("Paused until")),
                "paused label at {width}: {painted:?}"
            );
            assert!(
                painted.iter().any(|t| t == "Ada Lovelace"),
                "followed person at {width}"
            );
        }
    }
}

#[test]
fn chat_menu_has_mute() {
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        for width in CHAT_ACTION_WIDTHS {
            let mut h = themed(&theme, width);
            let row = leftmost_text(&h, "Katherine Johnson").expect("the row is listed");
            right_click(&mut h, row);
            assert!(
                has_text(&h, "Mute notifications"),
                "the menu offers Mute ({theme:?}, {width})"
            );
        }
    }
}

#[test]
fn account_menu_has_pause() {
    for width in [480.0, 720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme;
            h.app.open_account_menu = true;
            h.frames(3);
            assert!(
                painted_texts(&h)
                    .iter()
                    .any(|t| t == "Pause notifications for 1 hour"),
                "the account menu offers Pause at {width}"
            );
        }
    }
}

// -- Agents ------------------------------------------------------------------

/// A narrow window shows the chat alone, docked beside a call.
fn agents_harness(width: f32, theme: ThemeChoice) -> Harness {
    let mut h = Harness::new();
    h.size = egui::vec2(width, 900.0);
    h.app.settings.theme = theme;
    chaat::demo::populate_agents(&mut h.app);
    if width < 500.0 {
        h.app.pretend_docked();
    }
    h.frames(4);
    h
}

/// One full-window frame whose actions are read before the app applies them.
fn agents_frame(h: &mut Harness, events: Vec<egui::Event>) -> Vec<Action> {
    h.time += 1.0 / 60.0;
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, h.size)),
        time: Some(h.time),
        events,
        ..Default::default()
    };
    let app = &mut h.app;
    let mut output = h.ctx.run_ui(input, |ui| {
        chaat::ui::show(app, ui);
    });
    output.textures_delta.clear();
    h.shapes = output.shapes;
    std::mem::take(&mut h.app.actions)
}

fn agents_click(h: &mut Harness, wanted: &str) -> Vec<Action> {
    let pos = ring_text_pos(h, wanted).unwrap_or_else(|| panic!("{wanted:?} is painted"));
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let mut actions = agents_frame(h, vec![egui::Event::PointerMoved(pos)]);
    actions.extend(agents_frame(h, vec![button(true)]));
    actions.extend(agents_frame(h, vec![button(false)]));
    actions.extend(agents_frame(h, vec![]));
    actions
}

fn agents_chat() -> ConversationId {
    ConversationId::Chat(chaat::demo::AGENTS_CHAT.into())
}

#[test]
fn card_both_themes_and_widths() {
    for width in [360.0, 1400.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let h = agents_harness(width, theme.clone());
            let painted = painted_texts(&h);
            for text in [
                "Claude Code proposes a reply \u{B7} 2 min ago",
                "Thanks Ada. I will check how Spotifast phrases its notes and send you a draft this afternoon.",
                "Send",
                "Edit",
                "Claude Code \u{1F44D}",
                "Accept",
            ] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} shows at {width} in {theme:?}: {painted:?}"
                );
            }
            assert!(painted.iter().filter(|t| *t == "Discard").count() == 2);
            assert!(
                painted
                    .iter()
                    .any(|t| t.starts_with("Ada Lovelace: Morning!")),
                "the quoted message shows"
            );
        }
    }
}

#[test]
fn the_chat_list_marks_a_chat_with_a_proposal() {
    let mut h = agents_harness(1400.0, ThemeChoice::Dark);
    let rings = |h: &Harness| {
        painted_shapes(h)
            .iter()
            .filter(|s| {
                matches!(s, egui::epaint::Shape::Circle(c)
                    if c.fill == egui::Color32::TRANSPARENT && c.stroke.width == 1.5)
            })
            .count()
    };
    assert_eq!(rings(&h), 1);
    let account = h.app.id.clone().unwrap();
    for id in ["p1", "p2"] {
        h.app.agent_proposals.take(id);
    }
    assert!(!h.app.agent_proposals.has_pending(&account, &agents_chat()));
    h.frames(2);
    assert_eq!(rings(&h), 0);
}

#[test]
fn outdated_hides_send() {
    let mut h = agents_harness(1400.0, ThemeChoice::Dark);
    let account = h.app.id.clone().unwrap();
    h.app.agent_proposals.outdate(&account, &agents_chat());
    h.frames(3);
    let painted = painted_texts(&h);
    assert!(
        painted
            .iter()
            .any(|t| t == "Claude Code\u{2019}s reply is outdated: you answered since"),
        "{painted:?}"
    );
    assert!(!painted.iter().any(|t| t == "Send"));
    assert!(painted.iter().any(|t| t == "Edit"));
}

#[test]
fn agent_name_is_plain_text() {
    let mut h = agents_harness(1400.0, ThemeChoice::Light);
    let account = h.app.id.clone().unwrap();
    h.app.agent_proposals.reply(
        "demo-agents",
        &account,
        agents_chat(),
        "**bold** <b>x</b> [link](https://example.com)".into(),
        None,
        "**Bot**".into(),
        chaat::time::now(),
    );
    h.frames(3);
    let painted = painted_texts(&h);
    assert!(
        painted
            .iter()
            .any(|t| t.starts_with("**Bot** proposes a reply")),
        "{painted:?}"
    );
    assert!(
        painted
            .iter()
            .any(|t| t == "**bold** <b>x</b> [link](https://example.com)")
    );
}

#[test]
fn send_button_pushes_agent_send() {
    let mut h = agents_harness(1400.0, ThemeChoice::Dark);
    let actions = agents_click(&mut h, "Send");
    assert!(
        actions.contains(&Action::Agent(AgentAction::Send {
            proposal: "p1".into()
        })),
        "{actions:?}"
    );
    let mut h = agents_harness(1400.0, ThemeChoice::Dark);
    let actions = agents_click(&mut h, "Edit");
    assert!(actions.contains(&Action::Agent(AgentAction::Edit {
        proposal: "p1".into()
    })));
    let mut h = agents_harness(1400.0, ThemeChoice::Dark);
    let actions = agents_click(&mut h, "Accept");
    assert!(
        actions.contains(&Action::Agent(AgentAction::AcceptReaction {
            proposal: "p2".into()
        }))
    );
}

#[test]
fn section_only_for_agents_plugin() {
    for width in [480.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            chaat::demo::populate_agents(&mut h.app);
            h.size = egui::vec2(width, 3000.0);
            h.app.settings.theme = theme.clone();
            h.act(Action::ShowSettingsPage(SettingsPage::Plugins));
            h.app.plugins_view = PluginsView::Plugin("demo-agents".into());
            h.frames(3);
            let painted = painted_texts(&h);
            for text in [
                "WHAT AGENTS CAN SEE",
                "Chats",
                "Channels",
                "Selected",
                "Agents may propose replies and reactions",
            ] {
                assert!(
                    painted.iter().any(|t| t == text),
                    "{text:?} shows at {width} in {theme:?}: {painted:?}"
                );
            }
            // One block per signed-in account.
            assert_eq!(painted.iter().filter(|t| *t == "Chats").count(), 2);
            h.app.plugins_view = PluginsView::Plugin("demo-light".into());
            h.frames(3);
            assert!(!painted_texts(&h).iter().any(|t| t == "WHAT AGENTS CAN SEE"));
        }
    }
}

#[test]
fn everything_starts_closed() {
    use chaat::agents::access::Access;
    let h = agents_harness(1280.0, ThemeChoice::Dark);
    for account in h.app.plugin_accounts() {
        let access = h.app.agent_access_of(&account.id);
        assert_eq!(access.chats, Access::None);
        assert_eq!(access.channels, Access::None);
        assert!(!access.propose);
    }
}

#[test]
fn access_section_pushes_changes() {
    use chaat::agents::access::{Access, AgentAccess};
    let mut h = Harness::new();
    chaat::demo::populate_agents(&mut h.app);
    h.size = egui::vec2(900.0, 3000.0);
    h.app.plugins_view = PluginsView::Plugin("demo-agents".into());
    let account = h.app.plugin_accounts()[0].id.clone();
    h.app.settings.agent_access.insert(
        account.clone(),
        AgentAccess {
            chats: Access::Selected([agents_chat().key()].into()),
            ..Default::default()
        },
    );
    plugins_page_frame(&mut h, vec![]);
    plugins_page_frame(&mut h, vec![]);
    assert!(painted_texts(&h).iter().any(|t| t == "Ada Lovelace"));
    let actions = plugins_page_click(&mut h, "All");
    assert!(
        actions.iter().any(
            |a| matches!(a, Action::Agent(AgentAction::SetAccess { account: id, access })
            if *id == account && access.chats == Access::All)
        ),
        "{actions:?}"
    );
}

#[test]
fn header_menu_toggles() {
    use chaat::agents::access::Access;
    for kind in ["none", "selected", "all"] {
        let mut h = agents_harness(1280.0, ThemeChoice::Dark);
        let account = h.app.id.clone().unwrap();
        let mut access = h.app.agent_access_of(&account);
        match kind {
            "selected" => access.chats = Access::Selected([agents_chat().key()].into()),
            "all" => access.chats = Access::All,
            _ => {}
        }
        h.app.settings.agent_access.insert(account, access);
        egui::Popup::toggle_id(&h.ctx, chaat::ui::agents::menu_id(&agents_chat()));
        h.frames(3);
        let painted = painted_texts(&h);
        let has = |text: &str| painted.iter().any(|t| t == text);
        match kind {
            "none" => {
                assert!(has("Let agents read this chat"), "{painted:?}");
                let actions = agents_click(&mut h, "Let agents read this chat");
                assert!(actions.contains(&Action::Agent(AgentAction::LetRead {
                    conversation: agents_chat(),
                    on: true
                })));
            }
            "selected" => {
                assert!(has("Stop agents reading this chat"), "{painted:?}");
                assert!(!has("Let agents read this chat"));
                let actions = agents_click(&mut h, "Stop agents reading this chat");
                assert!(actions.contains(&Action::Agent(AgentAction::LetRead {
                    conversation: agents_chat(),
                    on: false
                })));
            }
            _ => {
                assert!(!has("Let agents read this chat"));
                assert!(!has("Stop agents reading this chat"));
                assert!(has("Agents may read all of these"));
            }
        }
    }
    // No agents plugin on: no menu at all.
    let mut h = Harness::new();
    egui::Popup::toggle_id(&h.ctx, chaat::ui::agents::menu_id(&agents_chat()));
    h.frames(3);
    assert!(!painted_texts(&h).iter().any(|t| t.contains("agents")));
}

#[test]
fn messages_need_has_consent_words() {
    let mut h = Harness::new();
    chaat::demo::populate_agents(&mut h.app);
    h.size = egui::vec2(900.0, 3000.0);
    h.app
        .settings
        .plugins
        .last_mut()
        .unwrap()
        .approved_needs
        .clear();
    h.app.plugins.get_mut("demo-agents").unwrap().pending_needs = vec!["messages"];
    h.app.plugins_view = PluginsView::Plugin("demo-agents".into());
    plugins_page_frame(&mut h, vec![]);
    plugins_page_frame(&mut h, vec![]);
    let painted = painted_texts(&h);
    assert!(
        painted
            .iter()
            .any(|t| t == "the text of messages in the chats and channels you allow")
    );
    assert!(!painted.iter().any(|t| t.contains("does not know")));
    h.app.plugins_view = PluginsView::List;
    plugins_page_frame(&mut h, vec![]);
    plugins_page_frame(&mut h, vec![]);
    assert!(painted_texts(&h).iter().any(|t| t == "Asks for messages"));
}

// ---- Account rail and Unified Chats ----

/// The center of a painted text equal to `wanted` that sits inside the rail.
fn rail_text(h: &Harness, wanted: &str) -> Option<egui::Pos2> {
    find_text(h, wanted)
        .into_iter()
        .filter(|t| t.pos.x < chaat::theme::RAIL_WIDTH)
        .map(|text| text.pos + text.galley.rect.center().to_vec2())
        .next()
}

fn account_chat_title(h: &Harness, account: usize) -> String {
    let acct = &h.app.accounts[account];
    acct.chats[0].title(acct.me())
}

/// The demo accounts' tile text ("Co", "Fa"): `account_look::initials`.
fn tile_initials(account: usize) -> String {
    chaat::account_look::initials(["Contoso", "Fabrikam"][account])
}

/// The 40 px `+` tile: a transparent rounded square in the rail.
fn has_add_tile(h: &Harness) -> bool {
    painted_shapes(h).iter().any(|shape| match shape {
        egui::epaint::Shape::Rect(r) => {
            r.fill == egui::Color32::TRANSPARENT
                && (r.rect.width() - 40.0).abs() < 0.5
                && (r.rect.height() - 40.0).abs() < 0.5
                && r.rect.right() < chaat::theme::RAIL_WIDTH
        }
        _ => false,
    })
}

#[test]
fn rail_shows_tiles_and_sections() {
    let mut h = Harness::new();
    h.frames(3);
    assert!(h.app.unified_view());
    for account in 0..2 {
        let initials = tile_initials(account);
        assert!(rail_text(&h, &initials).is_some(), "tile {initials}");
    }
    assert!(has_add_tile(&h), "the + tile is drawn");
    assert!(
        painted_texts(&h).contains(&"All accounts".to_owned()),
        "the Unified sidebar header"
    );
}

#[test]
fn selected_tile_ring_is_not_clipped() {
    let mut h = Harness::new();
    h.frames(3);
    h.act(Action::ShowAccount(0));
    h.frames(3);
    let mut rings = 0;
    for clipped in &h.shapes {
        let mut stack = vec![&clipped.shape];
        while let Some(shape) = stack.pop() {
            match shape {
                egui::epaint::Shape::Vec(inner) => stack.extend(inner),
                egui::epaint::Shape::Rect(r)
                    if r.stroke.width == 2.0
                        && r.stroke_kind == egui::StrokeKind::Outside
                        && r.rect.right() < chaat::theme::RAIL_WIDTH =>
                {
                    rings += 1;
                    let visual = shape.visual_bounding_rect();
                    assert!(
                        clipped.clip_rect.contains_rect(visual),
                        "ring {visual:?} is cut by clip {:?}",
                        clipped.clip_rect
                    );
                }
                _ => {}
            }
        }
    }
    assert!(rings > 0, "the selection ring is painted");
}

/// Where the rail's first item starts: macOS keeps the header's height free
/// for the window buttons (`ui::rail::show`).
fn rail_top() -> f32 {
    if cfg!(target_os = "macos") {
        chaat::theme::HEADER_HEIGHT + 4.0
    } else {
        8.0
    }
}

#[test]
fn logo_and_tiles_switch_scope() {
    let mut h = Harness::new();
    h.frames(3);
    assert!(h.app.unified);
    // A tile click shows that account alone.
    let initials = tile_initials(1);
    let tile = rail_text(&h, &initials).expect("tile 1");
    h.click(tile);
    assert!(!h.app.unified);
    assert_eq!(h.app.active, 1);
    // The logo (first item of the rail) goes back to all accounts.
    h.click(egui::pos2(
        chaat::theme::RAIL_WIDTH / 2.0,
        rail_top() + 22.0,
    ));
    assert!(h.app.unified, "the logo shows all accounts");
    h.act(Action::ShowAccount(0));
    assert!(!h.app.unified);
    h.act(Action::ShowUnified);
    assert!(h.app.unified);
}

#[test]
fn unified_chats_list_both_accounts() {
    let mut h = Harness::new();
    h.frames(3);
    let contoso = account_chat_title(&h, 0);
    let fabrikam = account_chat_title(&h, 1);
    let texts = painted_texts(&h);
    assert!(texts.contains(&contoso), "{contoso} painted");
    assert!(texts.contains(&fabrikam), "{fabrikam} painted");
}

#[test]
fn open_other_account_chat_stays_unified() {
    let mut h = Harness::new();
    h.frames(3);
    let title = account_chat_title(&h, 1);
    let id = h.app.accounts[1].chats[0].id.clone();
    let at = leftmost_text(&h, &title).expect("row painted");
    h.click(at);
    assert!(h.app.unified, "opening keeps the unified view");
    assert_eq!(h.app.active, 1);
    assert_eq!(h.app.open, Some(ConversationId::Chat(id)));
}

#[test]
fn tile_right_click_opens_editor() {
    let mut h = Harness::new();
    h.frames(3);
    let initials = tile_initials(1);
    let tile = rail_text(&h, &initials).expect("tile 1");
    right_click(&mut h, tile);
    let texts = painted_texts(&h);
    assert!(texts.contains(&"Name".to_owned()), "editor Name label");
    assert!(texts.contains(&"Color".to_owned()), "editor Color label");
}

#[test]
fn menu_has_no_account_rows() {
    let mut h = Harness::new();
    h.frames(3);
    h.app.open_account_menu = true;
    h.frames(3);
    let texts = painted_texts(&h);
    assert!(texts.contains(&"Available".to_owned()), "the menu is open");
    assert!(!texts.iter().any(|t| t.contains("Add account")));
    let other = h.app.accounts[1].username().to_owned();
    let hits: Vec<&String> = texts.iter().filter(|t| t.as_str() == other).collect();
    assert!(hits.is_empty(), "second account listed: {hits:?}");
}

#[test]
fn unified_rail_draws_in_both_themes_and_widths() {
    for width in [480.0, 720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = themed(&theme, width);
            assert!(h.app.unified_view());
            assert!(h.frames(2) > 50, "draws ({theme:?}, {width})");
            for section in [Section::Teams, Section::Calendar, Section::Calls] {
                h.act(Action::Navigate(section));
                assert!(h.frames(2) > 50, "{section:?} ({theme:?}, {width})");
            }
            h.act(Action::Navigate(Section::Chats));
            assert!(has_add_tile(&h), "+ tile ({theme:?}, {width})");
        }
    }
}

#[test]
fn single_account_shows_no_tiles() {
    let mut h = Harness::new();
    h.frames(2);
    let initials: Vec<String> = (0..2).map(tile_initials).collect();
    h.app.accounts.truncate(1);
    h.app.active = 0;
    h.frames(3);
    assert!(!h.app.unified_view());
    for initials in &initials {
        assert!(rail_text(&h, initials).is_none(), "no tile {initials}");
    }
    assert!(!painted_texts(&h).contains(&"All accounts".to_owned()));
    assert!(has_add_tile(&h), "the + tile stays");
}

// ---- Unified sections, settings, availability, shared chats ----

/// Painted texts in the sidebar column (right of the rail, left of the
/// content), whole-text equal to `wanted`.
fn sidebar_texts(h: &Harness, wanted: &str) -> usize {
    let right = chaat::theme::RAIL_WIDTH + h.app.settings.sidebar_width;
    find_text(h, wanted)
        .into_iter()
        .filter(|t| t.pos.x > chaat::theme::RAIL_WIDTH && t.pos.x < right)
        .count()
}

fn painted_count(h: &Harness, wanted: &str) -> usize {
    find_text(h, wanted).len()
}

#[test]
fn unified_only_hides_tiles() {
    let mut h = Harness::new();
    h.frames(3);
    assert!(rail_text(&h, &tile_initials(1)).is_some(), "tiles at first");
    h.act(Action::SetUnifiedOnly(true));
    assert!(h.app.unified_view());
    for account in 0..2 {
        assert!(
            rail_text(&h, &tile_initials(account)).is_none(),
            "no tile {account}"
        );
    }
    assert!(!has_add_tile(&h), "no + tile");
    assert!(
        painted_texts(&h).contains(&"All accounts".to_owned()),
        "still unified"
    );
}

#[test]
fn unified_only_leaves_no_gap_under_the_logo() {
    let mut h = Harness::new();
    h.act(Action::SetUnifiedOnly(true));
    h.frames(2);
    let separator = painted_shapes(&h)
        .into_iter()
        .find_map(|shape| match shape {
            egui::epaint::Shape::LineSegment { points, .. }
                if points[0].y == points[1].y
                    && points[1].x <= chaat::theme::RAIL_WIDTH
                    && points[0].y > 0.0 =>
            {
                Some(points[0].y)
            }
            _ => None,
        })
        .expect("rail separator");
    // Logo: the rail's top padding + 44 px tile, then one 12 px item gap.
    assert!(
        separator < rail_top() + 44.0 + 12.0 + 8.0,
        "separator at {separator}"
    );
}

/// Unified only: the logo has no tile, so the rule under it sits halfway
/// between the logo's mark and the Chats tile.
#[test]
fn unified_only_centres_the_rule_between_logo_and_chats() {
    let mut h = Harness::new();
    h.act(Action::SetUnifiedOnly(true));
    h.frames(2);
    let shapes = painted_shapes(&h);
    let separator = shapes
        .iter()
        .find_map(|shape| match shape {
            egui::epaint::Shape::LineSegment { points, .. }
                if points[0].y == points[1].y
                    && points[1].x <= chaat::theme::RAIL_WIDTH
                    && points[0].y > 0.0 =>
            {
                Some(points[0].y)
            }
            _ => None,
        })
        .expect("rail separator");
    let in_rail: Vec<egui::Rect> = shapes
        .iter()
        .map(|shape| shape.visual_bounding_rect())
        .filter(|r| r.is_finite() && r.max.x <= chaat::theme::RAIL_WIDTH && r.width() > 4.0)
        .collect();
    let logo_bottom = in_rail
        .iter()
        .filter(|r| r.max.y < separator)
        .map(|r| r.max.y)
        .fold(f32::MIN, f32::max);
    let chats_top = in_rail
        .iter()
        .filter(|r| r.min.y > separator)
        .map(|r| r.min.y)
        .fold(f32::MAX, f32::min);
    let above = separator - logo_bottom;
    let below = chats_top - separator;
    assert!(
        (above - below).abs() <= 1.5,
        "gap above the rule {above}, below {below}"
    );
}

#[test]
fn unified_only_settings_still_add_account() {
    let mut h = Harness::new();
    h.app.demo = false;
    h.act(Action::SetUnifiedOnly(true));
    h.act(Action::OpenSettings);
    h.frames(2);
    assert!(
        painted_texts(&h).contains(&"Add account".to_owned()),
        "Settings offers Add account"
    );
}

#[test]
fn availability_applies_to_all_accounts() {
    let mut h = Harness::new();
    h.frames(3);
    assert!(h.app.unified_view());
    h.act(Action::SetAvailability(Some(Availability::Busy)));
    for account in 0..2 {
        let now = h.app.accounts[account]
            .my_presence
            .as_ref()
            .map(|p| p.availability);
        assert_eq!(now, Some(Availability::Busy), "account {account}");
    }
}

#[test]
fn settings_account_page_edits_each_account() {
    let mut h = Harness::new();
    h.act(Action::OpenSettings);
    assert!(
        painted_texts(&h)
            .iter()
            .any(|t| t.eq_ignore_ascii_case("Accounts"))
    );
    assert!(painted_texts(&h).contains(&"Contoso".to_owned()));
    assert!(painted_texts(&h).contains(&"Fabrikam".to_owned()));
    assert_eq!(painted_count(&h, "Customize"), 2, "one editor per account");
    h.act(Action::SetAccountLook(
        1,
        chaat::settings::AccountLook {
            name: Some("Advisory".into()),
            ..Default::default()
        },
    ));
    let texts = painted_texts(&h);
    assert!(texts.contains(&"Advisory".to_owned()), "renamed");
    assert!(!texts.contains(&"Fabrikam".to_owned()));
}

#[test]
fn unified_teams_group_by_account() {
    let mut h = Harness::new();
    h.app.accounts[1].teams = vec![chaat::model::Team {
        id: "t-fab".into(),
        name: "Fabrikam Ops".into(),
        description: None,
        web_url: None,
    }];
    h.act(Action::Navigate(Section::Teams));
    let texts = painted_texts(&h);
    assert!(texts.contains(&"Fabrikam Ops".to_owned()), "second team");
    assert!(texts.contains(&"Fabrikam".to_owned()), "account header");
    // The first account's header and its "Contoso" team.
    assert!(painted_count(&h, "Contoso") >= 2, "header and team");
}

#[test]
fn unified_calendar_merges_accounts() {
    let mut h = Harness::new();
    let now = chaat::time::now();
    let shared = h.app.accounts[0]
        .meetings
        .iter()
        .find(|m| m.end > now && !m.cancelled)
        .expect("an upcoming demo meeting")
        .clone();
    let mut copy = shared.clone();
    copy.id = "other-mailbox-id".into();
    let unique = chaat::model::Meeting {
        id: "fab-1".into(),
        subject: "Fabrikam board prep".into(),
        start: now + 60 * 60_000,
        end: now + 90 * 60_000,
        ..Default::default()
    };
    h.app.accounts[1].meetings = vec![copy, unique];
    h.act(Action::Navigate(Section::Calendar));
    assert_eq!(
        sidebar_texts(&h, &shared.subject),
        1,
        "the shared meeting shows once"
    );
    assert_eq!(sidebar_texts(&h, "Fabrikam board prep"), 1);
}

#[test]
fn unified_calls_list_people_from_both() {
    let mut h = Harness::new();
    let one = |h: &Harness, account: usize| {
        let acct = &h.app.accounts[account];
        let me = acct.me();
        acct.chats
            .iter()
            .filter(|c| c.kind == chaat::model::ChatKind::OneOnOne)
            .find_map(|c| c.peer(me).map(|m| m.display_name.clone()))
            .expect("a one-on-one peer")
    };
    let (a, b) = (one(&h, 0), one(&h, 1));
    h.act(Action::Navigate(Section::Calls));
    let texts = painted_texts(&h);
    assert!(texts.contains(&a), "{a} listed");
    assert!(texts.contains(&b), "{b} listed");
}

/// Copies account 1's first chat into account 0, so both hold it.
fn share_first_chat(h: &mut Harness) -> (String, String) {
    let copy = h.app.accounts[1].chats[0].clone();
    let id = copy.id.clone();
    let me = h.app.accounts[1].me().to_owned();
    let title = copy.title(&me);
    h.app.accounts[0].chats.push(copy);
    (id, title)
}

#[test]
fn shared_chat_shows_once() {
    let mut h = Harness::new();
    h.frames(3);
    let (id, title) = {
        let acct = &h.app.accounts[1];
        (acct.chats[0].id.clone(), acct.chats[0].title(acct.me()))
    };
    assert_eq!(sidebar_texts(&h, &title), 1);
    share_first_chat(&mut h);
    h.frames(3);
    assert_eq!(h.app.chat_accounts(&id), vec![0, 1]);
    let copy = h.app.accounts[0].chats.last().unwrap();
    let as_first = copy.title(h.app.accounts[0].me());
    let rows = sidebar_texts(&h, &title)
        + if as_first == title {
            0
        } else {
            sidebar_texts(&h, &as_first)
        };
    assert_eq!(rows, 1, "merged into one row");
}

#[test]
fn sending_as_switches_account() {
    let mut h = Harness::new();
    h.frames(3);
    let (id, _) = share_first_chat(&mut h);
    h.act(Action::OpenSharedChat(1, id.clone()));
    assert_eq!(h.app.active, 1);
    assert!(h.app.unified);
    h.frames(3);
    let label = "Sending as Fabrikam".to_owned();
    let at = leftmost_text(&h, &label).expect("the sending-as control");
    h.click(at);
    let item = find_text(&h, "Contoso")
        .last()
        .map(|t| t.pos + t.galley.rect.center().to_vec2())
        .expect("the other account in the menu");
    h.click(item);
    assert_eq!(h.app.active, 0, "now sending as the first account");
    assert_eq!(h.app.open, Some(ConversationId::Chat(id)));
    assert!(h.app.unified, "still unified");
}

#[test]
fn switcher_lists_both_accounts() {
    let mut h = Harness::new();
    h.frames(3);
    h.act(Action::OpenSwitcher);
    h.app.switcher.query = "mae".into();
    h.frames(3);
    assert!(
        painted_texts(&h).contains(&"Mae Jemison".to_owned()),
        "the second account's chat"
    );
    h.app.switcher.query = "grace".into();
    h.frames(3);
    assert!(
        painted_texts(&h).iter().any(|t| t.contains("Grace")),
        "the first account's chat"
    );
}

#[test]
fn unified_sections_draw_in_both_themes_and_widths() {
    for width in [480.0, 720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = themed(&theme, width);
            h.app.accounts[1].teams = vec![chaat::model::Team {
                id: "t-fab".into(),
                name: "Fabrikam Ops".into(),
                description: None,
                web_url: None,
            }];
            for section in [Section::Teams, Section::Calendar, Section::Calls] {
                h.act(Action::Navigate(section));
                assert!(h.frames(2) > 50, "{section:?} ({theme:?}, {width})");
            }
            h.act(Action::OpenSettings);
            assert!(h.frames(2) > 50, "settings ({theme:?}, {width})");
            h.act(Action::CloseSettings);
            h.act(Action::OpenSwitcher);
            assert!(h.frames(2) > 50, "switcher ({theme:?}, {width})");
        }
    }
}

/// Clicks the account-picker button (shown as the active account's name),
/// then the menu entry for `name`.
fn pick_account(h: &mut Harness, from: &str, name: &str) {
    let button = leftmost_text(h, from).unwrap_or_else(|| panic!("picker shows {from}"));
    h.click(button);
    let entry = find_text(h, name)
        .into_iter()
        .map(|t| t.pos + t.galley.rect.center().to_vec2())
        .max_by(|a, b| a.y.total_cmp(&b.y))
        .unwrap_or_else(|| panic!("the menu lists {name}"));
    h.click(entry);
}

#[test]
fn new_chat_start_as_switches_account() {
    let mut h = Harness::new();
    h.frames(3);
    assert!(h.app.unified_view());
    h.act(Action::OpenNewChat);
    assert!(has_text(&h, "Start as"), "the picker shows in Unified");
    assert_eq!(h.app.active, 0);
    pick_account(&mut h, "Contoso", "Fabrikam");
    assert_eq!(h.app.active, 1, "the picked account is active");
    // Outside Unified there is no picker.
    h.act(Action::CloseOverlay);
    h.act(Action::ShowAccount(0));
    h.act(Action::OpenNewChat);
    assert!(!has_text(&h, "Start as"));
}

#[test]
fn join_by_link_join_as() {
    let mut h = Harness::new();
    h.act(Action::Navigate(Section::Calls));
    h.frames(3);
    assert!(has_text(&h, "Join as"), "the picker shows in Unified");
    assert_eq!(h.app.active, 0);
    pick_account(&mut h, "Contoso", "Fabrikam");
    assert_eq!(h.app.active, 1, "the picked account is active");
    h.act(Action::ShowAccount(1));
    assert!(!has_text(&h, "Join as"));
}

#[test]
fn expired_account_shows_warning_and_banner() {
    let mut h = Harness::new();
    h.frames(3);
    assert!(!has_text(&h, "Sign in again: Fabrikam"));
    h.app.accounts[1].auth = AuthStatus::Expired {
        username: "alex@fabrikam.com".into(),
        reason: "Sign in again to get new messages.".into(),
    };
    h.frames(3);
    let warning = h.app.palette.warning;
    let dot = painted_shapes(&h).iter().any(|shape| match shape {
        egui::epaint::Shape::Circle(c) => {
            c.fill == warning && c.center.x < chaat::theme::RAIL_WIDTH
        }
        _ => false,
    });
    assert!(dot, "the tile shows a warning dot");
    let banner =
        leftmost_text(&h, "Sign in again: Fabrikam").expect("the banner names the account");
    h.click(banner);
    assert!(!h.app.unified, "the click shows that account");
    assert_eq!(h.app.active, 1);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn window_title_shows_total_unread() {
    assert_eq!(chaat::ui::window_title(0), "Chaat");
    assert_eq!(chaat::ui::window_title(3), "(3) Chaat");
    let mut h = Harness::new();
    let expected = chaat::ui::window_title(h.app.accounts.iter().map(|a| a.unread_chats()).sum());
    let mut titles = Vec::new();
    for _ in 0..4 {
        h.frames(1);
        for command in &h.viewport_commands {
            if let egui::ViewportCommand::Title(title) = command {
                titles.push(title.clone());
            }
        }
    }
    assert_eq!(titles, [expected], "sent once, on the first frame only");
}

#[test]
fn toast_shows_account_mark() {
    let mut h = Harness::new();
    h.frames(3);
    let marks = |h: &Harness| painted_texts(h).iter().filter(|t| *t == "Fa").count();
    let before = marks(&h);
    h.app.toasts.push(chaat::model::Toast {
        id: 900,
        kind: chaat::model::ToastKind::Info,
        text: "Fabrikam: Sent".into(),
        action: None,
        account: Some(1),
    });
    h.frames(3);
    assert!(has_text(&h, "Fabrikam: Sent"));
    assert_eq!(marks(&h), before + 1, "the toast paints the account mark");
}

#[test]
fn shortcut_cycles_scopes() {
    let mods = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    let mut h = Harness::new();
    h.frames(3);
    assert!(h.app.unified_view());
    let mut seen = Vec::new();
    for _ in 0..3 {
        h.frame_with(vec![key_event(egui::Key::A, mods)]);
        h.frames(2);
        seen.push((h.app.unified_view(), h.app.active));
    }
    assert_eq!(seen[0], (false, 0), "Unified -> account 0");
    assert_eq!(seen[1], (false, 1), "account 0 -> account 1");
    assert!(seen[2].0, "account 1 -> Unified");

    h.app.settings.unified_only = true;
    h.frames(2);
    let before = (h.app.unified_view(), h.app.active);
    h.frame_with(vec![key_event(egui::Key::A, mods)]);
    h.frames(2);
    assert_eq!((h.app.unified_view(), h.app.active), before);
}

#[test]
fn other_account_rows_ask_photos_once() {
    let mut h = Harness::new();
    h.frames(3);
    assert!(h.app.unified_view());
    // Avatars never ask for photos in the demo; leave it, as other tests do.
    h.app.demo = false;
    // Draw frames without applying, so the asked actions stay visible.
    let mut asked: Vec<String> = Vec::new();
    for _ in 0..4 {
        h.time += 1.0 / 60.0;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, h.size)),
            time: Some(h.time),
            ..Default::default()
        };
        let app = &mut h.app;
        let _ = h.ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            app.background(&ctx);
            chaat::ui::show(app, ui);
        });
        for action in std::mem::take(&mut h.app.actions) {
            if let Action::ForAccount(1, inner) = action
                && let Action::LoadPhotos(users) = *inner
            {
                asked.extend(users);
            }
        }
    }
    let mut unique = asked.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        asked.len(),
        unique.len(),
        "a photo is asked once: {asked:?}"
    );
    assert!(!asked.is_empty(), "account 1 rows ask for missing photos");
}
