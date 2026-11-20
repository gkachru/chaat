//! External plugins: separate programs the person installs, which Chaat runs
//! and talks to over a small JSON protocol (AGENTS.md, "Plugins stay outside
//! Chaat"; docs/calling.md#call-alerts-plugin). `host` runs the process and
//! `protocol` defines the messages; what a plugin is for (today, call alerts)
//! lives with that feature, in `calling::alerts`.

pub mod event;
pub mod host;
pub mod options;
pub mod presence;
pub mod protocol;
pub mod registry;
