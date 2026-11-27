# Notifications

Chaat shows desktop notifications (toasts) for new messages, meeting
reminders and, if you ask, when a person becomes available. Settings ›
Notifications holds the controls. Chaat does not fetch anything to make a
notification: it uses messages and calendars it already has, from a fetch you
caused or a push update ([realtime.md](realtime.md)).

## When a message notifies

A new message in a chat or channel notifies unless one of these applies, in
this order:

1. **Master switch off**: the first checkbox under Notifications, "Desktop
   notifications for new messages and meeting reminders".
2. **Paused**: see Pause below.
3. **You are looking at it**: Chaat is the focused window and that chat is
   open.
4. **Urgent messages** (importance set to urgent in Teams) stop here and
   notify. They get through Do not disturb and quiet during calls, never
   rules 1 to 3.
5. **Do not disturb**: under "When to stay quiet", on by default. Applies
   while your Teams status is Do not disturb.
6. **During calls and meetings**: under "When to stay quiet", off by default.
   Applies while your presence shows a call or meeting, or while Chaat has a
   call open.
7. **Mentions**: a message that mentions you notifies. "Everyone" mentions
   count. A mention also gets through a local mute.
8. **Muted chat**: no notification.
9. **Chat type**:
   - One-on-one chats and channels always notify.
   - Group chats follow "Group chats": All messages or Mentions only.
   - Meeting chats follow "Meeting chats": All messages, After I take part,
     or Mentions only. "Take part" means you sent a message in that meeting
     chat (one Chaat has cached), or you pressed Join on its reminder in
     Chaat since Chaat started.

Messages that arrive together in one chat make one toast, with the newest
message and "(+N more)". Chaat does not notify the same message twice.

With "Show message preview" off, toasts say "New message" or "N new
messages" instead of the text. With several accounts, the title adds the
account's sign-in name.

## Pause

Pause silences message toasts, meeting reminders and "notify when
available" for a while: 1 hour, 2 hours, or until tomorrow at 08:00 local
time. Use Settings › Notifications › Pause, or the account menu (Pause
notifications for 1 hour, 2 hours or until tomorrow, then Resume
notifications). The end time is saved in the settings file, so a pause
survives a restart. Urgent messages do not get through a pause.

## Mute

"Mute notifications" in a chat's right-click menu (Unmute notifications to
undo) mutes that chat on this device only. It does not sync with Teams, and
Chaat cannot read Teams' own mute settings. A muted chat shows a small muted
bell in the list. Mentions and urgent messages still notify.

## Urgent messages

An urgent message repeats every 2 minutes for 20 minutes, until the chat is
read (opened, or marked read here or in another client that a fetch picks
up). Turn this off with "Repeat urgent messages until read". Repeats show
the cached message again and fetch nothing. They respect the master switch
and Pause.

## Taskbar, click and reminders

- **Taskbar**: while Chaat is not focused, a toast also asks the taskbar to
  flash. Turn this off with "Flash the taskbar".
- **Click**: clicking a toast brings Chaat forward, switches to the account
  it was for, and opens the chat at that message. Clicking a meeting
  reminder opens the calendar. Notices for a signed-out account are ignored.
- **Meeting reminders** appear 5 minutes before a meeting starts. They have
  Join (only when the meeting has a Teams link) and Snooze. Snooze shows the
  reminder again at the meeting's start; once the meeting has started there
  is no Snooze. Reminders follow the master switch and Pause, not Do not
  disturb or the chat rules.
- **Send test notification** shows a toast to check the setup.

## Notify when available

Right-click a one-on-one chat and choose "Notify when available" ("Stop
notifying when available" to undo). Chaat then shows "<name> is available"
when that person's presence changes from something else to Available. It
lasts until you turn it off, here or under Settings › Notifications ›
People (Turn off). It needs push updates ([realtime.md](realtime.md)), so the
menu entry appears only while push is live, and a note says so on the page.
It follows the master switch and Pause, and Do not disturb when that rule is
on. Chaat stores the person's ID and name in the settings file.

## What Graph cannot tell Chaat

- Notices that come from the chat list carry no mention or importance
  information. Chaat treats them as not mentioning you and not urgent, so
  mentions-only chats and urgent repeats work only for messages Chaat has
  fetched in full.
- Teams' mute and notification settings are not readable through Graph, so
  Chaat's mute is separate from Teams'.

## Platform notes

- **Windows**: toasts show as "Chaat" with Chaat's icon. For that, Chaat
  writes a per-user registry key and an icon file; see
  [privacy.md](privacy.md) for the names and how to remove them. A click on
  a toast that has already left the screen and sits in Action Center does
  not reach Chaat. Chaat keeps at most 16 toasts waiting for a click; past
  that, toasts show without click handling.
- **Linux**: a chat's new toast replaces its earlier one. Buttons and clicks
  depend on the desktop's notification daemon; it needs one running on the
  session bus (GNOME and KDE have their own; dunst or mako elsewhere).
- **macOS**: toasts come from the notification center. Chaat claims its
  own identity there only when it runs from an installed app bundle.
- **Tested**: headless tests cover the rules and settings page, and the
  feature was run on Windows, and on Linux in WSL (Debian 13 with dunst):
  toasts show there. macOS has not been compiled yet.
