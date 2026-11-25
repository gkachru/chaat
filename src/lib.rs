//! Chaat: a fast, native app that works with Microsoft Teams.
//!
//! See README.md for what works today and docs/architecture.md for how the
//! pieces fit: `ui` draws state and emits `model::Action`s, `app` applies
//! them, `backend` runs every network and disk task on its own runtime.

/// The app identifier: bundle id, Windows toast app id, single-instance slot
/// name and keyring service.
pub const APP_ID: &str = "app.chaat.Chaat";

pub mod account_look;
pub mod agents;
pub mod app;
pub mod auth;
pub mod backend;
pub mod calling;
pub mod cards;
pub mod clipboard;
pub mod compose_format;
pub mod compose_style;
pub mod demo;
pub mod emoji;
pub mod graph;
pub mod html;
pub mod images;
pub mod model;
pub mod notify;
pub mod outbox;
pub mod pacing;
pub mod paths;
pub mod platform;
pub mod plugin;
pub mod realtime;
pub mod relay_protocol;
pub mod secrets;
pub mod settings;
pub mod setup_file;
pub mod store;
pub mod sync;
pub mod theme;
pub mod time;
pub mod ui;
pub mod updates;
