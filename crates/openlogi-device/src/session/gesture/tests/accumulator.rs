//! The capture accumulator: presses, taps, swipes and raw XY across the gesture sources.

use super::*;
use crate::session::gesture::accum::FirstRawXyPolicy;

const GESTURE: &[u16] = &[reprog_controls::GESTURE_BUTTON_CID];

const PANEL: &[u16] = &[reprog_controls::HAPTIC_PANEL_CID];

const BOTH: &[u16] = &[
    reprog_controls::GESTURE_BUTTON_CID,
    reprog_controls::HAPTIC_PANEL_CID,
];

fn press() -> RawControlEvent {
    RawControlEvent::DivertedButtons([reprog_controls::GESTURE_BUTTON_CID, 0, 0, 0])
}

fn panel_press() -> RawControlEvent {
    RawControlEvent::DivertedButtons([reprog_controls::HAPTIC_PANEL_CID, 0, 0, 0])
}

fn both_press() -> RawControlEvent {
    RawControlEvent::DivertedButtons([
        reprog_controls::GESTURE_BUTTON_CID,
        reprog_controls::HAPTIC_PANEL_CID,
        0,
        0,
    ])
}

fn release() -> RawControlEvent {
    RawControlEvent::DivertedButtons([0, 0, 0, 0])
}

/// Read the next completed gesture while leaving lifecycle assertions to the
/// dedicated edge tests below.
fn next_gesture(
    rx: &mut mpsc::UnboundedReceiver<CapturedInput>,
) -> Result<CapturedInput, mpsc::error::TryRecvError> {
    loop {
        let input = rx.try_recv()?;
        if matches!(input, CapturedInput::Gesture(..)) {
            return Ok(input);
        }
    }
}

#[test]
fn a_still_held_second_source_takes_over_when_the_holder_releases() {
    // Both sources diverted: press the gesture button, add the panel, release
    // the gesture button (click — no swipe committed), and the still-held
    // panel must become the new holder so its subsequent swipe dispatches —
    // not be swallowed until its own release.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();

    handle_reprog(&mut acc, press(), BOTH, &[], &[], &tx);
    handle_reprog(&mut acc, both_press(), BOTH, &[], &[], &tx);
    handle_reprog(&mut acc, panel_press(), BOTH, &[], &[], &tx);
    assert_eq!(
        next_gesture(&mut rx),
        Ok(CapturedInput::Gesture(
            ButtonId::GestureButton,
            GestureDirection::Click
        )),
        "the released holder still clicks"
    );

    acc.backdate_hold_for_test();
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: 120, dy: 5 },
        BOTH,
        &[],
        &[],
        &tx,
    );
    assert_eq!(
        next_gesture(&mut rx),
        Ok(CapturedInput::Gesture(
            ButtonId::HapticPanel,
            GestureDirection::Right
        )),
        "the taken-over hold dispatches through the panel's own map"
    );

    handle_reprog(&mut acc, release(), BOTH, &[], &[], &tx);
    assert!(
        next_gesture(&mut rx).is_err(),
        "a committed takeover swipe must not also click on release"
    );
}

#[test]
fn raw_xy_during_a_two_source_overlap_is_dropped_not_misattributed() {
    // Raw-XY reports carry no source attribution: while BOTH sources are held,
    // motion must not commit through the first holder's map (the reports could
    // as well be the other control's). Motion resumes once the overlap ends.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();

    handle_reprog(&mut acc, press(), BOTH, &[], &[], &tx);
    acc.backdate_hold_for_test();
    handle_reprog(&mut acc, both_press(), BOTH, &[], &[], &tx);
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: 120, dy: 5 },
        BOTH,
        &[],
        &[],
        &tx,
    );
    assert!(
        next_gesture(&mut rx).is_err(),
        "ambiguous overlap motion must not commit a swipe"
    );

    // The panel lifts; the surviving hold accumulates again.
    handle_reprog(&mut acc, press(), BOTH, &[], &[], &tx);
    acc.backdate_hold_for_test();
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: 120, dy: 5 },
        BOTH,
        &[],
        &[],
        &tx,
    );
    assert_eq!(
        next_gesture(&mut rx),
        Ok(CapturedInput::Gesture(
            ButtonId::GestureButton,
            GestureDirection::Right
        )),
        "the original hold resumes once the overlap ends"
    );
}

#[test]
fn a_same_report_swap_to_the_panel_still_discards_its_contact_jump() {
    // Holder release and panel press arriving in ONE report: the takeover must
    // treat the panel as freshly touched, so its first raw-XY sample (the
    // absolute contact jump) is discarded before the accumulator sees it.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();

    handle_reprog(&mut acc, press(), BOTH, &[], &[], &tx);
    handle_reprog(&mut acc, panel_press(), BOTH, &[], &[], &tx);
    assert_eq!(
        next_gesture(&mut rx),
        Ok(CapturedInput::Gesture(
            ButtonId::GestureButton,
            GestureDirection::Click
        )),
        "the swapped-out holder still clicks"
    );

    acc.backdate_hold_for_test();
    // The contact jump — leftward, far past every threshold — must be dropped.
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: -3000, dy: 40 },
        BOTH,
        &[],
        &[],
        &tx,
    );
    assert!(
        next_gesture(&mut rx).is_err(),
        "the panel's contact jump must not commit a swipe"
    );
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: 120, dy: 5 },
        BOTH,
        &[],
        &[],
        &tx,
    );
    assert_eq!(
        next_gesture(&mut rx),
        Ok(CapturedInput::Gesture(
            ButtonId::HapticPanel,
            GestureDirection::Right
        ))
    );
}

#[test]
fn dedicated_quick_swipe_is_not_misclassified_as_click() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();

    handle_reprog(&mut acc, press(), GESTURE, &[], &[], &tx);
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: 120, dy: 5 },
        GESTURE,
        &[],
        &[],
        &tx,
    );
    handle_reprog(&mut acc, release(), GESTURE, &[], &[], &tx);

    assert_eq!(
        next_gesture(&mut rx),
        Ok(CapturedInput::Gesture(
            ButtonId::GestureButton,
            GestureDirection::Right
        ))
    );
    assert!(
        next_gesture(&mut rx).is_err(),
        "a quick swipe emits exactly one direction, without a release click"
    );
}

#[test]
fn a_held_gesture_commits_a_swipe_and_does_not_also_click() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();

    handle_reprog(&mut acc, press(), GESTURE, &[], &[], &tx);
    // Pretend the button has been held well past the swipe gate.
    acc.backdate_hold_for_test();
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: 120, dy: 5 },
        GESTURE,
        &[],
        &[],
        &tx,
    );

    assert_eq!(
        next_gesture(&mut rx),
        Ok(CapturedInput::Gesture(
            ButtonId::GestureButton,
            GestureDirection::Right
        ))
    );

    handle_reprog(&mut acc, release(), GESTURE, &[], &[], &tx);
    assert!(
        next_gesture(&mut rx).is_err(),
        "a committed swipe must not also click on release"
    );
}

#[test]
fn the_haptic_panel_gestures_when_diverted_for_gestures() {
    // On MX Master 4 the panel (CID 0x01a0) can gesture: its press begins a
    // hold, its contact jump is discarded, and the raw-XY that follows
    // commits a swipe.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();

    handle_reprog(&mut acc, panel_press(), PANEL, &[], &[], &tx);
    acc.backdate_hold_for_test();
    // The panel's contact jump, discarded before the accumulator sees it.
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: -3000, dy: 40 },
        PANEL,
        &[],
        &[],
        &tx,
    );
    // The real swipe that follows.
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: 5, dy: -120 },
        PANEL,
        &[],
        &[],
        &tx,
    );

    assert_eq!(
        next_gesture(&mut rx),
        Ok(CapturedInput::Gesture(
            ButtonId::HapticPanel,
            GestureDirection::Up
        ))
    );

    handle_reprog(&mut acc, release(), PANEL, &[], &[], &tx);
    assert!(
        next_gesture(&mut rx).is_err(),
        "a committed panel swipe must not also click on release"
    );
}

#[test]
fn a_quick_panel_tap_is_a_click() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();

    handle_reprog(&mut acc, panel_press(), PANEL, &[], &[], &tx);
    handle_reprog(&mut acc, release(), PANEL, &[], &[], &tx);

    assert_eq!(
        next_gesture(&mut rx),
        Ok(CapturedInput::Gesture(
            ButtonId::HapticPanel,
            GestureDirection::Click
        ))
    );
    assert!(
        next_gesture(&mut rx).is_err(),
        "a panel tap emits exactly one click"
    );
}

#[test]
fn the_panels_first_raw_xy_sample_after_contact_is_discarded() {
    // Real-hardware probe finding: the panel's first raw-XY sample after
    // contact is a large position jump (up to thousands of units), not a
    // relative delta. Un-discarded it would instantly commit a bogus swipe.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();

    handle_reprog(&mut acc, panel_press(), PANEL, &[], &[], &tx);
    // The contact jump — leftward, far past every threshold.
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: -3000, dy: 40 },
        PANEL,
        &[],
        &[],
        &tx,
    );
    assert!(
        next_gesture(&mut rx).is_err(),
        "the contact jump must not commit a swipe"
    );
    // The real swipe starts from a clean accumulator: had the jump been
    // summed, this rightward travel could never commit Right.
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: 120, dy: 5 },
        PANEL,
        &[],
        &[],
        &tx,
    );
    assert_eq!(
        next_gesture(&mut rx),
        Ok(CapturedInput::Gesture(
            ButtonId::HapticPanel,
            GestureDirection::Right
        ))
    );
}

#[test]
fn the_dedicated_buttons_first_sample_is_not_discarded() {
    // Devices without a haptic panel keep their first dedicated-button delta.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc =
        CaptureAccum::with_policy(FirstRawXyPolicy::from_control_ids(GESTURE.iter().copied()));

    handle_reprog(&mut acc, press(), GESTURE, &[], &[], &tx);
    acc.backdate_hold_for_test();
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: 120, dy: 5 },
        GESTURE,
        &[],
        &[],
        &tx,
    );

    assert_eq!(
        next_gesture(&mut rx),
        Ok(CapturedInput::Gesture(
            ButtonId::GestureButton,
            GestureDirection::Right
        )),
        "the dedicated button's very first sample still counts"
    );
}

#[test]
fn an_undiverted_gesture_source_does_not_gesture() {
    // Only the panel is diverted for gestures; a dedicated-button press must
    // not begin a hold, emit a click, or feed the swipe accumulator — the two
    // sources are distinct physical controls.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();

    handle_reprog(&mut acc, press(), PANEL, &[], &[], &tx);
    acc.backdate_hold_for_test();
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: 120, dy: 5 },
        PANEL,
        &[],
        &[],
        &tx,
    );
    handle_reprog(&mut acc, release(), PANEL, &[], &[], &tx);

    assert!(
        next_gesture(&mut rx).is_err(),
        "a non-owner source must neither gesture nor click"
    );
}

#[test]
fn gesture_sources_emit_independent_edges_without_snapshot_duplicates() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();

    handle_reprog(&mut acc, press(), BOTH, &[], &[], &tx);
    handle_reprog(&mut acc, press(), BOTH, &[], &[], &tx);
    handle_reprog(&mut acc, both_press(), BOTH, &[], &[], &tx);
    handle_reprog(&mut acc, panel_press(), BOTH, &[], &[], &tx);
    handle_reprog(&mut acc, release(), BOTH, &[], &[], &tx);

    let edges: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
        .filter(|input| !matches!(input, CapturedInput::Gesture(..)))
        .collect();
    assert_eq!(
        edges,
        vec![
            CapturedInput::ButtonDown(ButtonId::GestureButton),
            CapturedInput::ButtonDown(ButtonId::HapticPanel),
            CapturedInput::ButtonUp(ButtonId::GestureButton),
            CapturedInput::ButtonUp(ButtonId::HapticPanel),
        ]
    );
}

#[test]
fn a_plain_diverted_gesture_button_presses_without_gesturing() {
    // A gesture button diverted as a plain button (not in gesture mode; its
    // single binding needs delivery) must dispatch as a button press only —
    // the swipe accumulator belongs to the raw-XY gesture diverts and must
    // not also emit a gesture click on release.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();
    let buttons = [(reprog_controls::GESTURE_BUTTON_CID, ButtonId::GestureButton)];

    handle_reprog(&mut acc, press(), &[], &[], &buttons, &tx);
    handle_reprog(&mut acc, release(), &[], &[], &buttons, &tx);

    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonDown(ButtonId::GestureButton))
    );
    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonUp(ButtonId::GestureButton))
    );
    assert!(
        rx.try_recv().is_err(),
        "a plain-diverted gesture button must not also emit a gesture click"
    );
}

#[test]
fn a_plain_diverted_haptic_panel_presses_as_its_own_button() {
    // A single action bound to the panel (which is not in gesture mode) is
    // delivered as ButtonId::HapticPanel — its own control, never conflated
    // with the dedicated gesture button.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();
    let buttons = [(reprog_controls::HAPTIC_PANEL_CID, ButtonId::HapticPanel)];

    handle_reprog(&mut acc, panel_press(), &[], &[], &buttons, &tx);
    handle_reprog(&mut acc, release(), &[], &[], &buttons, &tx);

    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonDown(ButtonId::HapticPanel))
    );
    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonUp(ButtonId::HapticPanel))
    );
    assert!(
        rx.try_recv().is_err(),
        "a plain-diverted panel must not also emit a gesture click"
    );
}

#[test]
fn plain_button_snapshots_emit_each_edge_once_and_keep_buttons_independent() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();
    let buttons = [(0x0053, ButtonId::Back), (0x0056, ButtonId::Forward)];
    let back = RawControlEvent::DivertedButtons([0x0053, 0, 0, 0]);
    let both = RawControlEvent::DivertedButtons([0x0053, 0x0056, 0, 0]);
    let forward = RawControlEvent::DivertedButtons([0x0056, 0, 0, 0]);

    handle_reprog(&mut acc, back, &[], &[], &buttons, &tx);
    handle_reprog(&mut acc, back, &[], &[], &buttons, &tx);
    handle_reprog(&mut acc, both, &[], &[], &buttons, &tx);
    handle_reprog(&mut acc, forward, &[], &[], &buttons, &tx);
    handle_reprog(&mut acc, release(), &[], &[], &buttons, &tx);

    assert_eq!(rx.try_recv(), Ok(CapturedInput::ButtonDown(ButtonId::Back)));
    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonDown(ButtonId::Forward))
    );
    assert_eq!(rx.try_recv(), Ok(CapturedInput::ButtonUp(ButtonId::Back)));
    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonUp(ButtonId::Forward))
    );
    assert!(rx.try_recv().is_err(), "unchanged snapshots emit no edges");
}

#[test]
fn a_side_gesture_button_uses_its_hidpp_raw_xy() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();
    let cid = 0x0056;
    let buttons = [(cid, ButtonId::Forward)];
    let down = RawControlEvent::DivertedButtons([cid, 0, 0, 0]);

    acc.on_event(down, &[], &[], &buttons, &[], &tx);
    acc.backdate_hold_for_test();
    acc.on_event(
        RawControlEvent::RawXy { dx: -120, dy: 5 },
        &[],
        &[],
        &buttons,
        &[],
        &tx,
    );
    acc.on_event(release(), &[], &[], &buttons, &[], &tx);

    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonDown(ButtonId::Forward))
    );
    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::Gesture(
            ButtonId::Forward,
            GestureDirection::Left
        ))
    );
    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonUp(ButtonId::Forward))
    );
    assert!(
        rx.try_recv().is_err(),
        "a committed side-button swipe must not also click on release"
    );
}

#[test]
fn a_side_gesture_button_tap_is_a_click() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();
    let cid = 0x0056;
    let buttons = [(cid, ButtonId::Forward)];
    let down = RawControlEvent::DivertedButtons([cid, 0, 0, 0]);

    acc.on_event(down, &[], &[], &buttons, &[], &tx);
    acc.on_event(release(), &[], &[], &buttons, &[], &tx);

    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonDown(ButtonId::Forward))
    );
    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::Gesture(
            ButtonId::Forward,
            GestureDirection::Click
        ))
    );
    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonUp(ButtonId::Forward))
    );
    assert_eq!(rx.try_recv(), Err(mpsc::error::TryRecvError::Empty));
}

#[test]
fn a_dpi_gesture_button_uses_the_shared_raw_xy_path() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();
    let cid = reprog_controls::DPI_MODE_SHIFT_CIDS[0];
    let buttons = [(cid, ButtonId::DpiToggle)];
    let down = RawControlEvent::DivertedButtons([cid, 0, 0, 0]);

    acc.on_event(down, &[], &[], &buttons, &[], &tx);
    acc.backdate_hold_for_test();
    acc.on_event(
        RawControlEvent::RawXy { dx: 5, dy: -120 },
        &[],
        &[],
        &buttons,
        &[],
        &tx,
    );
    acc.on_event(release(), &[], &[], &buttons, &[], &tx);

    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonDown(ButtonId::DpiToggle))
    );
    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::Gesture(
            ButtonId::DpiToggle,
            GestureDirection::Up
        ))
    );
    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonUp(ButtonId::DpiToggle))
    );
    assert_eq!(rx.try_recv(), Err(mpsc::error::TryRecvError::Empty));
}

#[test]
fn a_held_dpi_button_presses_once_on_the_rising_edge() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();
    let dpi = reprog_controls::DPI_MODE_SHIFT_CIDS[0];
    let down = RawControlEvent::DivertedButtons([dpi, 0, 0, 0]);

    handle_reprog(&mut acc, down, GESTURE, &[dpi], &[], &tx);
    handle_reprog(&mut acc, down, GESTURE, &[dpi], &[], &tx);

    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonDown(ButtonId::DpiToggle))
    );
    assert!(rx.try_recv().is_err(), "a held DPI button presses once");
}

#[test]
fn a_dpi_button_re_presses_after_a_release() {
    // Rising-edge detection must re-arm: press → release → press is two
    // distinct presses. The release (a frame without the CID) is what resets
    // the edge; without it a re-press would be swallowed as "still held".
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();
    let dpi = reprog_controls::DPI_MODE_SHIFT_CIDS[0];
    let down = RawControlEvent::DivertedButtons([dpi, 0, 0, 0]);
    let up = RawControlEvent::DivertedButtons([0, 0, 0, 0]);

    handle_reprog(&mut acc, down, GESTURE, &[dpi], &[], &tx);
    handle_reprog(&mut acc, up, GESTURE, &[dpi], &[], &tx);
    handle_reprog(&mut acc, down, GESTURE, &[dpi], &[], &tx);

    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonDown(ButtonId::DpiToggle))
    );
    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonUp(ButtonId::DpiToggle)),
        "the falling edge must be preserved"
    );
    assert_eq!(
        rx.try_recv(),
        Ok(CapturedInput::ButtonDown(ButtonId::DpiToggle)),
        "a release re-arms the rising edge"
    );
    assert!(
        rx.try_recv().is_err(),
        "press → release → press emits exactly three lifecycle edges"
    );
}

#[test]
fn dedicated_swipe_commits_immediately_and_ignores_opposite_recovery() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();
    handle_reprog(&mut acc, press(), GESTURE, &[], &[], &tx);
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: -120, dy: 5 },
        GESTURE,
        &[],
        &[],
        &tx,
    );
    assert_eq!(
        next_gesture(&mut rx),
        Ok(CapturedInput::Gesture(
            ButtonId::GestureButton,
            GestureDirection::Left,
        )),
        "a deliberate dedicated-button swipe must not wait 160 ms"
    );
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: 300, dy: 0 },
        GESTURE,
        &[],
        &[],
        &tx,
    );
    handle_reprog(&mut acc, release(), GESTURE, &[], &[], &tx);
    assert!(
        next_gesture(&mut rx).is_err(),
        "recovery and release must not emit another action"
    );
}

#[test]
fn side_button_raw_xy_still_waits_for_the_hold_gate() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();
    let cid = 0x0056;
    let buttons = [(cid, ButtonId::Forward)];
    acc.on_event(
        RawControlEvent::DivertedButtons([cid, 0, 0, 0]),
        &[],
        &[],
        &buttons,
        &[],
        &tx,
    );
    acc.on_event(
        RawControlEvent::RawXy { dx: -120, dy: 5 },
        &[],
        &[],
        &buttons,
        &[],
        &tx,
    );
    assert!(
        next_gesture(&mut rx).is_err(),
        "ordinary buttons retain click-drift protection"
    );
    acc.backdate_hold_for_test();
    acc.on_event(
        RawControlEvent::RawXy { dx: -1, dy: 0 },
        &[],
        &[],
        &buttons,
        &[],
        &tx,
    );
    assert_eq!(
        next_gesture(&mut rx),
        Ok(CapturedInput::Gesture(
            ButtonId::Forward,
            GestureDirection::Left,
        ))
    );
}

#[test]
fn haptic_device_dedicated_button_ignores_stale_first_motion() {
    // Two leftward holds captured on an MX Master 4 (PR #1301): the first
    // packet carries rightward travel from before the press, then real left
    // motion. Without the discard these committed Right.
    let policy = FirstRawXyPolicy::from_control_ids(BOTH.iter().copied());
    for samples in [
        vec![(2041, -74), (-21, -5), (-21, -4), (-25, -4)],
        vec![(252, 34), (-37, -4), (-23, -2)],
    ] {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut acc = CaptureAccum::with_policy(policy);
        handle_reprog(&mut acc, press(), GESTURE, &[], &[], &tx);
        for (dx, dy) in samples {
            handle_reprog(
                &mut acc,
                RawControlEvent::RawXy { dx, dy },
                GESTURE,
                &[],
                &[],
                &tx,
            );
        }
        assert_eq!(
            next_gesture(&mut rx),
            Ok(CapturedInput::Gesture(
                ButtonId::GestureButton,
                GestureDirection::Left,
            ))
        );
        assert!(next_gesture(&mut rx).is_err(), "one action per hold");
    }
}

#[test]
fn haptic_device_dedicated_takeover_only_discards_on_a_fresh_press() {
    for overlaps in [false, true] {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut acc =
            CaptureAccum::with_policy(FirstRawXyPolicy::from_control_ids(BOTH.iter().copied()));
        handle_reprog(&mut acc, panel_press(), BOTH, &[], &[], &tx);
        if overlaps {
            handle_reprog(&mut acc, both_press(), BOTH, &[], &[], &tx);
            handle_reprog(
                &mut acc,
                RawControlEvent::RawXy { dx: 2041, dy: -74 },
                BOTH,
                &[],
                &[],
                &tx,
            );
        }
        handle_reprog(&mut acc, press(), BOTH, &[], &[], &tx);
        assert_eq!(
            next_gesture(&mut rx),
            Ok(CapturedInput::Gesture(
                ButtonId::HapticPanel,
                GestureDirection::Click
            ))
        );
        if !overlaps {
            handle_reprog(
                &mut acc,
                RawControlEvent::RawXy { dx: 2041, dy: -74 },
                BOTH,
                &[],
                &[],
                &tx,
            );
            assert!(
                next_gesture(&mut rx).is_err(),
                "fresh press drops stale sample"
            );
        }
        handle_reprog(
            &mut acc,
            RawControlEvent::RawXy { dx: -120, dy: -5 },
            BOTH,
            &[],
            &[],
            &tx,
        );
        assert_eq!(
            next_gesture(&mut rx),
            Ok(CapturedInput::Gesture(
                ButtonId::GestureButton,
                GestureDirection::Left
            ))
        );
    }
}

#[test]
fn a_reconnect_reset_keeps_the_haptic_device_policy() {
    // Regression guard for the port onto the split module: resetting capture
    // state after a wireless reconnect must not fall back to PanelOnly.
    let mut armed = ArmedControls::default();
    armed.first_raw_xy_policy = FirstRawXyPolicy::from_control_ids(BOTH.iter().copied());
    let capture = GestureCapture::new(armed);
    // A reconnect broadcast resets input state through the ArmedCapture hook.
    capture.reset_input_state();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = capture.accum.lock().unwrap_or_else(PoisonError::into_inner);
    handle_reprog(&mut acc, press(), GESTURE, &[], &[], &tx);
    handle_reprog(
        &mut acc,
        RawControlEvent::RawXy { dx: 2041, dy: -74 },
        GESTURE,
        &[],
        &[],
        &tx,
    );
    assert!(next_gesture(&mut rx).is_err(), "stale first sample dropped");
}

#[test]
fn a_quick_dpi_gesture_press_with_drift_is_still_a_click() {
    // DPI/ModeShift is a HID++ gesture source but an ordinary button in daily
    // use: it keeps the click-drift hold gate the dedicated controls dropped.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut acc = CaptureAccum::default();
    let cid = reprog_controls::DPI_MODE_SHIFT_CIDS[0];
    let buttons = [(cid, ButtonId::DpiToggle)];
    acc.on_event(
        RawControlEvent::DivertedButtons([cid, 0, 0, 0]),
        &[],
        &[],
        &buttons,
        &[],
        &tx,
    );
    acc.on_event(
        RawControlEvent::RawXy { dx: -120, dy: 5 },
        &[],
        &[],
        &buttons,
        &[],
        &tx,
    );
    acc.on_event(release(), &[], &[], &buttons, &[], &tx);
    assert_eq!(
        next_gesture(&mut rx),
        Ok(CapturedInput::Gesture(
            ButtonId::DpiToggle,
            GestureDirection::Click
        ))
    );
    assert!(next_gesture(&mut rx).is_err(), "exactly one click");
}
