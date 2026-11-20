//! Drives every screen of the demo through real egui frames, without a
//! window: layout must not panic, actions must apply, and each view must
//! draw something.

use chaat::app::{App, Overlay};
use chaat::backend::{AuthStatus, Waker};
use chaat::model::{Action, ConversationId, Section, SettingsPage};
use chaat::model::{Availability, MentionRef};
use chaat::paths::AppDirs;
use chaat::settings::{Settings, ThemeChoice};
use egui::load::BytesLoader as _;

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
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme;
            h.frames(3);
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
        painted_shapes(h)
            .iter()
            .find_map(|shape| match shape {
                egui::epaint::Shape::Text(text) if text.galley.job.text == value => {
                    Some((text.pos, text.galley.size().y))
                }
                _ => None,
            })
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
                        chaat::backend::Event::Pasted {
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
        chaat::backend::Event::Pasted {
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
                let mut h = Harness::new();
                h.size.x = width;
                h.app.settings.theme = theme.clone();
                h.frames(3);
                let chat = h.app.open.clone().unwrap();
                let url = "https://graph.microsoft.com/v1.0/chats/fixture/messages/1/hostedContents/1/$value";
                let mut bytes = std::io::Cursor::new(Vec::new());
                image::RgbImage::from_pixel(1200, 800, image::Rgb([0, 128, 255]))
                    .write_to(&mut bytes, format)
                    .unwrap();
                let bytes = bytes.into_inner();
                h.app.handle(
                    chaat::backend::Event::Media {
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
        chaat::backend::Event::Pasted {
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
                let mut h = Harness::new();
                h.size.x = width;
                h.app.settings.theme = theme.clone();
                h.frames(3);
                let chat = h.app.open.clone().unwrap();
                let image = chaat::clipboard::encode_rgba(
                    w,
                    height,
                    &[0, 128, 255, 255].repeat(w * height),
                )
                .unwrap();
                h.app.handle(
                    chaat::backend::Event::Pasted {
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
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme;
            h.frames(3);
            let chat = h.app.open.clone().unwrap();
            for (w, height) in [(1200, 24), (24, 1200), (256, 256)] {
                let image = chaat::clipboard::encode_rgba(
                    w,
                    height,
                    &[0, 128, 255, 255].repeat(w * height),
                )
                .unwrap();
                h.app.handle(
                    chaat::backend::Event::Pasted {
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
        chaat::backend::Event::Messages {
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

    fn painted_texts(h: &Harness) -> Vec<String> {
        painted_shapes(h)
            .iter()
            .filter_map(|shape| match shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect()
    }
    fn chip(h: &Harness, label: &str) -> egui::Pos2 {
        painted_shapes(h)
            .iter()
            .find_map(|shape| match shape {
                egui::epaint::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.pos + text.galley.size() / 2.0)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("the {label} chip is painted"))
    }

    for width in [720.0, 1280.0] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme.clone();
            h.frames(3);
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
                    .filter(|c| chaat::ui::chat_list::listed(&h.app, c, &me))
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
                if chaat::ui::chat_list::listed(&h.app, chat, &me) {
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
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme;
            h.frames(3);
            let rail_texts: Vec<String> = painted_shapes(&h)
                .iter()
                .filter_map(|shape| match shape {
                    egui::epaint::Shape::Text(text) if text.pos.x < 64.0 => {
                        Some(text.galley.job.text.clone())
                    }
                    _ => None,
                })
                .filter(|text| ["Chaat", "Teams", "Calendar", "Calls"].contains(&text.as_str()))
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
fn switcher_finds_people_and_messages() {
    let mut h = Harness::new();
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
    let bar = 6.0 * 32.0 + 9.0 + 2.0 * 28.0 + 6.0;
    assert!(bubble.width() > bar);
    h.click(egui::pos2(bubble.left() + bar - 17.0, bubble.top() - 16.0));
    let texts = painted_texts(&h);
    for item in ["Edit", "Delete", "Copy text"] {
        assert!(texts.iter().any(|t| t == item), "the menu offers {item}");
    }
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
    let mut h = Harness::new();
    h.act(Action::OpenConversation(ConversationId::Chat(
        "19:demo-ada@unq.gbl.spaces".into(),
    )));
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
        chaat::backend::Event::Pasted {
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
            let mut h = Harness::new();
            h.size.x = width;
            h.app.settings.theme = theme.clone();
            h.frames(2);
            h.act(Action::OpenConversation(ConversationId::Chat(
                "19:demo-grace@unq.gbl.spaces".into(),
            )));
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
            let row = painted_shapes(&h)
                .into_iter()
                .find_map(|shape| match shape {
                    egui::epaint::Shape::Text(t) if t.galley.job.text == label => {
                        Some(shape.visual_bounding_rect())
                    }
                    _ => None,
                })
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
            let text = painted_shapes(&h)
                .into_iter()
                .find_map(|shape| match shape {
                    egui::epaint::Shape::Text(t) if t.galley.job.text == label => {
                        Some(shape.visual_bounding_rect())
                    }
                    _ => None,
                })
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
    painted_shapes(h).into_iter().find_map(|shape| match shape {
        egui::epaint::Shape::Text(text) if text.galley.job.text == wanted => {
            Some(text.pos + text.galley.rect.center().to_vec2())
        }
        _ => None,
    })
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
            h.app.handle(
                chaat::backend::Event::IncomingCall(chaat::calling::RingAlert {
                    id: "call-1".into(),
                    caller: "Ada Lovelace".into(),
                    caller_mri: "8:orgid:u-ada".into(),
                    video: true,
                    group: false,
                    thread_id: None,
                    account: None,
                    username: None,
                }),
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
            h.frames(2);
            let ctx = h.ctx.clone();
            h.app.handle(
                chaat::backend::Event::CallAlertsSignIn {
                    account: None,
                    username: None,
                    verification_uri: "https://microsoft.com/devicelogin".into(),
                    user_code: "AB12CD34".into(),
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
            assert!(!h.app.call_alerts_sign_in.is_empty());
            h.act(Action::HideCallAlertsSignIn(None));
            assert!(!painted_texts(&h).iter().any(|t| t == "AB12CD34"));
        }
    }
}

#[test]
fn the_calls_settings_page_offers_the_call_alerts_plugin() {
    for width in [480.0, 1280.0] {
        let mut h = Harness::new();
        h.size.x = width;
        h.act(Action::ShowSettingsPage(SettingsPage::Calls));
        let painted = painted_texts(&h);
        for text in [
            "CALL ALERTS",
            "No plugin chosen",
            "Choose plugin…",
            "Show incoming calls",
        ] {
            assert!(
                painted.iter().any(|t| t == text),
                "{text:?} shows at {width}"
            );
        }
        let mut calls = h.app.settings.calls.clone();
        calls.call_alerts_plugin = Some("C:/plugins/call-alerts.exe".into());
        calls.call_alerts = true;
        h.act(Action::SetCallSettings(calls));
        let painted = painted_texts(&h);
        assert!(
            painted.iter().any(|t| t == "Remove"),
            "Remove shows at {width}"
        );
        assert!(painted.iter().any(|t| t == "The plugin isn't running."));
    }
}

#[test]
fn call_alerts_settings_list_each_account_and_whether_it_listens() {
    for width in [480.0, 1280.0] {
        let mut h = Harness::new();
        h.size.x = width;
        // Tall enough that the whole Calls page fits without scrolling.
        h.size.y = 1800.0;
        let mut calls = h.app.settings.calls.clone();
        calls.call_alerts_plugin = Some("C:/plugins/call-alerts.exe".into());
        calls.call_alerts = true;
        h.act(Action::SetCallSettings(calls));
        let ctx = h.ctx.clone();
        h.app.handle(
            chaat::backend::Event::CallAlertsStatus {
                running: true,
                message: None,
            },
            &ctx,
        );
        h.app.handle(
            chaat::backend::Event::CallAlertsListening {
                account: "u-alex2.fabrikam".into(),
            },
            &ctx,
        );
        h.act(Action::ShowSettingsPage(SettingsPage::Calls));
        let painted = painted_texts(&h);
        for text in ["alex@fabrikam.com", "Listening for calls", "Connecting…"] {
            assert!(
                painted.iter().any(|t| t == text),
                "{text:?} shows at {width}"
            );
        }
    }
}
