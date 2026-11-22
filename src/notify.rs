//! Desktop notifications: the notification centre on macOS, toasts on
//! Windows, the desktop's notification daemon on Linux.
//!
//! Shown from a short-lived thread, since the platform calls can block.
//! Notification text is never logged.

pub mod rules;
pub mod schedule;

use std::collections::HashMap;
use std::path::Path;
#[cfg(windows)]
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{LazyLock, Mutex, PoisonError};

use crate::backend::Waker;
use crate::model::ConversationId;

/// Toast threads that may wait for a click at once; past this, toasts show
/// without click handling.
const WAIT_CAP: usize = 16;

/// The Windows toast app id, registered by [`register_identity`].
#[cfg(windows)]
const APP_ID: &str = "app.chaat.Chaat";

/// Set once the Windows app id key is in place.
#[cfg(windows)]
static APP_ID_REGISTERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Which toast threads are waiting for a click, and which of them may still
/// forward one.
///
/// On Linux a notice with a replace key reuses one notification id, and
/// notify-rust's wait filters the daemon's signals by id and cannot be
/// cancelled. So a thread for an earlier toast keeps listening after a later
/// toast took its place, and one click would reach every such thread, the
/// older ones with stale targets. Each replace key therefore has a generation:
/// only the thread of the newest toast for that key may forward a click, and
/// replacing a toast frees its thread's slot at once. The replaced thread
/// itself ends when the notification closes (its close signal carries the same
/// id), but it forwards nothing and no longer counts against the cap.
///
/// Notices without a key (always the case on Windows and macOS, where
/// replacement is unused) never go stale.
struct Waiters {
    cap: usize,
    state: Mutex<WaitState>,
}

#[derive(Default)]
struct WaitState {
    next_generation: u64,
    /// Per replace key: the newest generation and whether it holds a slot.
    keyed: HashMap<String, (u64, bool)>,
    /// Slots held by notices without a key.
    unkeyed: usize,
}

impl WaitState {
    fn waiting(&self) -> usize {
        self.unkeyed + self.keyed.values().filter(|(_, held)| *held).count()
    }
}

/// One toast's place in the order for its replace key.
struct Ticket {
    key: Option<String>,
    generation: u64,
}

/// A claim on one of the waiting slots, released when dropped.
struct WaitGuard<'a> {
    waiters: &'a Waiters,
    ticket: Ticket,
}

impl Waiters {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            state: Mutex::new(WaitState::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WaitState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes the next place for `key`, which makes every earlier ticket for
    /// the key stale and frees the slot it held. Call in the order the
    /// notices are shown, before showing.
    fn register(&self, key: Option<&str>) -> Ticket {
        let mut state = self.lock();
        state.next_generation += 1;
        let generation = state.next_generation;
        if let Some(key) = key {
            state.keyed.insert(key.to_owned(), (generation, false));
        }
        Ticket {
            key: key.map(str::to_owned),
            generation,
        }
    }

    fn is_current(&self, ticket: &Ticket) -> bool {
        match &ticket.key {
            None => true,
            Some(key) => self
                .lock()
                .keyed
                .get(key)
                .is_some_and(|(generation, _)| *generation == ticket.generation),
        }
    }

    /// Claims a slot, unless the ticket is stale or the cap is reached.
    fn claim(&self, ticket: Ticket) -> Option<WaitGuard<'_>> {
        let mut state = self.lock();
        if state.waiting() >= self.cap {
            // Not waiting after all: leave no entry behind for the key.
            if let Some(key) = &ticket.key
                && state
                    .keyed
                    .get(key)
                    .is_some_and(|(generation, _)| *generation == ticket.generation)
            {
                state.keyed.remove(key);
            }
            return None;
        }
        match &ticket.key {
            None => state.unkeyed += 1,
            Some(key) => match state.keyed.get_mut(key) {
                Some(entry) if entry.0 == ticket.generation => entry.1 = true,
                _ => return None,
            },
        }
        drop(state);
        Some(WaitGuard {
            waiters: self,
            ticket,
        })
    }

    /// Drops a ticket that never waited (the toast failed to show).
    fn forget(&self, ticket: &Ticket) {
        if let Some(key) = &ticket.key {
            let mut state = self.lock();
            if state
                .keyed
                .get(key)
                .is_some_and(|(generation, _)| *generation == ticket.generation)
            {
                state.keyed.remove(key);
            }
        }
    }
}

impl WaitGuard<'_> {
    fn is_current(&self) -> bool {
        self.waiters.is_current(&self.ticket)
    }
}

impl Drop for WaitGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.waiters.lock();
        match &self.ticket.key {
            None => state.unkeyed -= 1,
            Some(key) => {
                // A replaced ticket's slot was freed when it was replaced.
                if state
                    .keyed
                    .get(key)
                    .is_some_and(|(generation, _)| *generation == self.ticket.generation)
                {
                    state.keyed.remove(key);
                }
            }
        }
    }
}

static WAITERS: LazyLock<Waiters> = LazyLock::new(|| Waiters::new(WAIT_CAP));

pub fn show(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    claim_identity();
    let title = title.to_owned();
    let body = crate::html::truncate(body, 240);
    let _ = std::thread::Builder::new()
        .name("chaat-notify".into())
        .spawn(move || {
            let mut notification = notify_rust::Notification::new();
            notification.summary(&title).body(&body).appname("Chaat");
            #[cfg(target_os = "linux")]
            notification.icon("chaat");
            if let Err(error) = notification.show() {
                log::info!("could not show a notification: {error}");
            }
        });
}

/// macOS attributes a notification to an app by bundle identifier. Inside
/// Chaat.app, post as Chaat; a bare binary (development) keeps the library's
/// default, since an identifier without a bundle shows nothing.
#[cfg(target_os = "macos")]
fn claim_identity() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let bundled = std::env::current_exe()
            .map(|path| path.to_string_lossy().contains(".app/Contents/MacOS/"))
            .unwrap_or(false);
        if bundled && let Err(error) = notify_rust::set_application("app.chaat.Chaat") {
            log::info!("notifications keep the default sender: {error}");
        }
    });
}

/// What a notification opens when clicked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Conversation {
        account: Option<String>,
        conversation: ConversationId,
        message: Option<String>,
    },
    Meeting {
        account: Option<String>,
        meeting: String,
    },
    Person {
        account: Option<String>,
        user: String,
    },
    Test,
}

/// A button on a notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Join,
    Snooze,
}

/// A notification to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    pub target: Target,
    pub buttons: Vec<Button>,
    /// A key: a notice with the same key replaces the earlier one.
    pub replace: Option<String>,
}

/// What the person did with a notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clicked {
    Open(Target),
    Join(Target),
    Snooze(Target),
}

/// What a notification action id means. `"default"` is a click on the body;
/// buttons carry their own ids; `"__closed"` and anything else do nothing.
fn clicked_for(action: &str, target: &Target) -> Option<Clicked> {
    match action {
        "default" => Some(Clicked::Open(target.clone())),
        "join" => Some(Clicked::Join(target.clone())),
        "snooze" => Some(Clicked::Snooze(target.clone())),
        _ => None,
    }
}

fn button_id(button: Button) -> (&'static str, &'static str) {
    match button {
        Button::Join => ("join", "Join"),
        Button::Snooze => ("snooze", "Snooze"),
    }
}

/// The actions a notification declares, as (id, label). On Linux a click on
/// the body comes back only when there is a `default` action (freedesktop
/// notification spec); without it daemons such as dunst treat the click as
/// a dismissal. GNOME and KDE show no button for it. Windows reports body
/// clicks on its own, and there an extra action would draw a button.
fn actions(buttons: &[Button], with_default: bool) -> Vec<(&'static str, &'static str)> {
    let mut list = Vec::with_capacity(buttons.len() + 1);
    if with_default {
        list.push(("default", "Open"));
    }
    list.extend(buttons.iter().map(|button| button_id(*button)));
    list
}

/// The Linux notification id for a replace key. Never 0, which asks for a
/// new notification.
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
fn replace_id(key: &str) -> u32 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    (hasher.finish() as u32).max(1)
}

/// The handle the app shows notifications through. A recording handle keeps
/// them instead of showing them (demo and tests).
pub struct Desktop {
    recording: bool,
    shown: Mutex<Vec<Notice>>,
    clicks_tx: Sender<Clicked>,
    clicks_rx: Mutex<Receiver<Clicked>>,
    waker: Option<Waker>,
}

impl Desktop {
    pub fn new(waker: Waker) -> Self {
        Self::build(false, Some(waker))
    }

    pub fn recording() -> Self {
        Self::build(true, None)
    }

    fn build(recording: bool, waker: Option<Waker>) -> Self {
        let (clicks_tx, clicks_rx) = channel();
        Self {
            recording,
            shown: Mutex::new(Vec::new()),
            clicks_tx,
            clicks_rx: Mutex::new(clicks_rx),
            waker,
        }
    }

    pub fn show(&self, notice: Notice) {
        if self.recording {
            if let Ok(mut shown) = self.shown.lock() {
                shown.push(notice);
            }
            return;
        }
        #[cfg(target_os = "macos")]
        claim_identity();
        let tx = self.clicks_tx.clone();
        let waker = self.waker.clone();
        // Only Linux replaces a toast in place; elsewhere each toast keeps
        // its own click handling.
        #[cfg(all(unix, not(target_os = "macos")))]
        let key = notice.replace.as_deref();
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let key = None;
        let ticket = WAITERS.register(key);
        let _ = std::thread::Builder::new()
            .name("chaat-notify".into())
            .spawn(move || show_and_wait(notice, ticket, tx, waker));
    }

    /// Clicks since the last call.
    pub fn clicks(&self) -> Vec<Clicked> {
        self.clicks_rx
            .lock()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default()
    }

    /// Notices shown so far (recording handles only).
    pub fn recorded(&self) -> Vec<Notice> {
        self.shown.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Records a click as if the person made it.
    pub fn click(&self, clicked: Clicked) {
        let _ = self.clicks_tx.send(clicked);
    }
}

/// Shows one notification, then (while under the cap) waits for what the
/// person does with it. Runs on its own thread; only fixed texts are logged.
fn show_and_wait(notice: Notice, ticket: Ticket, tx: Sender<Clicked>, waker: Option<Waker>) {
    let mut notification = notify_rust::Notification::new();
    notification
        .summary(&notice.title)
        .body(&crate::html::truncate(&notice.body, 240))
        .appname("Chaat");
    #[cfg(target_os = "linux")]
    notification.icon("chaat");
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(key) = &notice.replace {
        notification.id(replace_id(key));
    }
    #[cfg(windows)]
    if APP_ID_REGISTERED.load(Ordering::Acquire) {
        notification.app_id(APP_ID);
    }
    for (id, label) in actions(&notice.buttons, cfg!(all(unix, not(target_os = "macos")))) {
        notification.action(id, label);
    }
    let handle = match notification.show() {
        Ok(handle) => handle,
        Err(error) => {
            log::info!("could not show a notification: {error}");
            WAITERS.forget(&ticket);
            return;
        }
    };
    // Replaced before it could wait, or over the cap: no click handling. A
    // stale toast must not listen at all.
    let Some(slot) = WAITERS.claim(ticket) else {
        return;
    };
    let target = notice.target;
    let _ = handle.wait_for_response(|response: &notify_rust::NotificationResponse| {
        use notify_rust::NotificationResponse as R;
        // wait_for_action reports a Windows body click as closed, so read
        // the typed response and map it to the ids ourselves.
        let action = match response {
            R::Default => "default",
            R::Action(id) => id.as_str(),
            _ => "__closed",
        };
        // A replaced toast shares the id of its successor, so it hears the
        // successor's click too; it must not forward it.
        if !slot.is_current() {
            return;
        }
        if let Some(clicked) = clicked_for(action, &target) {
            let _ = tx.send(clicked);
            if let Some(waker) = &waker {
                waker.wake();
            }
        }
    });
}

/// Registers Chaat's notification identity (Windows): a display name and
/// icon under the toast app id, so toasts say Chaat and not PowerShell. The
/// process's own AppUserModelID is left alone, which keeps taskbar pins.
/// Elsewhere this does nothing.
pub fn register_identity(data_dir: &Path) {
    #[cfg(windows)]
    match windows_identity::register(data_dir) {
        Ok(()) => APP_ID_REGISTERED.store(true, Ordering::Release),
        Err(()) => log::info!("notifications keep the default sender name"),
    }
    #[cfg(not(windows))]
    let _ = data_dir;
}

#[cfg(windows)]
mod windows_identity {
    use std::path::Path;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
        RegCloseKey, RegCreateKeyExW, RegQueryValueExW, RegSetValueExW,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Writes the icon and the two values. Failures carry no detail, since
    /// it could name paths.
    pub fn register(data_dir: &Path) -> Result<(), ()> {
        let icon = data_dir.join("chaat-notification.png");
        write_icon(&icon)?;
        let icon = icon.to_str().ok_or(())?;
        let subkey = wide(&format!(
            "Software\\Classes\\AppUserModelId\\{}",
            super::APP_ID
        ));
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: valid NUL-terminated subkey, null optional pointers, and an
        // out pointer to a local.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(());
        }
        let result = set_if_changed(key, "DisplayName", "Chaat")
            .and_then(|()| set_if_changed(key, "IconUri", icon));
        // SAFETY: `key` was opened above and is closed once.
        unsafe { RegCloseKey(key) };
        result
    }

    fn write_icon(path: &Path) -> Result<(), ()> {
        const SIZE: usize = 256;
        let pixels = crate::platform::icon_rgba(SIZE);
        let image = image::RgbaImage::from_raw(SIZE as u32, SIZE as u32, pixels).ok_or(())?;
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|_| ())?;
        if std::fs::read(path).is_ok_and(|existing| existing == png) {
            return Ok(());
        }
        crate::paths::write_atomically(path, &png).map_err(|_| ())
    }

    fn current(key: HKEY, name: &[u16]) -> Option<String> {
        let mut buffer = vec![0u16; 1024];
        let mut kind = 0;
        let mut bytes = (buffer.len() * 2) as u32;
        // SAFETY: the buffer and its byte length match; the name is NUL-terminated.
        let status = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if status != ERROR_SUCCESS || kind != REG_SZ {
            return None;
        }
        buffer.truncate(bytes as usize / 2);
        while buffer.last() == Some(&0) {
            buffer.pop();
        }
        String::from_utf16(&buffer).ok()
    }

    fn set_if_changed(key: HKEY, name: &str, value: &str) -> Result<(), ()> {
        let name = wide(name);
        if current(key, &name).as_deref() == Some(value) {
            return Ok(());
        }
        let data = wide(value);
        // SAFETY: `data` is a NUL-terminated UTF-16 string and the byte count
        // covers it, including the terminator.
        let status = unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice() -> Notice {
        Notice {
            title: "t".into(),
            body: "b".into(),
            target: Target::Test,
            buttons: vec![Button::Join],
            replace: None,
        }
    }

    #[test]
    fn recording_never_touches_os() {
        let d = Desktop::recording();
        d.show(notice());
        assert_eq!(d.recorded(), vec![notice()]);
        assert!(d.clicks().is_empty());
        d.click(Clicked::Open(Target::Test));
        assert_eq!(d.clicks(), vec![Clicked::Open(Target::Test)]);
        assert!(d.clicks().is_empty());
    }

    #[test]
    fn linux_toasts_declare_a_default_action_first() {
        assert_eq!(
            actions(&[Button::Join, Button::Snooze], true),
            [("default", "Open"), ("join", "Join"), ("snooze", "Snooze")]
        );
        assert_eq!(actions(&[Button::Join], false), [("join", "Join")]);
        assert!(actions(&[], false).is_empty());
        // Whatever comes back for it opens the toast's target.
        assert_eq!(
            clicked_for(actions(&[], true)[0].0, &Target::Test),
            Some(Clicked::Open(Target::Test))
        );
    }

    #[test]
    fn action_ids_map_to_clicked() {
        let t = Target::Test;
        assert_eq!(
            clicked_for("default", &t),
            Some(Clicked::Open(Target::Test))
        );
        assert_eq!(clicked_for("join", &t), Some(Clicked::Join(Target::Test)));
        assert_eq!(
            clicked_for("snooze", &t),
            Some(Clicked::Snooze(Target::Test))
        );
        assert_eq!(clicked_for("__closed", &t), None);
        assert_eq!(clicked_for("other", &t), None);
        assert_eq!(button_id(Button::Join).0, "join");
        assert_eq!(button_id(Button::Snooze).0, "snooze");
    }

    #[test]
    fn wait_cap_is_16() {
        let waiters = Waiters::new(WAIT_CAP);
        assert_eq!(WAIT_CAP, 16);
        let held: Vec<_> = (0..WAIT_CAP)
            .map(|_| {
                waiters
                    .claim(waiters.register(None))
                    .expect("under the cap")
            })
            .collect();
        assert!(waiters.claim(waiters.register(None)).is_none());
        drop(held);
        assert!(waiters.claim(waiters.register(None)).is_some());
        assert_eq!(waiters.lock().waiting(), 0);
    }

    #[test]
    fn current_generation_forwards_and_stale_does_not() {
        let waiters = Waiters::new(WAIT_CAP);
        let first = waiters.claim(waiters.register(Some("chat"))).unwrap();
        assert!(first.is_current());
        let second = waiters.claim(waiters.register(Some("chat"))).unwrap();
        assert!(!first.is_current());
        assert!(second.is_current());
        // Another key is unaffected, and so are notices without one.
        let other = waiters.claim(waiters.register(Some("other"))).unwrap();
        let plain = waiters.claim(waiters.register(None)).unwrap();
        assert!(second.is_current() && other.is_current() && plain.is_current());
        // The stale waiter ending does not disturb the current one.
        drop(first);
        assert!(second.is_current());
        drop(second);
        assert_eq!(waiters.lock().waiting(), 2);
    }

    #[test]
    fn stale_ticket_cannot_claim_or_wait() {
        let waiters = Waiters::new(WAIT_CAP);
        let old = waiters.register(Some("chat"));
        let new = waiters.register(Some("chat"));
        assert!(!waiters.is_current(&old));
        assert!(waiters.claim(old).is_none());
        assert!(waiters.is_current(&new));
        assert_eq!(waiters.lock().waiting(), 0);
    }

    #[test]
    fn stale_waiters_do_not_count_against_the_cap() {
        let waiters = Waiters::new(WAIT_CAP);
        // 40 replacements of one key, every earlier waiter still blocked.
        let guards: Vec<_> = (0..40)
            .map(|_| waiters.claim(waiters.register(Some("chat"))).expect("slot"))
            .collect();
        assert_eq!(waiters.lock().waiting(), 1);
        assert!(guards.last().unwrap().is_current());
        assert!(guards[..39].iter().all(|g| !g.is_current()));
        // Room is left for other chats.
        let others: Vec<_> = (0..WAIT_CAP - 1)
            .map(|i| {
                waiters
                    .claim(waiters.register(Some(&format!("c{i}"))))
                    .expect("slot")
            })
            .collect();
        assert!(waiters.claim(waiters.register(Some("full"))).is_none());
        drop(others);
        drop(guards);
        assert_eq!(waiters.lock().waiting(), 0);
        assert!(waiters.lock().keyed.is_empty());
    }

    #[test]
    fn failed_show_forgets_its_ticket() {
        let waiters = Waiters::new(WAIT_CAP);
        let ticket = waiters.register(Some("chat"));
        waiters.forget(&ticket);
        assert!(waiters.lock().keyed.is_empty());
        // A stale ticket forgetting does not drop its successor.
        let old = waiters.register(Some("chat"));
        let new = waiters.register(Some("chat"));
        waiters.forget(&old);
        assert!(waiters.is_current(&new));
    }

    #[test]
    fn replace_key_gives_stable_nonzero_id() {
        assert_eq!(replace_id("chat-1"), replace_id("chat-1"));
        assert_ne!(replace_id("chat-1"), replace_id("chat-2"));
        assert_ne!(replace_id(""), 0);
    }
}
