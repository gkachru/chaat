use super::*;

const FRAME: f64 = 1.0 / 60.0;

fn line(y: f32) -> Event {
    Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: Vec2::new(0.0, y),
        phase: TouchPhase::Move,
        modifiers: Modifiers::NONE,
    }
}

/// Runs frames from `start`, one per entry, each with its wheel deltas in
/// lines, and returns every frame's output events.
fn run(pad: &mut Touchpad, start: f64, frames: &[&[f32]]) -> Vec<Vec<Event>> {
    frames
        .iter()
        .enumerate()
        .map(|(index, deltas)| {
            let mut events: Vec<Event> = deltas.iter().map(|&y| line(y)).collect();
            pad.process(&mut events, start + index as f64 * FRAME);
            events
        })
        .collect()
}

/// Quiet frames from `start` until `until`, returned per frame.
fn quiet(pad: &mut Touchpad, start: f64, until: f64) -> Vec<Vec<Event>> {
    let count = ((until - start) / FRAME).ceil() as usize;
    run(pad, start, &vec![&[][..]; count])
}

fn moved(events: &[Event]) -> f32 {
    events
        .iter()
        .map(|event| match event {
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta,
                phase: TouchPhase::Move,
                ..
            } => delta.y,
            Event::MouseWheel {
                unit: MouseWheelUnit::Line,
                delta,
                ..
            } => delta.y * 1000.0, // A line delta reaching egui is a failure here.
            _ => 0.0,
        })
        .sum()
}

fn phases(frames: &[Vec<Event>]) -> Vec<TouchPhase> {
    frames
        .iter()
        .flatten()
        .filter_map(|event| match event {
            Event::MouseWheel { phase, .. } if *phase != TouchPhase::Move => Some(*phase),
            _ => None,
        })
        .collect()
}

#[test]
fn mouse_wheel_notches_pass_through() {
    let mut pad = Touchpad::default();
    let frames = run(&mut pad, 0.0, &[&[1.0], &[], &[-3.0]]);
    assert_eq!(frames[0], vec![line(1.0)]);
    assert_eq!(frames[2], vec![line(-3.0)]);
    assert!(phases(&frames).is_empty());
}

#[test]
fn touchpad_moves_in_points_inside_a_touch() {
    let mut pad = Touchpad::default();
    let frames = run(&mut pad, 0.0, &[&[-0.8], &[-1.3]]);
    assert_eq!(phases(&frames), vec![TouchPhase::Start]);
    assert!((moved(&frames[0]) - -0.8 * LINE_POINTS).abs() < 1e-3);
    assert!((moved(&frames[1]) - -1.3 * LINE_POINTS).abs() < 1e-3);
}

/// A flick recorded on an ASUS Precision Touchpad: movement every frame,
/// then nothing for 0.37 s, then the momentum in one message.
#[test]
fn a_flick_glides_without_the_late_jump() {
    let mut pad = Touchpad::default();
    let mut frames = run(
        &mut pad,
        0.0,
        &[&[-1.4], &[-0.8], &[-3.7], &[-4.6], &[-4.5, -4.0]],
    );
    let lifted_at = 4.0 * FRAME;
    let glide = quiet(&mut pad, 5.0 * FRAME, lifted_at + 0.37);
    // The glide starts at the lift and moves every frame until it stops.
    let first = glide
        .iter()
        .position(|events| moved(events) != 0.0)
        .expect("the flick glides");
    assert!(first as f64 * FRAME <= LIFT_GAP + FRAME);
    let stopped = glide[first..]
        .iter()
        .position(|events| moved(events) == 0.0)
        .map_or(glide.len(), |at| first + at);
    assert!(
        glide[first..stopped]
            .iter()
            .all(|events| moved(events) < 0.0)
    );
    let glided: f32 = glide.iter().map(|events| moved(events)).sum();
    assert!(glided < -500.0, "glided {glided}");

    // The momentum message is dropped: the frame only takes its glide step.
    let late = run(&mut pad, lifted_at + 0.375, &[&[-40.8]]);
    assert!(moved(&late[0]) > -100.0, "moved {}", moved(&late[0]));
    frames.extend(glide);
    frames.extend(late);
    // The glide slows down and stops.
    let rest = quiet(&mut pad, lifted_at + 0.4, lifted_at + 4.0);
    assert!(rest.iter().all(|events| moved(events) <= 0.0));
    assert!(
        rest.iter()
            .rev()
            .take(10)
            .all(|events| moved(events) == 0.0)
    );
    frames.extend(rest);
    assert_eq!(phases(&frames), vec![TouchPhase::Start, TouchPhase::End]);
}

#[test]
fn stopping_before_lifting_starts_no_glide() {
    let mut pad = Touchpad::default();
    let mut frames = run(
        &mut pad,
        0.0,
        &[
            &[-2.1],
            &[-1.1],
            &[-0.4],
            &[-0.2],
            &[-0.1],
            &[-0.05],
            &[-0.03],
            &[-0.02],
            &[-0.01],
        ],
    );
    let after = quiet(&mut pad, 9.0 * FRAME, 1.0);
    assert!(after.iter().all(|events| moved(events) == 0.0));
    frames.extend(after);
    assert_eq!(phases(&frames), vec![TouchPhase::Start, TouchPhase::End]);
}

#[test]
fn a_new_swipe_the_other_way_is_kept() {
    let mut pad = Touchpad::default();
    run(&mut pad, 0.0, &[&[-3.2], &[-6.1], &[-8.3]]);
    quiet(&mut pad, 3.0 * FRAME, 0.2);
    let swipe = run(&mut pad, 0.2, &[&[3.4], &[15.0]]);
    assert!((moved(&swipe[0]) - 3.4 * LINE_POINTS).abs() < 1e-3);
    assert!((moved(&swipe[1]) - 15.0 * LINE_POINTS).abs() < 1e-3);
}

#[test]
fn a_late_message_after_the_window_is_scrolled() {
    let mut pad = Touchpad::default();
    run(&mut pad, 0.0, &[&[-3.2], &[-6.1], &[-8.3]]);
    quiet(&mut pad, 3.0 * FRAME, 2.0);
    let late = run(&mut pad, 2.0, &[&[-20.5]]);
    assert!((moved(&late[0]) - -20.5 * LINE_POINTS).abs() < 1e-3);
}

#[test]
fn a_click_stops_the_glide() {
    let mut pad = Touchpad::default();
    run(&mut pad, 0.0, &[&[-3.2], &[-6.1], &[-8.3]]);
    let glide = quiet(&mut pad, 3.0 * FRAME, 0.15);
    assert!(glide.iter().any(|events| moved(events) != 0.0));
    let mut events = vec![Event::PointerButton {
        pos: egui::Pos2::ZERO,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }];
    pad.process(&mut events, 0.15 + FRAME);
    assert_eq!(moved(&events), 0.0);
    assert_eq!(phases(&[events]), vec![TouchPhase::End]);
    let after = quiet(&mut pad, 0.15 + 2.0 * FRAME, 0.5);
    assert!(after.iter().all(|events| moved(events) == 0.0));
}

fn pointer_to(x: f32) -> Event {
    Event::PointerMoved(egui::pos2(x, 0.0))
}

#[test]
fn the_glide_stays_where_it_started() {
    let mut pad = Touchpad::default();
    run(&mut pad, 0.0, &[&[-3.2], &[-6.1], &[-8.3]]);
    // Moving the pointer to another list mid-glide reaches egui only when
    // the glide ends.
    let mut events = vec![pointer_to(10.0)];
    pad.process(&mut events, 0.1);
    let mut events2 = vec![pointer_to(20.0)];
    pad.process(&mut events2, 0.1 + FRAME);
    assert!(
        !events
            .iter()
            .chain(&events2)
            .any(|e| matches!(e, Event::PointerMoved(_)))
    );
    let rest = quiet(&mut pad, 0.1 + 2.0 * FRAME, 4.0);
    let pointers: Vec<_> = rest
        .iter()
        .flatten()
        .filter(|e| matches!(e, Event::PointerMoved(_)))
        .collect();
    assert_eq!(pointers, vec![&pointer_to(20.0)]);
    // After the glide, the pointer moves freely.
    let mut events = vec![pointer_to(30.0)];
    pad.process(&mut events, 4.1);
    assert_eq!(events, vec![pointer_to(30.0)]);
}

#[test]
fn a_click_during_the_glide_lands_where_the_pointer_is() {
    let mut pad = Touchpad::default();
    run(&mut pad, 0.0, &[&[-3.2], &[-6.1], &[-8.3]]);
    quiet(&mut pad, 3.0 * FRAME, 0.15);
    let mut events = vec![pointer_to(50.0)];
    pad.process(&mut events, 0.15);
    let click = Event::PointerButton {
        pos: egui::pos2(50.0, 0.0),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    };
    let mut events = vec![click.clone()];
    pad.process(&mut events, 0.15 + FRAME);
    assert_eq!(events[0], pointer_to(50.0));
    assert_eq!(events[1], click);
}

#[test]
fn the_pointer_moves_freely_without_a_touchpad_scroll() {
    let mut pad = Touchpad::default();
    let mut events = vec![line(1.0), pointer_to(5.0)];
    pad.process(&mut events, 0.0);
    assert_eq!(events, vec![line(1.0), pointer_to(5.0)]);
}

/// At the end of a list nothing takes the glide's steps: the glide stops
/// and the pointer moves again at once.
#[test]
fn a_glide_at_the_end_of_the_list_stops() {
    let mut pad = Touchpad::default();
    run(&mut pad, 0.0, &[&[3.2], &[6.1], &[8.3]]);
    // Frames until the glide starts; each step is taken.
    let mut now = 3.0 * FRAME;
    loop {
        let mut events = Vec::new();
        pad.process(&mut events, now);
        now += FRAME;
        if moved(&events) != 0.0 {
            // This step is not taken.
            assert!(pad.after_frame(Vec2::new(0.0, moved(&events))));
            break;
        }
        assert!(!pad.after_frame(Vec2::ZERO));
    }
    let mut events = vec![pointer_to(40.0)];
    pad.process(&mut events, now);
    assert_eq!(phases(std::slice::from_ref(&events)), vec![TouchPhase::End]);
    assert!(events.contains(&pointer_to(40.0)));
    assert_eq!(moved(&events), 0.0);
    let rest = quiet(&mut pad, now + FRAME, now + 1.0);
    assert!(rest.iter().all(|events| moved(events) == 0.0));
}

/// A glide whose steps are taken keeps going.
#[test]
fn a_taken_glide_step_keeps_the_glide() {
    let mut pad = Touchpad::default();
    run(&mut pad, 0.0, &[&[3.2], &[6.1], &[8.3]]);
    let glide = quiet(&mut pad, 3.0 * FRAME, 0.2);
    assert!(glide.iter().any(|events| moved(events) != 0.0));
    assert!(!pad.after_frame(Vec2::ZERO));
    let mut events = Vec::new();
    pad.process(&mut events, 0.2);
    assert!(moved(&events) > 0.0);
}

/// Glides from a gentle flick, then the system's momentum message at 0.37 s.
/// Returns the distance glided after the lift, in points.
fn flick_with_momentum(pad: &mut Touchpad, start: f64, momentum: f32) -> f32 {
    let mut glided: f32 = run(pad, start, &[&[0.6], &[0.7], &[0.8]])
        .iter()
        .skip(3)
        .map(|events| moved(events))
        .sum();
    let lifted_at = start + 2.0 * FRAME;
    glided += quiet(pad, start + 3.0 * FRAME, lifted_at + 0.37)
        .iter()
        .map(|events| moved(events))
        .sum::<f32>();
    let late = run(pad, lifted_at + 0.37, &[&[momentum]]);
    glided += moved(&late[0]);
    glided += quiet(pad, lifted_at + 0.37 + FRAME, lifted_at + 5.0)
        .iter()
        .map(|events| moved(events))
        .sum::<f32>();
    glided
}

#[test]
fn the_glide_eases_out_to_the_systems_momentum() {
    let mut pad = Touchpad::default();
    let momentum = 25.0;
    let glided = flick_with_momentum(&mut pad, 0.0, momentum);
    let target = momentum * LINE_POINTS;
    assert!(
        (glided - target).abs() < target * 0.15,
        "glided {glided}, the system's momentum {target}"
    );
}

#[test]
fn glides_learn_the_systems_momentum() {
    let mut pad = Touchpad::default();
    let first_step = |pad: &mut Touchpad, start: f64| {
        run(pad, start, &[&[0.6], &[0.7], &[0.8]]);
        quiet(pad, start + 3.0 * FRAME, start + 0.2)
            .iter()
            .map(|events| moved(events))
            .find(|step| *step != 0.0)
            .unwrap()
    };
    let before = first_step(&mut pad, 0.0);
    // The system coasts much further than the glide estimated.
    let late = run(&mut pad, 2.0 * FRAME + 0.37, &[&[40.0]]);
    assert!(
        moved(&late[0]).abs() < 100.0,
        "the momentum is not scrolled at once"
    );
    quiet(&mut pad, 0.5, 6.0);
    assert!(pad.calibration() > 1.2, "{}", pad.calibration());
    let after = first_step(&mut pad, 10.0);
    assert!(after > before * 1.2, "{before} then {after}");
}

#[test]
fn calibration_stays_in_bounds() {
    let mut pad = Touchpad::default();
    for round in 0..10 {
        flick_with_momentum(&mut pad, f64::from(round) * 10.0, 200.0);
    }
    assert!(pad.calibration() <= *CALIBRATION.end());
    let mut pad = Touchpad::default();
    for round in 0..10 {
        flick_with_momentum(&mut pad, f64::from(round) * 10.0, 0.81);
    }
    assert!(pad.calibration() >= *CALIBRATION.start());
}
