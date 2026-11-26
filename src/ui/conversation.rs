//! The open chat or channel: header, virtualized history, composer.
//!
//! The history lays out only the rows near the viewport. Every other row
//! takes the height it had when last measured (or an estimate), so a
//! conversation with thousands of messages costs what its visible rows cost.

mod composer;
mod header;
mod history;
mod message;
mod pins;
mod thread;

use egui::{Align, CornerRadius, Frame, Key, Layout, Margin, Modifiers, Sense, Stroke, vec2};

use super::{composer_tools, find, rich, widgets};
use crate::app::{App, Draft};
use crate::model::{
    Access, Action, AgentAction, AttachmentKind, ChatKind, ConversationId, Delivery, Message,
    QUICK_REACTION_NAMES, QUICK_REACTIONS,
};
use crate::theme::{self, Icon};

use composer::composer;
use header::header;
use history::history;
use pins::pin_banner;

/// Room for the avatar left of other people's messages.
const GUTTER: f32 = 44.0;
const AVATAR: f32 = 32.0;
/// Between a missed call's words and its Call back button.
const CALL_BACK_GAP: f32 = 10.0;
/// How long the pointer rests on a message before its hover bar shows.
const HOVER_DELAY: f64 = 0.3;
/// Height of the name and time line above a message group.
const HEADER: f32 = 19.0;
/// Space between a bubble's edge and its text.
const BUBBLE_PAD: Margin = Margin {
    left: 12,
    right: 12,
    top: 7,
    bottom: 8,
};

/// The widest a chat bubble gets, and the width its text wraps at.
fn bubble_widths(width: f32) -> (f32, f32) {
    let bubble = ((width - GUTTER - 8.0) * 0.85).clamp(120.0, 760.0);
    let text = bubble - f32::from(BUBBLE_PAD.left + BUBBLE_PAD.right);
    (bubble, text)
}

/// The messages that carry a recording's link, newest first.
fn recording_messages(messages: &[Message]) -> Vec<&Message> {
    let mut found: Vec<&Message> = messages.iter().filter(|m| m.recording.is_some()).collect();
    found.sort_by_key(|m| std::cmp::Reverse(m.created));
    found
}

/// The recordings in a conversation's messages, newest first.
pub fn recordings_in(messages: &[Message]) -> Vec<&crate::model::Recording> {
    recording_messages(messages)
        .into_iter()
        .filter_map(|m| m.recording.as_ref())
        .collect()
}

pub fn show(app: &mut App, ui: &mut egui::Ui, conversation: ConversationId) {
    header(app, ui, &conversation);
    find::show(app, ui, &conversation);
    pin_banner(app, ui, &conversation);
    composer(app, ui, &conversation);
    history(app, ui, &conversation);
}

pub use pins::pin_banner_bottom;
