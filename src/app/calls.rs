//! Calls in Teams on the web: opening them, docking Chaat beside the call
//! window, and the call session behind the call bar (docs/calling.md).

use egui::{ViewportCommand, pos2, vec2};

use super::{App, Overlay};
use crate::backend::Command;
use crate::calling::browser::{self, Browser, BrowserProfile, Placement};
use crate::calling::dock;
use crate::calling::session::{CallSession, CallState, CallTarget, ChatLookup, in_call};
use crate::calling::{Opener, TEAMS_WEB, same_meeting};
use crate::model::{Action, Chat, ChatKind, ConversationId, Member, Ring, Section, ToastKind};

/// Chaat's window before it docked, to restore afterwards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Docked {
    position: Option<egui::Pos2>,
    inner: egui::Vec2,
    maximized: bool,
    /// Where the call window was asked to open.
    pub call: Placement,
}

/// The smallest window Chaat allows, except while docked.
const MIN_SIZE: egui::Vec2 = vec2(720.0, 480.0);

impl App {
    /// Edge and Chrome, found once.
    pub fn call_browsers(&mut self) -> &[Browser] {
        self.browsers.get_or_insert_with(browser::discover)
    }

    /// How a call opens now, from the setting and what is installed.
    pub fn opener(&mut self) -> Opener {
        let open_in = self.settings.calls.open_in;
        let preferred = self.settings.calls.browser;
        Opener::resolve(open_in, preferred, self.call_browsers())
    }

    /// The browser profile for an account, when each account has its own.
    fn browser_profile(&self, index: usize) -> Option<String> {
        self.settings
            .calls
            .profile_per_account
            .then(|| self.accounts.get(index).map(|a| a.browser_profile.clone()))
            .flatten()
    }

    /// Tells each account's backend where Microsoft's sign-in page opens: the
    /// call browser, so Teams on the web finds the same Microsoft session.
    pub(super) fn sync_call_browser(&mut self) {
        let opener = self.opener();
        for index in 0..self.accounts.len() {
            let target = opener.sign_in_browser().map(|browser| BrowserProfile {
                browser: browser.clone(),
                profile: self.browser_profile(index),
            });
            self.accounts[index]
                .backend
                .send(Command::SignInBrowser(target));
        }
    }

    /// Remembers which browser profile belongs to a signed-in account.
    pub(super) fn remember_browser_profile(&mut self) {
        let Some(id) = self.id.clone() else { return };
        let mine = self.browser_profile.clone();
        let profile = self
            .settings
            .calls
            .profiles
            .entry(id)
            .or_insert(mine)
            .clone();
        if profile != self.browser_profile {
            self.browser_profile = profile;
        }
        self.mark_settings_dirty();
        if self.settings.calls.profile_per_account {
            self.sync_call_browser();
        }
    }

    pub(super) fn join_meeting(&mut self, url: String, ctx: &egui::Context) {
        if !crate::calling::is_teams_link(&url) {
            self.toast(ToastKind::Error, "That is not a Microsoft Teams link.");
            return;
        }
        let subject = self
            .meetings
            .iter()
            .find(|m| m.join_url.as_deref().is_some_and(|j| same_meeting(j, &url)))
            .map(|m| m.subject.clone())
            .or_else(|| {
                let me = self.me().to_owned();
                self.chats
                    .iter()
                    .find(|c| c.join_url.as_deref().is_some_and(|j| same_meeting(j, &url)))
                    .map(|c| c.title(&me))
            })
            .unwrap_or_default();
        let target = CallTarget::Meeting {
            subject,
            join_url: url.clone(),
        };
        self.open_call(target, url, None, ctx);
    }

    pub(super) fn start_call(&mut self, users: Vec<String>, video: bool, ctx: &egui::Context) {
        // Started from a conversation: that is the call's chat. From the
        // Calls page: the one-on-one chat with that person, if any.
        let chat = if matches!(self.section, Section::Chats | Section::Teams) {
            self.open.clone()
        } else {
            self.one_on_one_with(&users)
        };
        self.call_people(users, video, chat, ctx);
    }

    /// Calls `users` in Teams, following `chat` as the call's chat.
    fn call_people(
        &mut self,
        users: Vec<String>,
        video: bool,
        chat: Option<ConversationId>,
        ctx: &egui::Context,
    ) {
        let Some(link) = crate::calling::call_link(&users, video) else {
            self.toast(
                ToastKind::Error,
                "No one to call: the chat's members have no email address.",
            );
            return;
        };
        let names = self.names_for(&users);
        self.open_call(CallTarget::People { names, video }, link, chat, ctx);
    }

    fn one_on_one_with(&self, users: &[String]) -> Option<ConversationId> {
        let [email] = users else { return None };
        let me = self.me();
        self.chats
            .iter()
            .filter(|c| c.kind == ChatKind::OneOnOne)
            .find(|c| {
                c.peer(me)
                    .and_then(|p| p.email.as_deref())
                    .is_some_and(|e| e.eq_ignore_ascii_case(email))
            })
            .map(|c| ConversationId::Chat(c.id.clone()))
    }

    fn names_for(&self, users: &[String]) -> String {
        let names: Vec<String> = users
            .iter()
            .map(|email| {
                self.chats
                    .iter()
                    .flat_map(|c| &c.members)
                    .find(|m| {
                        m.email
                            .as_deref()
                            .is_some_and(|e| e.eq_ignore_ascii_case(email))
                    })
                    .map(|m| m.display_name.clone())
                    .unwrap_or_else(|| email.clone())
            })
            .collect();
        match names.as_slice() {
            [] => String::new(),
            [one] => one.clone(),
            [first, rest @ ..] => format!("{first} and {} more", rest.len()),
        }
    }

    /// Opens a call or meeting link and starts following it.
    fn open_call(
        &mut self,
        target: CallTarget,
        url: String,
        chat: Option<ConversationId>,
        ctx: &egui::Context,
    ) {
        // The same meeting again: bring it forward rather than join twice.
        if let Some(call) = &self.call
            && !call.is_ended()
            && call.join_url().is_some_and(|open| same_meeting(open, &url))
        {
            self.show_call();
            return;
        }
        let opener = self.opener();
        // Dock first, so the call window opens where Chaat was.
        let placement = match &opener {
            Opener::AppWindow(_) if self.settings.calls.dock => self.dock(ctx),
            _ => None,
        };
        let place = opener.place();
        if self.demo {
            self.toast(
                ToastKind::Info,
                format!("Demo: this opens Teams on the web in {place}."),
            );
        } else {
            let profile = self.browser_profile(self.active);
            let launched = match &opener {
                Opener::AppWindow(browser) => {
                    browser::app_window(browser, &url, placement, profile.as_deref())
                        .map(|launch| browser::spawn(&launch))
                }
                Opener::Browser | Opener::TeamsApp => None,
            };
            match launched {
                Some(Ok(())) => {}
                Some(Err(error)) => {
                    log::warn!("could not start the call browser: {error}");
                    super::open_external(&url);
                }
                None => super::open_external(&url),
            }
        }
        let now = crate::time::now();
        let mut session = CallSession::new(target, now, self.id.clone(), place);
        if self.demo {
            session.state = CallState::Live { since: now };
        }
        session.chat = chat.or_else(|| self.cached_meeting_chat(&session));
        if session.chat.is_some() {
            if session.is_meeting() {
                session.lookup = ChatLookup::Found;
            }
        } else if let Some(url) = session.join_url().map(str::to_owned) {
            session.lookup = if self.demo {
                ChatLookup::NotFound
            } else if self.has_scope(crate::auth::ONLINE_MEETINGS_SCOPE) {
                self.backend.send(Command::FindMeetingChat(url));
                ChatLookup::Searching
            } else {
                ChatLookup::NeedsPermission
            };
        }
        if let Some(chat) = session.chat.clone() {
            self.section = Section::Chats;
            self.actions.push(Action::OpenConversation(chat));
        }
        self.call = Some(session);
    }

    fn cached_meeting_chat(&self, session: &CallSession) -> Option<ConversationId> {
        let url = session.join_url()?;
        self.chats
            .iter()
            .find(|c| c.join_url.as_deref().is_some_and(|j| same_meeting(j, url)))
            .map(|c| ConversationId::Chat(c.id.clone()))
    }

    /// The person asked for the meeting's chat: look it up through Graph,
    /// asking for OnlineMeetings.Read first.
    pub(super) fn find_meeting_chat(&mut self) {
        let Some(session) = &self.call else { return };
        if let Some(chat) = self.cached_meeting_chat(session) {
            self.attach_chat(chat);
            return;
        }
        let Some(url) = session.join_url().map(str::to_owned) else {
            return;
        };
        if !self.ensure_scope(crate::auth::ONLINE_MEETINGS_SCOPE) {
            return;
        }
        self.backend.send(Command::FindMeetingChat(url));
        if let Some(call) = &mut self.call {
            call.lookup = ChatLookup::Searching;
        }
    }

    fn attach_chat(&mut self, chat: ConversationId) {
        if let Some(call) = &mut self.call {
            call.chat = Some(chat.clone());
            call.lookup = ChatLookup::Found;
        }
        if !self.hidden {
            self.section = Section::Chats;
            self.open_conversation(chat);
        }
    }

    pub(super) fn on_meeting_chat(&mut self, join_url: String, chat: Option<Box<Chat>>) {
        let ours = self
            .call
            .as_ref()
            .and_then(|c| c.join_url())
            .is_some_and(|open| same_meeting(open, &join_url));
        match chat {
            Some(chat) => {
                let chat = *chat;
                let id = chat.id.clone();
                self.chats.retain(|c| c.id != id);
                self.chats.insert(0, chat);
                if ours {
                    self.attach_chat(ConversationId::Chat(id));
                }
            }
            None if ours => {
                if let Some(call) = &mut self.call {
                    call.lookup = ChatLookup::NotFound;
                }
            }
            None => {}
        }
    }

    /// Follows the call from a reading of the person's own presence. A call
    /// Chaat did not open shows too, but not a calendar entry alone.
    /// A bar the person closed stays closed for the rest of that call.
    pub(super) fn observe_call(&mut self, activity: Option<&str>) {
        let now = crate::time::now();
        let id = self.id.clone();
        let in_a_call = in_call(activity, true);
        let dismissed = self.call_dismissed.as_ref() == Some(&id);
        if dismissed && !in_a_call {
            self.call_dismissed = None;
        }
        match &mut self.call {
            Some(call) if call.account == id => call.observe(activity, now),
            Some(_) => {}
            None if !self.hidden && !self.demo && in_a_call && !dismissed => {
                self.call = Some(CallSession::elsewhere(now, id));
            }
            None => {}
        }
    }

    /// Moves the call past its deadlines; once a frame.
    pub(super) fn tick_call(&mut self, ctx: &egui::Context) {
        let Some(call) = &mut self.call else { return };
        let now = crate::time::now();
        let next = call.advance(now);
        let ended = call.is_ended();
        if call.is_finished(now) {
            self.call = None;
        } else if let Some(ms) = next {
            ctx.request_repaint_after(std::time::Duration::from_millis(
                ms.clamp(250, 60_000) as u64
            ));
        }
        if ended && self.docked.is_some() {
            self.undock(ctx);
        }
    }

    pub(super) fn end_call(&mut self, ctx: &egui::Context) {
        if let Some(call) = self.call.take() {
            self.call_dismissed = Some(call.account);
        }
        self.undock(ctx);
    }

    /// Whether "Show call" can bring the call window forward here: on macOS,
    /// by activating the browser or Teams.
    pub fn can_show_call(&self) -> bool {
        use crate::calling::CallsOpenIn;
        let no_app_window = self.settings.calls.open_in == CallsOpenIn::AppWindow
            && self.browsers.as_deref().is_some_and(<[Browser]>::is_empty);
        cfg!(target_os = "macos")
            && !self.demo
            && self.settings.calls.open_in != CallsOpenIn::Browser
            && !no_app_window
    }

    /// Brings the browser (or Teams) forward. Never re-opens the link, which
    /// could join a second time.
    pub(super) fn show_call(&mut self) {
        #[cfg(target_os = "macos")]
        {
            let bundle = match self.opener() {
                Opener::AppWindow(browser) => browser
                    .program
                    .ancestors()
                    .find(|p| p.extension().is_some_and(|e| e == "app"))
                    .map(|p| p.to_path_buf()),
                Opener::TeamsApp => Some("/Applications/Microsoft Teams.app".into()),
                Opener::Browser => None,
            };
            if let Some(bundle) = bundle {
                let launch = browser::Launch {
                    program: "/usr/bin/open".into(),
                    args: vec!["-a".into(), bundle.to_string_lossy().into_owned()],
                };
                if let Err(error) = browser::spawn(&launch) {
                    log::info!("could not bring the call forward: {error}");
                }
            }
        }
    }

    /// Moves Chaat into a narrow panel on the right of its frame and returns
    /// where the call window should go, or `None` when it cannot dock
    /// (unknown position on Wayland, full screen, a small window).
    pub(super) fn dock(&mut self, ctx: &egui::Context) -> Option<Placement> {
        if let Some(docked) = self.docked {
            return Some(docked.call);
        }
        let (outer, inner, fullscreen, maximized) = ctx.input(|i| {
            let viewport = i.viewport();
            (
                viewport.outer_rect,
                viewport.inner_rect,
                viewport.fullscreen.unwrap_or(false),
                viewport.maximized.unwrap_or(false),
            )
        });
        let (Some(outer), Some(inner)) = (outer, inner) else {
            return None;
        };
        if fullscreen {
            return None;
        }
        // egui measures in zoomed points; browsers in screen points.
        let zoom = ctx.zoom_factor();
        let plan = dock::plan(dock::Frame {
            x: outer.min.x * zoom,
            y: outer.min.y * zoom,
            width: outer.width() * zoom,
            height: outer.height() * zoom,
        })?;
        let decorations = outer.size() - inner.size();
        let size = vec2(
            plan.chaat.width / zoom - decorations.x,
            plan.chaat.height / zoom - decorations.y,
        );
        self.docked = Some(Docked {
            position: Some(outer.min),
            inner: inner.size(),
            maximized,
            call: plan.call,
        });
        if maximized {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
        }
        ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(vec2(
            size.x.min(MIN_SIZE.x),
            size.y.min(MIN_SIZE.y),
        )));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(
            plan.chaat.x / zoom,
            plan.chaat.y / zoom,
        )));
        Some(plan.call)
    }

    /// Puts Chaat's window back where it was before docking.
    pub(super) fn undock(&mut self, ctx: &egui::Context) {
        let Some(docked) = self.docked.take() else {
            return;
        };
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(docked.inner));
        if let Some(position) = docked.position {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(position));
        }
        ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(MIN_SIZE));
        if docked.maximized {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
        }
    }

    /// For tests and screenshots: the docked layout without moving a window.
    pub fn pretend_docked(&mut self) {
        self.docked = Some(Docked {
            position: None,
            inner: MIN_SIZE,
            maximized: false,
            call: Placement {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
        });
    }

    pub(super) fn join_on_phone(&mut self) {
        if let Some(url) = self.call.as_ref().and_then(|c| c.join_url()) {
            self.overlay = Some(Overlay::JoinOnPhone(url.to_owned()));
        }
    }

    /// Opens Teams on the web once in the call browser, to sign in there (and
    /// choose "Stay signed in") ahead of calls.
    pub(super) fn prepare_teams_web(&mut self) {
        let profile = self.browser_profile(self.active);
        let launched = match self.opener() {
            Opener::AppWindow(browser) => {
                browser::app_window(&browser, TEAMS_WEB, None, profile.as_deref())
                    .map(|launch| browser::spawn(&launch))
            }
            _ => None,
        };
        if !matches!(launched, Some(Ok(()))) {
            super::open_external(TEAMS_WEB);
        }
    }

    /// With the setting on, signing out of Chaat also signs the call browser
    /// out of Microsoft (and so out of Teams on the web).
    pub(super) fn sign_out_browser(&mut self) {
        if !self.settings.calls.sign_out_browser || self.demo {
            return;
        }
        let Opener::AppWindow(browser) = self.opener() else {
            return;
        };
        let tenant = self.settings.tenant.trim().to_owned();
        let url = format!(
            "{}/{}/oauth2/v2.0/logout",
            crate::auth::DEFAULT_AUTHORITY,
            urlencoding::encode(if tenant.is_empty() {
                "organizations"
            } else {
                &tenant
            })
        );
        let profile = self.browser_profile(self.active);
        if let Some(launch) = browser::page(&browser, &url, profile.as_deref())
            && let Err(error) = browser::spawn(&launch)
        {
            log::info!("could not open the sign-out page: {error}");
        }
    }

    /// The launch for a web link when links open in the call browser, or
    /// `None` for the default browser.
    pub fn link_launch(&mut self, url: &str) -> Option<browser::Launch> {
        if !self.settings.calls.links_in_browser {
            return None;
        }
        let preferred = self.settings.calls.browser;
        let chosen = browser::pick(self.call_browsers(), preferred)?.clone();
        let profile = self.browser_profile(self.active);
        browser::link(&chosen, url, profile.as_deref())
    }

    /// Opens a web link in the call browser when chosen, else (or if that
    /// fails) in the default browser.
    pub(super) fn open_link(&mut self, url: &str) {
        if let Some(launch) = self.link_launch(url) {
            match browser::spawn(&launch) {
                Ok(()) => return,
                Err(error) => log::info!("could not open a link in the call browser: {error}"),
            }
        }
        super::open_external(url);
    }

    pub(super) fn set_call_settings(&mut self, calls: crate::settings::CallSettings) {
        if self.settings.calls == calls {
            return;
        }
        self.settings.calls = calls;
        self.mark_settings_dirty();
        self.sync_call_browser();
    }
}

/// Which rings already raised the window: each call id once, and the window
/// at most once every `MIN_GAP` overall, so a plugin cannot keep stealing focus.
#[derive(Debug, Default)]
pub(super) struct RingRaises {
    seen: std::collections::VecDeque<String>,
    last: Option<std::time::Instant>,
}

impl RingRaises {
    const REMEMBER: usize = 16;
    const MIN_GAP: std::time::Duration = std::time::Duration::from_secs(10);

    /// Whether the ring of this call may raise the window now. Remembers the
    /// call either way, so it never raises later.
    fn allow(&mut self, call_id: &str, now: std::time::Instant) -> bool {
        if self.seen.iter().any(|seen| seen == call_id) {
            return false;
        }
        if self.seen.len() >= Self::REMEMBER {
            self.seen.pop_front();
        }
        self.seen.push_back(call_id.to_owned());
        if self
            .last
            .is_some_and(|last| now.saturating_duration_since(last) < Self::MIN_GAP)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// Ring alerts from the call-alerts plugin (docs/calling.md).
impl App {
    /// A call is ringing, as the call-alerts plugin reported. Shows the ring
    /// toast, with the caller resolved against the chats of the account it
    /// rang on, and brings Chaat forward so the person sees it.
    pub(super) fn on_incoming_call(&mut self, alert: crate::calling::RingAlert) {
        // An older plugin doesn't say which account: take the one on screen.
        let index = match &alert.account {
            None => Some(self.active),
            Some(id) => self
                .accounts
                .iter()
                .position(|a| a.id.as_deref() == Some(id.as_str())),
        };
        let (chat, member) = index
            .and_then(|i| self.accounts.get(i))
            .map(|account| resolve_caller(account, &alert))
            .unwrap_or_default();
        // Name the account when it isn't obvious: several accounts, or one
        // Chaat doesn't have.
        let account_name = match index.and_then(|i| self.accounts.get(i)) {
            Some(account) if self.accounts.len() > 1 => Some(account.username())
                .filter(|name| !name.is_empty())
                .or_else(|| Some(account.label())),
            Some(_) => None,
            None => alert
                .username
                .clone()
                .filter(|name| !name.is_empty())
                .or_else(|| alert.account.clone()),
        };
        let account = index
            .and_then(|i| self.accounts.get(i))
            .and_then(|a| a.id.clone());
        // One group call can ring several accounts: a card for each.
        let key = match &alert.account {
            Some(account) => format!("{}#{account}", alert.id),
            None => alert.id.clone(),
        };
        self.rings.retain(|ring| ring.id != key);
        let call_id = alert.id.clone();
        // A resolved caller shows under Chaat's own name for them; an
        // unresolved one under the plugin's, which the card marks.
        let caller = member
            .as_ref()
            .map(|m| m.display_name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(alert.caller);
        self.rings.push(Ring {
            id: key,
            caller,
            video: alert.video,
            group: alert.group,
            chat,
            member,
            account,
            account_name,
            shown_for: alert
                .ring_timeout
                .unwrap_or(crate::calling::alerts::DEFAULT_RING),
            arrived: std::time::Instant::now(),
        });
        // Several at once is rare; keep the newest few.
        if self.rings.len() > 3 {
            self.rings.remove(0);
        }
        if self.ring_raises.allow(&call_id, std::time::Instant::now()) {
            self.wants_show = true;
        }
    }

    /// Takes a ring off the screen and shows the account it rang on, so its
    /// call back or chat happens there.
    fn take_ring_and_switch(&mut self, id: &str, ctx: &egui::Context) -> Option<Ring> {
        let ring = self.take_ring(id)?;
        if let Some(index) = ring.account.as_deref().and_then(|account| {
            self.accounts
                .iter()
                .position(|a| a.id.as_deref() == Some(account))
        }) && index != self.active
        {
            self.apply(Action::SwitchAccount(index), ctx);
        }
        Some(ring)
    }

    /// "Place call": call the caller back in Teams. Without their address,
    /// opens Teams on the web so the person can call from there.
    pub(super) fn ring_place_call(&mut self, id: &str, ctx: &egui::Context) {
        let Some(ring) = self.take_ring_and_switch(id, ctx) else {
            return;
        };
        match ring.email() {
            Some(email) => {
                let users = vec![email.to_owned()];
                // A group ring's chat is the group's; the call back dials only
                // the caller, so it follows their one-on-one chat instead.
                let chat = if ring.group {
                    self.one_on_one_with(&users)
                } else {
                    ring.chat.clone().or_else(|| self.one_on_one_with(&users))
                };
                self.call_people(users, ring.video, chat, ctx);
            }
            None => {
                if !self.demo {
                    self.prepare_teams_web();
                }
                self.toast(
                    ToastKind::Info,
                    format!(
                        "Chaat doesn't know {}'s address; opened Teams to call back.",
                        ring.caller
                    ),
                );
            }
        }
    }

    /// "Open chat": the chat with the caller, in Chaat. Starts the one-on-one
    /// chat if Chaat knows the caller but has no chat with them yet.
    pub(super) fn ring_open_chat(&mut self, id: &str, ctx: &egui::Context) {
        let Some(ring) = self.take_ring_and_switch(id, ctx) else {
            return;
        };
        if let Some(chat) = ring.chat {
            self.open_conversation(chat);
        } else if let Some(member) = ring.member {
            self.create_chat(vec![member], None);
        }
    }

    fn take_ring(&mut self, id: &str) -> Option<Ring> {
        let index = self.rings.iter().position(|ring| ring.id == id)?;
        Some(self.rings.remove(index))
    }
}

/// The chat and the person behind a ring, from what one account already has:
/// the call's thread when the push carried one, and the chat member whose user
/// id (the Entra object id) matches the caller's MRI.
fn resolve_caller(
    account: &super::Account,
    alert: &crate::calling::RingAlert,
) -> (Option<ConversationId>, Option<Member>) {
    let me = account.me();
    let member = alert.caller_object_id.as_deref().and_then(|object_id| {
        let matches = |m: &&Member| m.user_id.eq_ignore_ascii_case(object_id);
        // Prefer a member record with an address, for the call back.
        let mut known = account
            .chats
            .iter()
            .flat_map(|c| &c.members)
            .filter(matches);
        let first = known.clone().next().cloned();
        known.find(|m| m.email.is_some()).cloned().or(first)
    });
    // The plugin's thread counts only when the resolved caller is in it.
    let thread = alert
        .thread_id
        .as_deref()
        .filter(|id| {
            member.as_ref().is_some_and(|member| {
                account.chat(id).is_some_and(|chat| {
                    chat.members
                        .iter()
                        .any(|m| m.user_id.eq_ignore_ascii_case(&member.user_id))
                })
            })
        })
        .map(|id| ConversationId::Chat(id.to_owned()));
    let one_on_one = member.as_ref().and_then(|member| {
        account
            .chats
            .iter()
            .find(|c| {
                c.kind == ChatKind::OneOnOne
                    && c.peer(me).is_some_and(|p| p.user_id == member.user_id)
            })
            .map(|c| ConversationId::Chat(c.id.clone()))
    });
    (thread.or(one_on_one), member)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calling::session::GRACE;

    fn demo() -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(
            crate::paths::AppDirs::under(dir.path()),
            crate::settings::Settings::default(),
            crate::backend::Waker::default(),
            true,
        );
        (app, dir)
    }

    fn alert(id: &str, caller: &str, object_id: &str) -> crate::calling::RingAlert {
        crate::calling::RingAlert {
            id: id.into(),
            caller: caller.into(),
            caller_object_id: Some(object_id.into()),
            video: false,
            group: false,
            thread_id: None,
            account: None,
            username: None,
            ring_timeout: None,
        }
    }

    #[test]
    fn a_ring_raises_the_window_once_per_call_and_at_most_every_ten_seconds() {
        let (mut app, _dir) = demo();
        app.wants_show = false;
        app.on_incoming_call(alert("c1", "A", "u-ada"));
        assert!(app.wants_show);
        // The same call again: no second raise.
        app.wants_show = false;
        app.on_incoming_call(alert("c1", "A", "u-ada"));
        assert!(!app.wants_show);
        // Another call right after: the card shows, the window stays put.
        app.on_incoming_call(alert("c2", "B", "u-ada"));
        assert!(!app.wants_show);
        assert_eq!(app.rings.len(), 2);

        // The gap and the memory, directly.
        let mut raises = RingRaises::default();
        let t0 = std::time::Instant::now();
        assert!(raises.allow("a", t0));
        assert!(!raises.allow("b", t0 + std::time::Duration::from_secs(9)));
        assert!(raises.allow("c", t0 + std::time::Duration::from_secs(10)));
        assert!(!raises.allow("c", t0 + std::time::Duration::from_secs(60)));
        // "b" never raised, and it never will.
        assert!(!raises.allow("b", t0 + std::time::Duration::from_secs(60)));
        // Only the last 16 calls are remembered.
        for n in 0..16 {
            raises.allow(&format!("x{n}"), t0);
        }
        assert!(raises.seen.len() <= 16);
        assert!(raises.allow("a", t0 + std::time::Duration::from_secs(600)));
    }

    #[test]
    fn the_card_names_the_resolved_member_not_the_plugin() {
        let (mut app, _dir) = demo();
        app.on_incoming_call(alert("c1", "Totally Not Ada", "u-ada"));
        let shown = &app.rings[0];
        assert!(shown.member.is_some());
        assert_eq!(shown.caller, "Ada Lovelace");
    }

    #[test]
    fn an_unresolved_caller_is_marked_as_from_the_plugin() {
        let (mut app, _dir) = demo();
        app.on_incoming_call(alert("c1", "Someone New", "u-nobody"));
        let shown = &app.rings[0];
        assert_eq!(shown.caller, "Someone New");
        // The card marks a name without a member as the plugin's.
        assert!(shown.member.is_none());
    }

    #[test]
    fn a_thread_without_the_caller_is_not_opened() {
        let (mut app, _dir) = demo();
        let without = app
            .chats
            .iter()
            .find(|c| c.members.iter().all(|m| m.user_id != "u-ada"))
            .map(|c| c.id.clone())
            .expect("a demo chat without Ada");
        let with = app
            .chats
            .iter()
            .find(|c| {
                c.kind != ChatKind::OneOnOne && c.members.iter().any(|m| m.user_id == "u-ada")
            })
            .map(|c| c.id.clone())
            .expect("a demo group chat with Ada");
        let mut ring = alert("c1", "Ada", "u-ada");
        ring.thread_id = Some(without.clone());
        app.on_incoming_call(ring);
        assert_ne!(
            app.rings[0].chat,
            Some(ConversationId::Chat(without.clone()))
        );
        // Nobody resolved: no thread at all.
        let mut ring = alert("c2", "Ada", "u-nobody");
        ring.thread_id = Some(with.clone());
        app.on_incoming_call(ring);
        assert_eq!(app.rings[1].chat, None);
        // The caller is in it: kept.
        let mut ring = alert("c3", "Ada", "u-ada");
        ring.thread_id = Some(with.clone());
        app.on_incoming_call(ring);
        assert_eq!(app.rings[2].chat, Some(ConversationId::Chat(with)));
    }

    const UNKNOWN: &str =
        "https://teams.microsoft.com/l/meetup-join/19%3ameeting_elsewhere%40thread.v2/0";

    #[test]
    fn presence_ends_a_call_after_the_grace_period() {
        let (mut app, _dir) = demo();
        let ctx = egui::Context::default();
        app.join_meeting(UNKNOWN.into(), &ctx);
        app.pretend_docked();
        // The demo treats the call as live at once.
        app.observe_call(Some("Available"));
        let Some(call) = &mut app.call else {
            panic!("a call")
        };
        let CallState::Leaving { since, .. } = call.state else {
            panic!("leaving, not {:?}", call.state)
        };
        // A minute later presence still says the call is over.
        call.state = CallState::Leaving {
            since,
            left: crate::time::now() - GRACE - 1,
        };
        app.tick_call(&ctx);
        assert!(app.call.as_ref().is_some_and(CallSession::is_ended));
        assert!(
            app.docked.is_none(),
            "the window is restored when the call ends"
        );
    }

    #[test]
    fn calls_elsewhere_show_but_calendar_meetings_do_not() {
        let (mut app, _dir) = demo();
        app.demo = false;
        app.observe_call(Some("InAMeeting"));
        assert!(app.call.is_none());
        app.observe_call(Some("InACall"));
        assert_eq!(app.call.as_ref().unwrap().target, CallTarget::Elsewhere);
    }

    #[test]
    fn a_dismissed_call_bar_stays_closed_until_the_call_ends() {
        let (mut app, _dir) = demo();
        app.demo = false;
        let ctx = egui::Context::default();
        app.observe_call(Some("InACall"));
        assert!(app.call.is_some());
        app.apply(Action::CallDone, &ctx);
        assert!(app.call.is_none());
        // Presence still says the call goes on: the bar stays closed.
        app.observe_call(Some("InACall"));
        assert!(app.call.is_none());
        // The call ended; the next one shows again.
        app.observe_call(Some("Available"));
        app.observe_call(Some("InACall"));
        assert!(app.call.is_some());
    }

    #[test]
    fn a_meeting_chat_found_later_is_shown_beside_the_call() {
        let (mut app, _dir) = demo();
        let ctx = egui::Context::default();
        app.join_meeting(UNKNOWN.into(), &ctx);
        assert_eq!(app.call.as_ref().unwrap().lookup, ChatLookup::NotFound);
        assert_eq!(app.call.as_ref().unwrap().title(), "Meeting");
        let chat = Chat {
            id: "19:meeting_elsewhere@thread.v2".into(),
            kind: ChatKind::Meeting,
            join_url: Some(UNKNOWN.into()),
            ..Default::default()
        };
        // Another meeting's answer is kept but not attached.
        app.on_meeting_chat(
            "https://teams.microsoft.com/l/meetup-join/other/0".into(),
            None,
        );
        assert_eq!(app.call.as_ref().unwrap().lookup, ChatLookup::NotFound);
        app.on_meeting_chat(UNKNOWN.into(), Some(Box::new(chat)));
        let call = app.call.as_ref().unwrap();
        assert_eq!(call.lookup, ChatLookup::Found);
        let id = ConversationId::Chat("19:meeting_elsewhere@thread.v2".into());
        assert_eq!(call.chat.as_ref(), Some(&id));
        assert_eq!(app.open.as_ref(), Some(&id));
        assert!(
            app.chats
                .iter()
                .any(|c| c.id == "19:meeting_elsewhere@thread.v2")
        );
    }

    #[test]
    fn calls_started_from_a_chat_follow_that_chat() {
        let (mut app, _dir) = demo();
        let ctx = egui::Context::default();
        let me = app.me().to_owned();
        let chat = app
            .chats
            .iter()
            .find(|c| {
                c.kind == ChatKind::OneOnOne && c.peer(&me).is_some_and(|p| p.email.is_some())
            })
            .cloned()
            .unwrap();
        let email = chat.peer(&me).and_then(|p| p.email.clone()).unwrap();
        let name = chat.peer(&me).map(|p| p.display_name.clone()).unwrap();
        // From the Calls page: the one-on-one chat is found by address.
        app.section = Section::Calls;
        app.start_call(vec![email], true, &ctx);
        let call = app.call.as_ref().unwrap();
        assert_eq!(call.title(), name);
        assert_eq!(call.chat, Some(ConversationId::Chat(chat.id.clone())));
        assert_eq!(call.lookup, ChatLookup::NotApplicable);
    }

    #[test]
    fn settings_reach_every_backend_and_profiles_stick_to_accounts() {
        let (mut app, _dir) = demo();
        app.id = Some("oid.tid".into());
        app.remember_browser_profile();
        let profile = app.browser_profile.clone();
        assert_eq!(app.settings.calls.profiles.get("oid.tid"), Some(&profile));
        assert!(app.browser_profile(0).is_none(), "off by default");
        let mut calls = app.settings.calls.clone();
        calls.profile_per_account = true;
        app.set_call_settings(calls);
        assert_eq!(app.browser_profile(0), Some(profile));
    }

    #[test]
    fn links_open_in_the_call_browser_only_when_chosen() {
        let (mut app, _dir) = demo();
        let edge = Browser {
            kind: browser::BrowserKind::Edge,
            program: "/edge".into(),
        };
        let chrome = Browser {
            kind: browser::BrowserKind::Chrome,
            program: "/chrome".into(),
        };
        app.browsers = Some(vec![edge, chrome]);
        let url = "https://example.com/doc";
        assert!(
            app.link_launch(url).is_none(),
            "the default browser by default"
        );

        let mut calls = app.settings.calls.clone();
        calls.links_in_browser = true;
        calls.browser = Some(browser::BrowserKind::Chrome);
        app.set_call_settings(calls);
        let launch = app.link_launch(url).unwrap();
        assert_eq!(launch.program, std::path::PathBuf::from("/chrome"));
        assert_eq!(launch.args, [url]);
        // Links follow the call profile when each account has its own.
        let mut calls = app.settings.calls.clone();
        calls.profile_per_account = true;
        app.set_call_settings(calls);
        let profile = app.browser_profile.clone();
        assert_eq!(
            app.link_launch(url).unwrap().args,
            [format!("--profile-directory={profile}"), url.to_owned()]
        );
        // Links that are not web links never reach the browser.
        assert!(app.link_launch("file:///etc/passwd").is_none());
        // With no Edge or Chrome, the default browser opens them.
        app.browsers = Some(Vec::new());
        assert!(app.link_launch(url).is_none());
    }
}
