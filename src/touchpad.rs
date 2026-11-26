//! Precision touchpad scrolling on Windows.
//!
//! Apps that don't read the touchpad through DirectManipulation (winit
//! doesn't) get it from Windows as wheel messages with fractional line
//! deltas. While the fingers move, small deltas come every frame; the
//! momentum after they lift comes as one large message about 350 ms later.
//! The view stopped, then jumped by the whole momentum at once.
//!
//! [`Touchpad`] rewrites each frame's wheel events before egui reads them:
//!
//! - fractional line deltas become point deltas inside a touch phase, so egui
//!   applies them as they come, without its wheel smoothing and without the
//!   mouse-wheel step `fastframe_scroll` gives lines;
//! - when the input goes quiet, the fingers have lifted: the view glides on
//!   from their speed and slows down;
//! - the late momentum message that follows is not scrolled at once. It says
//!   how far Windows coasts, as apps reading the touchpad directly (Firefox)
//!   do: the glide under way eases out to end there, and the ratio of that
//!   distance to the glide's own estimate calibrates the next glides, so
//!   they follow the system's momentum on any touchpad and its settings;
//! - pointer movement is held back until the scroll and its glide end, so
//!   the glide stays on the list it started on (egui scrolls whatever is
//!   under the pointer). A click first moves the pointer to where it is.
//!   A glide that reaches the end of its list stops there.
//!
//! A mouse wheel's notches are whole lines and pass through untouched.

use egui::{Event, Modifiers, MouseWheelUnit, TouchPhase, Vec2};

/// Points one line of touchpad movement scrolls: half egui's line step, set
/// by feel on an ASUS Precision Touchpad.
pub const LINE_POINTS: f32 = 20.0;

/// Input quiet this long, in seconds, means the fingers lifted. Moving
/// fingers send a delta every frame or two.
const LIFT_GAP: f64 = 0.07;
/// How long after the last movement, in seconds, the momentum message may
/// still arrive. Measured arrivals: 0.33 to 0.43 s.
const BURST_WINDOW: f64 = 0.7;
/// The glide's exponential decay time, in seconds, as fastframe's Linux
/// glide. It travels its starting speed times this.
const GLIDE_DECAY: f32 = 0.35;
/// Bounds of the decay time when a glide eases out to the system's distance.
const EASE_DECAY: std::ops::RangeInclusive<f32> = 0.08..=1.0;
/// Bounds of the learned ratio of the system's momentum to the glide's own
/// estimate, and how much each momentum message moves it.
const CALIBRATION: std::ops::RangeInclusive<f32> = 0.25..=2.0;
const CALIBRATION_RATE: f32 = 0.5;
/// The release speed below which lifting starts no glide, points per second.
const GLIDE_START: f32 = 120.0;
/// The speed at which a glide stops, points per second.
const GLIDE_STOP: f32 = 40.0;
/// The span of movement, in seconds, the release speed is measured over.
const SPEED_SPAN: f32 = 0.05;

/// Touchpad state for one window. Feed it each frame's raw input with
/// [`process`](Self::process).
#[derive(Debug)]
pub struct Touchpad {
    /// egui was sent a touch start and no end yet.
    touching: bool,
    /// Recent positions of the gesture, for its speed at the lift.
    history: egui::util::History<Vec2>,
    /// Where the gesture has scrolled to so far, in points.
    travelled: Vec2,
    /// Input time of the gesture's latest movement.
    last_movement: Option<f64>,
    /// The latest movement, for the direction of a lift without speed.
    last_delta: Vec2,
    /// The speed still carrying the view after the lift, points per second.
    glide: Option<Vec2>,
    /// The momentum message expected after the lift.
    burst: Option<Burst>,
    /// The largest message of the gesture, in lines: a momentum message is
    /// larger.
    peak: f32,
    /// How far the glide has carried the view since the lift, in points.
    coasted: f32,
    /// The glide's decay time, in seconds.
    decay: f32,
    /// The learned ratio of the system's momentum distance to the glide's
    /// estimate from the release speed.
    calibration: f32,
    /// Input time of the previous frame.
    last_frame: Option<f64>,
    modifiers: Modifiers,
    /// The latest pointer movement, held back from egui while scrolling.
    held_pointer: Option<Event>,
    /// This frame's events carry a glide step.
    glided: bool,
    /// Nothing took the last glide step: end the touch next frame.
    stalled: bool,
}

impl Default for Touchpad {
    fn default() -> Self {
        Self {
            touching: false,
            history: egui::util::History::new(2..32, SPEED_SPAN),
            travelled: Vec2::ZERO,
            last_movement: None,
            last_delta: Vec2::ZERO,
            glide: None,
            burst: None,
            peak: 0.0,
            coasted: 0.0,
            decay: GLIDE_DECAY,
            calibration: 1.0,
            last_frame: None,
            modifiers: Modifiers::NONE,
            held_pointer: None,
            glided: false,
            stalled: false,
        }
    }
}

impl Touchpad {
    /// Rewrites one frame's events (see the module docs). `now` is the
    /// frame's input time in seconds. Returns whether another frame is
    /// needed soon: to notice the lift, or to move the glide on.
    pub fn process(&mut self, events: &mut Vec<Event>, now: f64) -> bool {
        let dt = self
            .last_frame
            .map_or(1.0 / 60.0, |last| (now - last) as f32)
            .clamp(0.001, 0.05);
        self.last_frame = Some(now);

        let mut out = Vec::with_capacity(events.len() + 2);
        self.glided = false;
        // Before this frame's movement: an end clears egui's delta.
        if std::mem::take(&mut self.stalled) {
            self.end(&mut out);
        }
        let mut moved = false;
        let mut pressed = false;
        for event in events.drain(..) {
            match event {
                Event::MouseWheel {
                    unit: MouseWheelUnit::Line,
                    delta,
                    phase: TouchPhase::Move,
                    modifiers,
                } => {
                    if let Some(burst) = self.burst.filter(|b| b.matches(delta, now)) {
                        self.burst = None;
                        self.follow_momentum(&burst, delta.length() * LINE_POINTS);
                        continue;
                    }
                    if !self.touching && !fractional(delta) {
                        // A mouse wheel notch.
                        out.push(event);
                        continue;
                    }
                    self.burst = None;
                    self.glide = None;
                    if !self.touching || self.last_movement.is_none() {
                        if !self.touching {
                            out.push(wheel(TouchPhase::Start, Vec2::ZERO, modifiers));
                            self.touching = true;
                        }
                        self.history.clear();
                        self.travelled = Vec2::ZERO;
                        self.peak = 0.0;
                    }
                    self.peak = self.peak.max(delta.length());
                    let points = delta * LINE_POINTS;
                    self.travelled += points;
                    self.history.add(now, self.travelled);
                    self.last_movement = Some(now);
                    self.last_delta = points;
                    self.modifiers = modifiers;
                    moved = true;
                    out.push(wheel(TouchPhase::Move, points, modifiers));
                }
                // egui scrolls whatever is under the pointer: until the
                // scroll and its glide end, it keeps the pointer where the
                // scroll started.
                Event::PointerMoved(_) | Event::PointerGone if self.touching => {
                    self.held_pointer = Some(event);
                }
                Event::PointerButton { pressed: down, .. } => {
                    // A click lands where the pointer is now.
                    out.extend(self.held_pointer.take());
                    pressed |= down;
                    out.push(event);
                }
                other => out.push(other),
            }
        }

        let lifted = !moved && self.last_movement.is_some_and(|last| now - last > LIFT_GAP);
        if lifted {
            self.release();
        }
        if pressed && self.glide.take().is_some() {
            self.end(&mut out);
        }
        if let Some(speed) = self.glide
            && !moved
        {
            let slower = speed * (-dt / self.decay).exp();
            if slower.length() > GLIDE_STOP {
                out.push(wheel(TouchPhase::Move, speed * dt, self.modifiers));
                self.coasted += (speed * dt).length();
                self.glide = Some(slower);
                self.glided = true;
            } else {
                // Its own frame: an end clears egui's delta for the frame.
                self.glide = None;
                self.end(&mut out);
            }
        } else if lifted && self.glide.is_none() {
            self.end(&mut out);
        }
        *events = out;
        self.glide.is_some() || self.last_movement.is_some()
    }

    /// The learned ratio of the system's momentum to the glide's estimate.
    #[must_use]
    pub fn calibration(&self) -> f32 {
        self.calibration
    }

    /// Call after the frame's interface ran, with the scroll delta left in
    /// egui's input. A glide step nobody took (the list is at its end) ends
    /// the glide, so the pointer is not held while nothing moves. Returns
    /// whether a frame is needed to end the touch.
    pub fn after_frame(&mut self, unconsumed: Vec2) -> bool {
        if self.glided && unconsumed != Vec2::ZERO {
            self.glide = None;
            self.stalled = true;
        }
        self.stalled
    }

    /// The fingers lifted: glide on from their speed, and watch for the
    /// momentum message.
    fn release(&mut self) {
        let Some(last) = self.last_movement.take() else {
            return;
        };
        let raw = self.history.velocity().unwrap_or(Vec2::ZERO);
        let direction = if raw == Vec2::ZERO {
            self.last_delta
        } else {
            raw
        };
        let speed = raw * self.calibration;
        self.burst = Some(Burst {
            until: last + BURST_WINDOW,
            direction,
            peak: self.peak,
            estimate: raw.length() * GLIDE_DECAY,
        });
        self.glide = (speed.length() > GLIDE_START).then_some(speed);
        self.decay = GLIDE_DECAY;
        self.coasted = 0.0;
        self.history.clear();
        self.travelled = Vec2::ZERO;
    }

    /// The system's momentum arrived, `distance` points in all: learn how
    /// it compares with the glide's estimate, and let the glide under way
    /// ease out to end there without changing its speed.
    fn follow_momentum(&mut self, burst: &Burst, distance: f32) {
        if burst.estimate * self.calibration > GLIDE_START * GLIDE_DECAY {
            let ratio = (distance / burst.estimate).clamp(*CALIBRATION.start(), *CALIBRATION.end());
            self.calibration += (ratio - self.calibration) * CALIBRATION_RATE;
        }
        if let Some(speed) = self.glide {
            let remaining = (distance - self.coasted).max(0.0);
            self.decay = (remaining / speed.length()).clamp(*EASE_DECAY.start(), *EASE_DECAY.end());
        }
    }

    fn end(&mut self, out: &mut Vec<Event>) {
        if self.touching {
            out.push(wheel(TouchPhase::End, Vec2::ZERO, self.modifiers));
            self.touching = false;
        }
        out.extend(self.held_pointer.take());
    }
}

/// What a lift leaves to recognise and use the momentum message by.
#[derive(Clone, Copy, Debug)]
struct Burst {
    /// Input time after which it no longer comes.
    until: f64,
    direction: Vec2,
    /// The gesture's largest message, in lines.
    peak: f32,
    /// The glide distance the release speed predicts before calibration,
    /// in points.
    estimate: f32,
}

impl Burst {
    /// One message, in the gesture's direction, larger than any the moving
    /// fingers sent.
    fn matches(&self, delta: Vec2, now: f64) -> bool {
        now <= self.until && delta.dot(self.direction) > 0.0 && delta.length() > self.peak
    }
}

fn fractional(delta: Vec2) -> bool {
    delta.x.fract().abs() > 1e-3 || delta.y.fract().abs() > 1e-3
}

fn wheel(phase: TouchPhase, delta: Vec2, modifiers: Modifiers) -> Event {
    Event::MouseWheel {
        unit: if phase == TouchPhase::Move {
            MouseWheelUnit::Point
        } else {
            MouseWheelUnit::Line
        },
        delta,
        phase,
        modifiers,
    }
}

#[cfg(test)]
mod tests;
