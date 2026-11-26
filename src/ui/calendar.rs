//! The next two weeks of meetings: an agenda in the sidebar, the selected
//! meeting on the right with its Join button.

use std::collections::HashMap;

use egui::{CornerRadius, Frame, Margin, Sense, Stroke, vec2};

use super::widgets;
use crate::app::App;
use crate::model::{Action, Meeting};
use crate::theme::{self, Icon};

/// One agenda entry: a meeting and the accounts that have it.
struct Entry {
    meeting: Meeting,
    /// `(account index, that account's meeting id)`, in account order.
    copies: Vec<(usize, String)>,
}

/// The same meeting: equal join links, or equal subject, start and end
/// (mailbox ids differ across accounts, and invites can arrive twice).
#[cfg(test)]
fn same_meeting(a: &Meeting, b: &Meeting) -> bool {
    if let (Some(x), Some(y)) = (&a.join_url, &b.join_url)
        && x == y
    {
        return true;
    }
    a.subject == b.subject && a.start == b.start && a.end == b.end
}

/// Upcoming meetings, merged over every account in Unified (one entry per
/// meeting, even when several accounts were invited) and ordered by start.
fn entries(app: &App, now: i64) -> Vec<Entry> {
    let indexes: Vec<usize> = if app.unified_view() {
        (0..app.accounts.len()).collect()
    } else {
        vec![app.active]
    };
    let mut out: Vec<Entry> = Vec::new();
    // First entry per join link and per (subject, start, end): the same
    // result as scanning `out` with `same_meeting`, without the quadratic cost.
    let mut by_url: HashMap<&str, usize> = HashMap::new();
    let mut by_key: HashMap<(&str, i64, i64), usize> = HashMap::new();
    for j in indexes {
        for m in app.accounts[j]
            .meetings
            .iter()
            .filter(|m| m.end > now && !m.cancelled)
        {
            let hit = [
                m.join_url.as_deref().and_then(|u| by_url.get(u)),
                by_key.get(&(m.subject.as_str(), m.start, m.end)),
            ]
            .into_iter()
            .flatten()
            .min()
            .copied();
            match hit {
                Some(i) => out[i].copies.push((j, m.id.clone())),
                None => {
                    let i = out.len();
                    if let Some(u) = m.join_url.as_deref() {
                        by_url.entry(u).or_insert(i);
                    }
                    by_key
                        .entry((m.subject.as_str(), m.start, m.end))
                        .or_insert(i);
                    out.push(Entry {
                        meeting: m.clone(),
                        copies: vec![(j, m.id.clone())],
                    });
                }
            }
        }
    }
    out.sort_by_key(|e| e.meeting.start);
    out
}

pub fn list(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let now = crate::time::now();
    let unified = app.unified_view();
    let active = app.active;
    let upcoming = entries(app, now);
    if upcoming.is_empty() {
        let (title, text) = if app.loading.contains(&crate::sync::Resource::Calendar) {
            ("Loading your calendar…", "")
        } else {
            ("Nothing scheduled", "No meetings in the next two weeks.")
        };
        widgets::empty_state(ui, &palette, Icon::Calendar, title, text);
        return;
    }
    let mut actions = Vec::new();
    let mut select = None;
    egui::ScrollArea::vertical()
        .id_salt("agenda")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut day = None;
            for entry in &upcoming {
                let meeting = &entry.meeting;
                let key = crate::time::day_key(meeting.start);
                if day != Some(key) {
                    day = Some(key);
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add_space(18.0);
                        widgets::section_label(
                            ui,
                            &palette,
                            &crate::time::day_label(meeting.start, now),
                        );
                    });
                    ui.add_space(4.0);
                }
                // The copy to act on: the active account's own when it has the
                // meeting (no switch needed), else the first copy.
                let (j, id) = entry
                    .copies
                    .iter()
                    .find(|(j, _)| *j == active)
                    .or(entry.copies.first())
                    .cloned()
                    .unwrap_or((active, meeting.id.clone()));
                let selected = j == active && app.selected_meeting.as_deref() == Some(id.as_str());
                let marks: Vec<usize> = if unified {
                    let mut v: Vec<usize> = entry.copies.iter().map(|(j, _)| *j).collect();
                    v.dedup();
                    v
                } else {
                    Vec::new()
                };
                let response = row(ui, app, meeting, &marks, selected, now);
                if response.clicked() {
                    if unified {
                        if j != active {
                            actions.push(Action::SwitchAccount(j));
                        }
                        actions.push(Action::SelectMeeting(id.clone()));
                    } else {
                        select = Some(id.clone());
                    }
                }
                if response.double_clicked()
                    && let Some(url) = &meeting.join_url
                {
                    if j != active {
                        actions.push(Action::SwitchAccount(j));
                    }
                    actions.push(Action::JoinMeeting(url.clone()));
                }
            }
            ui.add_space(12.0);
        });
    if let Some(id) = select {
        app.selected_meeting = Some(id);
    }
    if app.selected_meeting.is_none() {
        // Unified: pick through an action, from the active account's own
        // meetings so that no account switch is needed.
        let first = upcoming
            .iter()
            .find_map(|e| e.copies.iter().find(|(j, _)| *j == active))
            .map(|(_, id)| id.clone());
        if let Some(id) = first {
            if unified {
                actions.push(Action::SelectMeeting(id));
            } else {
                app.selected_meeting = Some(id);
            }
        }
    }
    app.actions.extend(actions);
}

fn row(
    ui: &mut egui::Ui,
    app: &App,
    meeting: &Meeting,
    marks: &[usize],
    selected: bool,
    now: i64,
) -> egui::Response {
    let palette = &app.palette;
    let (response, inner) = widgets::list_row(ui, palette, 54.0, selected, Sense::click());
    if ui.is_rect_visible(response.rect) {
        let (secondary, _) = palette.row_text(selected);
        let live = meeting.start <= now && meeting.end > now;
        let bar = egui::Rect::from_min_size(inner.left_top() + vec2(8.0, 9.0), vec2(3.0, 34.0));
        ui.painter().rect_filled(
            bar,
            CornerRadius::same(2),
            if live {
                palette.success
            } else if meeting.join_url.is_some() {
                palette.accent
            } else {
                palette.dim
            },
        );
        let left = bar.right() + 10.0;
        // Account marks on the right, one per account that has the meeting.
        let marks_width = marks.len() as f32 * 20.0;
        let mut x = inner.right() - 8.0 - marks_width;
        for &j in marks {
            let look = app.account_look(j);
            let rect = egui::Rect::from_min_size(
                egui::pos2(x, inner.top() + 9.0),
                egui::Vec2::splat(16.0),
            );
            widgets::paint_account_mark(ui, &look, rect);
            x += 20.0;
        }
        let text_right = inner.right() - marks_width;
        let title = super::widgets::elided(
            ui,
            &meeting.subject,
            theme::semibold(13.0),
            palette.text,
            text_right - left - 10.0,
        );
        ui.painter()
            .galley(egui::pos2(left, inner.top() + 8.0), title, palette.text);
        let mut detail = crate::time::span_label(meeting.start, meeting.end, meeting.all_day);
        if live {
            detail.push_str(" · now");
        } else if meeting.start > now && meeting.start - now < 60 * 60 * 1000 {
            detail.push_str(&format!(
                " · {}",
                crate::time::until_label(meeting.start, now)
            ));
        }
        if let Some(location) = &meeting.location {
            detail.push_str(&format!(" · {location}"));
        }
        let detail = super::widgets::elided(
            ui,
            &detail,
            theme::regular(12.0),
            secondary,
            inner.right() - left - 10.0,
        );
        ui.painter()
            .galley(egui::pos2(left, inner.top() + 28.0), detail, secondary);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn detail(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let header = egui::Rect::from_min_size(
        ui.max_rect().min,
        vec2(ui.max_rect().width(), theme::HEADER_HEIGHT),
    );
    widgets::window_drag(ui, header, "calendar-drag");
    let Some(meeting) = app
        .selected_meeting
        .as_ref()
        .and_then(|id| app.meetings.iter().find(|m| &m.id == id))
        .cloned()
    else {
        widgets::empty_state(
            ui,
            &palette,
            Icon::Calendar,
            "Your calendar",
            "Upcoming meetings from Outlook appear on the left. Join Teams meetings with one click.",
        );
        return;
    };
    let now = crate::time::now();
    ui.add_space(theme::HEADER_HEIGHT + 12.0);
    Frame::new().inner_margin(Margin::symmetric(32, 0)).show(ui, |ui| {
        ui.set_max_width(ui.available_width().min(640.0));
        ui.label(egui::RichText::new(&meeting.subject).font(theme::bold(24.0)).color(palette.text));
        ui.add_space(6.0);
        let when = format!(
            "{} · {}",
            crate::time::day_label(meeting.start, now),
            crate::time::span_label(meeting.start, meeting.end, meeting.all_day)
        );
        ui.label(egui::RichText::new(when).font(theme::medium(14.0)).color(palette.secondary));
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            if let Some(url) = &meeting.join_url {
                let join = widgets::pill(ui, &palette, Icon::Video, "Join", true);
                widgets::on_click(&join, &mut app.actions, Action::JoinMeeting(url.clone()));
                let copy = widgets::pill(ui, &palette, Icon::Copy, "Copy link", false);
                widgets::on_click(&copy, &mut app.actions, Action::CopyText(url.clone()));
            }
            if let Some(web) = &meeting.web_link {
                let open = widgets::pill(ui, &palette, Icon::ExternalLink, "Open in Outlook", false);
                widgets::on_click(&open, &mut app.actions, Action::OpenUrl(web.clone()));
            }
        });
        ui.add_space(20.0);
        Frame::new()
            .fill(palette.panel)
            .stroke(Stroke::new(1.0, palette.outline))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::Grid::new("meeting-facts").num_columns(2).spacing(vec2(18.0, 10.0)).show(ui, |ui| {
                    let fact = |ui: &mut egui::Ui, label: &str, value: &str| {
                        ui.label(egui::RichText::new(label).color(palette.dim));
                        ui.label(egui::RichText::new(value).color(palette.text));
                        ui.end_row();
                    };
                    if let Some(organizer) = &meeting.organizer {
                        fact(ui, "Organizer", organizer);
                    }
                    if let Some(location) = &meeting.location {
                        fact(ui, "Location", location);
                    }
                    fact(ui, "Online", if meeting.join_url.is_some() { "Microsoft Teams meeting" } else { "No" });
                    if meeting.start > now {
                        fact(ui, "Starts", &crate::time::until_label(meeting.start, now));
                    }
                });
                if let Some(preview) = &meeting.preview {
                    ui.add_space(12.0);
                    ui.label(egui::RichText::new(preview).color(palette.secondary));
                }
            });
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new("Joining opens the meeting in Teams on the web beside Chaat, with its chat here. Chaat reminds you five minutes before online meetings start.")
                .font(theme::regular(12.0))
                .color(palette.dim),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meeting(subject: &str, start: i64, end: i64, url: Option<&str>) -> Meeting {
        Meeting {
            subject: subject.into(),
            start,
            end,
            join_url: url.map(str::to_owned),
            ..Default::default()
        }
    }

    #[test]
    fn equal_join_urls_match() {
        let a = meeting("A", 1, 2, Some("u"));
        let b = meeting("B", 5, 6, Some("u"));
        assert!(same_meeting(&a, &b));
    }

    #[test]
    fn same_subject_start_end_match_despite_urls() {
        let a = meeting("A", 1, 2, Some("u1"));
        let b = meeting("A", 1, 2, Some("u2"));
        assert!(same_meeting(&a, &b));
    }

    #[test]
    fn different_end_is_not_same() {
        let a = meeting("A", 1, 2, None);
        let b = meeting("A", 1, 3, None);
        assert!(!same_meeting(&a, &b));
    }

    #[test]
    fn entries_group_like_same_meeting() {
        let (mut app, _d) = crate::app::tests::demo_app();
        let far = i64::MAX / 2;
        let mk = |id: &str, subject: &str, start: i64, url: Option<&str>| Meeting {
            id: id.into(),
            ..meeting(subject, start, start + 10, url)
        };
        app.accounts[0].meetings = vec![
            mk("a1", "A", far, Some("u")),
            mk("b1", "B", far + 100, None),
        ];
        app.accounts[1].meetings = vec![
            mk("a2", "Other", far + 500, Some("u")),
            mk("b2", "B", far + 100, None),
            mk("c2", "C", far + 200, None),
        ];
        app.unified = true;
        let out = entries(&app, 0);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].copies.len(), 2);
        assert_eq!(out[1].copies.len(), 2);
        assert_eq!(out[2].copies.len(), 1);
    }
}
