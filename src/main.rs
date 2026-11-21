//! Desktop entry point.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod notices;

use std::time::Instant;

use chaat::app::App;
use chaat::backend::Waker;
use chaat::paths::AppDirs;
use chaat::settings::Settings;
use clap::Parser;

/// A fast, native client for Microsoft Teams.
#[derive(Debug, Parser)]
#[command(name = "chaat", version, about)]
struct Cli {
    /// Print the bundled license texts and third-party notices, then quit.
    #[arg(long)]
    licenses: bool,
    /// Log more.
    #[arg(short, long)]
    verbose: bool,
    /// Start in the tray without a window (when "keep running" is on).
    #[arg(long)]
    start_hidden: bool,
    /// Print the time to the first frame and quit.
    #[arg(long)]
    measure_startup: bool,
}

fn main() -> eframe::Result<()> {
    let started = Instant::now();
    let launch = fastframe_update::intercept(&chaat::updates::CONFIG);
    let cli = Cli::parse_from(&launch.arguments);
    if cli.licenses {
        print!("{}", notices::TEXT);
        return Ok(());
    }
    let waker = Waker::default();

    // One running copy; a second launch shows the first one's window.
    let (instance, requests) = if cli.measure_startup {
        (None, None)
    } else {
        match chaat::platform::single_instance(&waker) {
            Some((guard, requests)) => (guard, Some(requests)),
            None => {
                eprintln!("Chaat is already running; asked it to show its window.");
                return Ok(());
            }
        }
    };

    let dirs = AppDirs::discover();
    dirs.ensure()
        .map_err(|e| eframe::Error::AppCreation(e.into()))?;
    let filter = if cli.verbose {
        "info,chaat=debug"
    } else {
        "warn,chaat=info"
    };
    let logging = fastframe_log::Logging::new("chaat", env!("CARGO_PKG_VERSION"))
        .filter(filter)
        .panic_log(dirs.panic_log());
    dirs.keep_previous_log();
    logging
        .file(dirs.log_file())
        .init()
        .map_err(|e| eframe::Error::AppCreation(e.into()))?;

    let settings = Settings::load(&dirs.settings_file());
    // Windows and Linux: Chaat's own title bar unless the person turned it off.
    fastframe_titlebar::init();
    fastframe_titlebar::set_enabled(settings.custom_titlebar);
    let geometry = settings.window;
    let mut app = App::new(dirs.clone(), settings, waker.clone(), false);
    app.attach_service(chaat::backend::Backend::spawn(dirs.clone(), waker.clone()));
    app.onboarding_forced = chaat::app::onboarding_forced_by_env();
    if app.onboarding_forced {
        log::info!("welcome screen forced by {}", chaat::app::ONBOARDING_ENV);
    }
    app.update_receipt = launch.receipt;
    if let Some(error) = launch.error {
        app.toast(chaat::model::ToastKind::Error, error);
    }
    app.requests = requests;
    app.perf.started = Some(started);
    log::debug!(
        "startup: state ready at {} ms",
        started.elapsed().as_millis()
    );
    if app.settings.keep_running {
        app.tray = chaat::platform::tray(&waker);
    }
    let start_hidden = cli.start_hidden && app.tray.as_ref().is_some_and(|t| t.is_shown());

    let measure = cli.measure_startup;
    let persistence = Some(dirs.state.join("window.ron"));

    fastframe_shell::Shell::new(app, &waker)
        .start_hidden(start_hidden)
        .idle(fastframe_tray::idle)
        .run(|lease| {
            let mut viewport = egui::ViewportBuilder::default()
                .with_title("Chaat")
                .with_app_id("chaat")
                .with_inner_size(geometry.map_or([1240.0, 800.0], |g| [g.width, g.height]))
                .with_min_inner_size([720.0, 480.0])
                .with_icon(chaat::platform::window_icon())
                // macOS: Chaat draws its own title bar under the traffic lights.
                .with_fullsize_content_view(true)
                .with_decorations(!fastframe_titlebar::enabled())
                .with_titlebar_shown(false)
                .with_title_shown(false);
            if let Some(g) = geometry
                && let (Some(x), Some(y)) = (g.x, g.y)
            {
                viewport = viewport.with_position([x, y]);
            }
            let options = eframe::NativeOptions {
                viewport,
                persist_window: true,
                persistence_path: persistence.clone(),
                ..Default::default()
            };
            eframe::run_native(
                "Chaat",
                options,
                Box::new(move |cc| {
                    log::debug!(
                        "startup: window ready at {} ms",
                        started.elapsed().as_millis()
                    );
                    chaat::ui::install(&cc.egui_ctx, false);
                    log::debug!(
                        "startup: fonts and loaders ready at {} ms",
                        started.elapsed().as_millis()
                    );
                    let mut app = lease.take(&cc.egui_ctx);
                    if let Some(tray) = &mut app.tray {
                        tray.attach();
                    }
                    app.window_hidden = false;
                    Ok(Box::new(Window {
                        pacing: chaat::pacing::FramePacer::new(cc),
                        app,
                        recovery_checked: false,
                        paste_input: chaat::clipboard::PasteInput::default(),
                        scrolling: fastframe_scroll::Scrolling::default(),
                        measure,
                    }))
                }),
            )
        })?;
    drop(instance);
    Ok(())
}

/// The eframe side of one window. The [`App`] goes back to the shell when
/// the window closes.
struct Window {
    pacing: chaat::pacing::FramePacer,
    app: fastframe_shell::Held<App>,
    recovery_checked: bool,
    paste_input: chaat::clipboard::PasteInput,
    scrolling: fastframe_scroll::Scrolling,
    measure: bool,
}

impl eframe::App for Window {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, input: &mut egui::RawInput) {
        self.paste_input.prepare(input);
    }
    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.pacing.begin_frame(frame);
        if !std::mem::replace(&mut self.recovery_checked, true) {
            fastframe_shell::window::recover_offscreen(ctx, frame);
        }
        let app = &mut *self.app;
        app.perf.record(frame.info().cpu_usage);
        app.observe_window(ctx);
        app.background(ctx);
        fastframe_titlebar::align_traffic_lights(frame, ctx, chaat::theme::HEADER_HEIGHT);
        if app.wants_show {
            app.wants_show = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if app.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.scrolling.apply(&ctx);
        let app = &mut *self.app;
        chaat::ui::show(app, ui);
        app.apply_actions(&ctx);
        if app.perf.first_frame.is_none()
            && let Some(started) = app.perf.started
        {
            let elapsed = started.elapsed();
            app.perf.first_frame = Some(elapsed);
            if let Some(receipt) = app.update_receipt.take() {
                app.service
                    .send(chaat::backend::Command::AcknowledgeUpdate(receipt));
            }
            log::info!("first frame after {} ms", elapsed.as_millis());
            log::debug!(
                "window geometry: {:?}",
                ctx.input(|i| i.viewport().outer_rect)
            );
            if self.measure {
                println!("first frame after {} ms", elapsed.as_millis());
                app.quit_requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        // Remember the window's size and place.
        let (size, position, maximized) = ctx.input(|i| {
            let viewport = i.viewport();
            (
                viewport.inner_rect.map(|r| r.size()),
                viewport.outer_rect.map(|r| r.min),
                viewport.maximized.unwrap_or(false),
            )
        });
        // Not while docked beside a call: the next start gets the full size.
        if let Some(size) = size
            && app.docked.is_none()
        {
            let geometry = chaat::settings::WindowGeometry {
                width: size.x,
                height: size.y,
                x: position.map(|p| p.x),
                y: position.map(|p| p.y),
                maximized,
            };
            if app.settings.window != Some(geometry) && !ctx.input(|i| i.pointer.any_down()) {
                app.settings.window = Some(geometry);
                app.mark_settings_dirty();
            }
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.app.save_settings();
    }
}
