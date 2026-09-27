//! The ring itself: the GPUI view, and the window it is drawn in.
//!
//! Placement is the interesting part — the panel is centred on the cursor and
//! clamped to the display it came up on, so a ring raised near a screen edge
//! stays whole instead of being cut off.

#[cfg(any(not(target_os = "windows"), test))]
use gpui::{Bounds, Pixels, Point, Size, point};
use gpui::{
    Context, Hsla, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Window, WindowBackgroundAppearance, WindowKind,
    WindowOptions, div, linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px, svg,
};
use openlogi_core::binding::{Action, ActionRingSlot};
use openlogi_ipc::ActionRingInvocation;
use openlogi_ui::action_icons::RING_CANCEL_ICON;
use openlogi_ui::color;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::ipc::OverlayCommand;
use crate::session::{ClickAwaySession, ShowingRing};

pub(crate) const WINDOW_SIZE: f32 = 360.0;
/// Distance from the ring's centre to the centre of each action chip.
pub(crate) const RADIUS: f32 = 112.0;
/// Each action is one horizontal chip carrying its icon *and* its name, so a
/// label can never drift away from, or crowd into, a neighbour. At this size
/// on the 112 px radius the closest pair (diagonal neighbours) keeps a 4.8 px
/// gap, the farthest corner sits 168.6 px out (inside the 174 px panel), and
/// the cancel button keeps a 54 px clearing.
const CHIP_WIDTH: f32 = 112.0;
const CHIP_HEIGHT: f32 = 28.0;
/// Inset of the round panel from the window edge (panel radius 174 px).
pub(crate) const PANEL_INSET: f32 = 6.0;
const CANCEL_SIZE: f32 = 44.0;
/// The hovered action's full name, under the cancel button.
const HOVER_LABEL_WIDTH: f32 = 180.0;

/// The ring's own palette. It floats over whatever is on the desktop, so unlike
/// the settings app it cannot take its surfaces from the OS appearance. Only
/// the accent is shared (`openlogi_ui::color`); these greys are local by nature.
struct Palette {
    /// Panel gradient, top to bottom.
    panel: (Hsla, Hsla),
    /// Resting chip gradient, top to bottom: a lit upper edge fading down is
    /// what reads as glass rather than a flat translucent fill.
    chip: (Hsla, Hsla),
    cancel: Hsla,
}

/// Frosted glass over macOS's behind-window blur. gpui strips the blur view's
/// own material tint (its `updateLayer` clears every sublayer fill), so all of
/// the darkness has to come from these fills: the panel stays dark enough that
/// white 11 px labels keep roughly 5:1 contrast even over a white page.
const GLASS: Palette = Palette {
    panel: (neutral(0.18, 0.72), neutral(0.04, 0.86)),
    chip: (neutral(1.0, 0.17), neutral(1.0, 0.05)),
    cancel: neutral(1.0, 0.12),
};

/// No blur behind the window (other platforms, or macOS "Reduce
/// transparency"): the same shapes on near-opaque fills.
const SOLID: Palette = Palette {
    panel: (neutral(0.10, 0.98), neutral(0.06, 0.98)),
    chip: (neutral(0.24, 1.0), neutral(0.18, 1.0)),
    cancel: neutral(0.24, 1.0),
};

/// A hairline of light on every glass edge, the cue that separates layers when
/// there is no opaque fill to do it.
const GLASS_EDGE: Hsla = neutral(1.0, 0.16);
const GLYPH: Hsla = neutral(0.98, 1.0);
const LABEL: Hsla = neutral(0.97, 1.0);
const LABEL_RESTING: Hsla = neutral(0.90, 1.0);
const CANCEL_GLYPH: Hsla = neutral(0.86, 1.0);

const fn neutral(lightness: f32, alpha: f32) -> Hsla {
    Hsla {
        h: 0.0,
        s: 0.0,
        l: lightness,
        a: alpha,
    }
}

/// The accent deepened for the dark panel: the brand lightness sits too close to
/// the white glyph a selected chip carries, so the fill drops to `0.48` and the
/// edge around it rises to `0.78`. Both keep the brand hue and saturation.
const SELECTED_FILL_L: f32 = 0.48;
const SELECTED_BORDER_L: f32 = 0.78;

/// The text shown for a slot. User-authored labels render verbatim: passing
/// them through the localization table would translate any label that happens
/// to collide with a known key ("Copy" → "Copier" under fr).
fn display_label(presentation: &openlogi_ipc::ActionRingPresentation) -> SharedString {
    let label = if presentation.literal {
        presentation.label.clone()
    } else if let Some(key) = Action::translation_key_for_label(&presentation.label) {
        rust_i18n::t!(key).into_owned()
    } else {
        presentation.label.clone()
    };
    SharedString::from(label)
}

/// Top-left corner of `slot`'s chip inside the window.
fn chip_origin(slot: ActionRingSlot) -> (f32, f32) {
    // A zero-size placement is the chip's centre.
    let (cx, cy) = slot.placement(WINDOW_SIZE, RADIUS, 0.0);
    (cx - CHIP_WIDTH / 2.0, cy - CHIP_HEIGHT / 2.0)
}

pub(crate) struct RingView {
    invocation: ActionRingInvocation,
    /// Chosen once per ring: glass when the OS will blur behind the window.
    palette: &'static Palette,
    commands: mpsc::UnboundedSender<OverlayCommand>,
    hovered: Option<ActionRingSlot>,
    /// Publishes click-away identity for exactly this view's lifetime.
    _showing: ShowingRing,
}

impl RingView {
    /// Open a view on `invocation`, reporting interactions through `commands`.
    pub(crate) fn new(
        invocation: ActionRingInvocation,
        commands: mpsc::UnboundedSender<OverlayCommand>,
        live: &Arc<ClickAwaySession>,
    ) -> Self {
        let showing = live.showing(invocation.session_id);
        Self {
            invocation,
            palette: if crate::platform::glass_enabled() {
                &GLASS
            } else {
                &SOLID
            },
            commands,
            hovered: None,
            _showing: showing,
        }
    }

    /// The ring session this view is showing.
    pub(crate) const fn session_id(&self) -> u64 {
        self.invocation.session_id
    }

    /// Report this ring cancelled. The window is closed by the caller, which
    /// is the only one holding the handle.
    pub(crate) fn cancel(&self) {
        let _ = self.commands.send(OverlayCommand::Cancel {
            session_id: self.invocation.session_id,
        });
    }

    /// One action chip: icon and name on a single glass pill, which is the
    /// whole hit target for hover (haptic buzz) and activation.
    fn chip_element(
        &self,
        slot: ActionRingSlot,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let presentation = self.invocation.slots.get(&slot)?;
        let icon_path = presentation.icon.asset_path();
        let selected = self.hovered == Some(slot);
        let (left, top) = chip_origin(slot);
        let session_id = self.invocation.session_id;
        let activate = self.commands.clone();
        Some(
            div()
                .id(("ring-slot", slot.index()))
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(CHIP_WIDTH))
                .h(px(CHIP_HEIGHT))
                .flex()
                .items_center()
                .gap(px(5.0))
                .px(px(9.0))
                .rounded_full()
                .border_1()
                .map(|chip| {
                    if selected {
                        chip.bg(color::accent_at_lightness(SELECTED_FILL_L))
                            .border_color(color::accent_at_lightness(SELECTED_BORDER_L))
                    } else {
                        chip.bg(linear_gradient(
                            180.0,
                            linear_color_stop(self.palette.chip.0, 0.0),
                            linear_color_stop(self.palette.chip.1, 1.0),
                        ))
                        .border_color(GLASS_EDGE)
                    }
                })
                .cursor_pointer()
                .child(
                    svg()
                        .path(icon_path)
                        .size(px(14.0))
                        .flex_none()
                        .text_color(GLYPH),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(11.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(if selected { LABEL } else { LABEL_RESTING })
                        .child(display_label(presentation)),
                )
                .on_hover(cx.listener(move |this, hovered, _, cx| {
                    if *hovered && this.hovered != Some(slot) {
                        this.hovered = Some(slot);
                        let _ = this
                            .commands
                            .send(OverlayCommand::Hover { session_id, slot });
                        cx.notify();
                    } else if !*hovered && this.hovered == Some(slot) {
                        this.hovered = None;
                        cx.notify();
                    }
                }))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    let _ = activate.send(OverlayCommand::Activate { session_id, slot });
                    window.remove_window();
                })
                .into_any_element(),
        )
    }
}

impl Render for RingView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let session_id = self.invocation.session_id;
        let root_commands = self.commands.clone();
        let center_commands = self.commands.clone();
        let hovered_label = self
            .hovered
            .and_then(|slot| self.invocation.slots.get(&slot).map(display_label));
        let chips = ActionRingSlot::ALL
            .into_iter()
            .filter_map(|slot| self.chip_element(slot, cx))
            .collect::<Vec<_>>();

        div()
            .id("ring-root")
            .relative()
            .size_full()
            .child(
                div()
                    .absolute()
                    .left(px(PANEL_INSET))
                    .top(px(PANEL_INSET))
                    .size(px(WINDOW_SIZE - 2.0 * PANEL_INSET))
                    .rounded_full()
                    .bg(linear_gradient(
                        180.0,
                        linear_color_stop(self.palette.panel.0, 0.0),
                        linear_color_stop(self.palette.panel.1, 1.0),
                    ))
                    .border_1()
                    .border_color(GLASS_EDGE),
            )
            .children(chips)
            .child(
                div()
                    .id("ring-cancel")
                    .absolute()
                    .left(px(WINDOW_SIZE / 2.0 - CANCEL_SIZE / 2.0))
                    .top(px(WINDOW_SIZE / 2.0 - CANCEL_SIZE / 2.0))
                    .size(px(CANCEL_SIZE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(self.palette.cancel)
                    .border_1()
                    .border_color(GLASS_EDGE)
                    .text_color(CANCEL_GLYPH)
                    .cursor_pointer()
                    .child(svg().path(RING_CANCEL_ICON).size(px(18.0)).flex_none())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        let _ = center_commands.send(OverlayCommand::Cancel { session_id });
                        window.remove_window();
                    }),
            )
            .when_some(hovered_label, |ring, label| {
                ring.child(
                    div()
                        .absolute()
                        // Between the cancel button and the Bottom chip, clear
                        // of the Left/Right and lower diagonal chips.
                        .left(px(WINDOW_SIZE / 2.0 - HOVER_LABEL_WIDTH / 2.0))
                        .top(px(WINDOW_SIZE / 2.0 + CANCEL_SIZE / 2.0 + 6.0))
                        .w(px(HOVER_LABEL_WIDTH))
                        .text_center()
                        .text_xs()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(LABEL)
                        .child(label),
                )
            })
            .on_click(move |_, window, _| {
                let _ = root_commands.send(OverlayCommand::Cancel { session_id });
                window.remove_window();
            })
    }
}

/// Appearance shared by the platform-specific placement paths.
pub(crate) fn ring_window_options() -> WindowOptions {
    WindowOptions {
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        // macOS frosts what is behind the ring; the blur view gpui installs is
        // masked to the panel circle in `platform::configure_windows`.
        window_background: if cfg!(target_os = "macos") {
            WindowBackgroundAppearance::Blurred
        } else {
            WindowBackgroundAppearance::Transparent
        },
        app_id: Some("openlogi-action-ring".to_string()),
        ..WindowOptions::default()
    }
}

#[cfg(any(not(target_os = "windows"), test))]
pub(crate) fn clamp_window_origin(
    desired: Point<Pixels>,
    window_size: Size<Pixels>,
    display: Bounds<Pixels>,
) -> Point<Pixels> {
    let max = point(
        display.right() - window_size.width,
        display.bottom() - window_size.height,
    );
    desired.clamp(&display.origin, &max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_origin_is_clamped_to_the_display() {
        let display = Bounds::new(point(px(100.0), px(50.0)), Size::new(px(800.0), px(600.0)));
        let size = Size::new(px(400.0), px(400.0));
        assert_eq!(
            clamp_window_origin(point(px(-50.0), px(-50.0)), size, display),
            point(px(100.0), px(50.0))
        );
        assert_eq!(
            clamp_window_origin(point(px(700.0), px(500.0)), size, display),
            point(px(500.0), px(250.0))
        );
    }

    #[test]
    fn overlay_origin_stays_cursor_centered_away_from_edges() {
        let display = Bounds::new(Point::default(), Size::new(px(1600.0), px(1000.0)));
        let desired = point(px(600.0), px(300.0));
        assert_eq!(
            clamp_window_origin(desired, Size::new(px(400.0), px(400.0)), display),
            desired
        );
    }
}

#[cfg(test)]
mod chip_layout_tests {
    use super::*;

    fn rect(slot: ActionRingSlot) -> (f32, f32, f32, f32) {
        let (left, top) = chip_origin(slot);
        (left, top, left + CHIP_WIDTH, top + CHIP_HEIGHT)
    }

    #[test]
    fn chips_never_touch_each_other_the_cancel_button_or_the_panel_rim() {
        let centre = WINDOW_SIZE / 2.0;
        let panel_radius = centre - PANEL_INSET;
        let slots = ActionRingSlot::ALL;
        for (i, &a) in slots.iter().enumerate() {
            let (l, t, r, b) = rect(a);
            for &(x, y) in &[(l, t), (r, t), (l, b), (r, b)] {
                let reach = ((x - centre).powi(2) + (y - centre).powi(2)).sqrt();
                assert!(reach <= panel_radius - 2.0, "{a:?} corner leaves the panel");
            }
            let nearest_x = centre.clamp(l, r);
            let nearest_y = centre.clamp(t, b);
            let clearance = ((nearest_x - centre).powi(2) + (nearest_y - centre).powi(2)).sqrt();
            assert!(
                clearance >= CANCEL_SIZE / 2.0 + 8.0,
                "{a:?} crowds the cancel button"
            );
            let label_top = centre + CANCEL_SIZE / 2.0 + 6.0;
            let (label_l, label_r, label_b) = (
                centre - HOVER_LABEL_WIDTH / 2.0,
                centre + HOVER_LABEL_WIDTH / 2.0,
                label_top + 20.0,
            );
            let label_gap = (l - label_r)
                .max(label_l - r)
                .max(t - label_b)
                .max(label_top - b);
            assert!(label_gap >= 2.0, "{a:?} overlaps the hover label");
            for &other in &slots[i + 1..] {
                let (l2, t2, r2, b2) = rect(other);
                let gap = (l2 - r).max(l - r2).max(t2 - b).max(t - b2);
                assert!(gap >= 4.0, "{a:?} and {other:?} are only {gap} px apart");
            }
        }
    }
}
