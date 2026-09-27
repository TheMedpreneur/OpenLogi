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
use openlogi_core::binding::{Action, ActionRingIcon, ActionRingSlot};
use openlogi_ipc::ActionRingInvocation;
use openlogi_ui::action_icons::RING_CANCEL_ICON;
use openlogi_ui::color;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::ipc::OverlayCommand;
use crate::session::{ClickAwaySession, ShowingRing};

pub(crate) const WINDOW_SIZE: f32 = 480.0;
/// Distance from the ring's centre to the centre of each action tile.
pub(crate) const RADIUS: f32 = 165.0;
/// Each action is one glass tile: a colour-coded icon badge over a label of up
/// to two lines, so names read at a normal size instead of being squeezed onto
/// one tiny line. On the 165 px radius the closest tiles keep a 16.7 px gap,
/// the farthest corner (a diagonal tile's) sits 226.7 px out, inside the
/// 232 px panel, and the diagonal tiles keep 103.9 px from the centre.
const CHIP_WIDTH: f32 = 100.0;
const CHIP_HEIGHT: f32 = 74.0;
const BADGE_SIZE: f32 = 32.0;
const LABEL_SIZE: f32 = 13.0;
/// Inset of the round panel from the window edge (panel radius 232 px).
pub(crate) const PANEL_INSET: f32 = 8.0;
const CANCEL_SIZE: f32 = 48.0;

/// A hue per kind of action, so the ring can be read at a glance by colour
/// before the labels are read: editing, files and windows, navigation, media,
/// marking, capture and system, pointer. Only `Ban` stays neutral.
fn badge_hue(icon: ActionRingIcon) -> Option<f32> {
    use ActionRingIcon as I;
    Some(match icon {
        I::Copy
        | I::Paste
        | I::Cut
        | I::Undo
        | I::Redo
        | I::SelectAll
        | I::Save
        | I::Search
        | I::Keyboard => 0.58,
        I::Grid
        | I::Layers
        | I::Monitor
        | I::Applications
        | I::PreviousDesktop
        | I::NextDesktop
        | I::Folder
        | I::File
        | I::Book => 0.47,
        I::NewTab
        | I::CloseTab
        | I::ReopenTab
        | I::NextTab
        | I::PreviousTab
        | I::Reload
        | I::MouseBack
        | I::MouseForward
        | I::ArrowUp
        | I::ArrowDown
        | I::ArrowLeft
        | I::ArrowRight
        | I::ScrollLeft
        | I::ScrollRight
        | I::Globe => 0.72,
        I::Play | I::Volume | I::VolumeDown | I::Mute | I::PreviousTrack | I::NextTrack => 0.07,
        I::Star | I::Heart | I::Bell | I::Calendar | I::User => 0.13,
        I::Camera | I::Lock | I::Refresh | I::Terminal | I::Settings | I::Palette => 0.93,
        I::Pointer | I::Mouse | I::Gauge => 0.36,
        I::Ban => return None,
    })
}

/// The badge fill: a lit-top gradient in the action's hue, or neutral glass.
fn badge_fill(icon: ActionRingIcon) -> gpui::Background {
    let (top, bottom) = badge_hue(icon).map_or((neutral(0.42, 0.95), neutral(0.28, 0.95)), |h| {
        (
            Hsla {
                h,
                s: 0.62,
                l: 0.56,
                a: 1.0,
            },
            Hsla {
                h,
                s: 0.66,
                l: 0.40,
                a: 1.0,
            },
        )
    });
    linear_gradient(
        180.0,
        linear_color_stop(top, 0.0),
        linear_color_stop(bottom, 1.0),
    )
}

/// The ring's own palette. It floats over whatever is on the desktop, so unlike
/// the settings app it cannot take its surfaces from the OS appearance. Only
/// the accent is shared (`openlogi_ui::color`); these greys are local by nature.
///
/// Smoked glass: a near-opaque panel darkening top to bottom under a soft
/// sheen, with translucent tiles on it. Measured on screen over a white page,
/// anything much lighter let page text show through the panel.
struct Palette {
    /// Panel gradient, top to bottom.
    panel: (Hsla, Hsla),
    /// Resting tile gradient, top to bottom: a lit upper edge fading down.
    chip: (Hsla, Hsla),
    cancel: Hsla,
}

const PALETTE: Palette = Palette {
    panel: (neutral(0.15, 0.96), neutral(0.05, 0.98)),
    chip: (neutral(1.0, 0.13), neutral(1.0, 0.04)),
    cancel: neutral(1.0, 0.18),
};

/// A hairline of light on every glass edge, the cue that separates layers when
/// there is no opaque fill to do it.
const GLASS_EDGE: Hsla = neutral(1.0, 0.16);
const GLYPH: Hsla = neutral(0.98, 1.0);
const LABEL: Hsla = neutral(0.97, 1.0);
const LABEL_RESTING: Hsla = neutral(0.90, 1.0);
const CANCEL_GLYPH: Hsla = neutral(0.96, 1.0);
/// The light catching the top of the glass disc.
const SHEEN: Hsla = neutral(1.0, 0.09);

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

    /// One action tile: colour badge over a two-line label, which is the
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
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(4.0))
                .px(px(6.0))
                .rounded(px(18.0))
                .border_1()
                .map(|chip| {
                    if selected {
                        chip.bg(color::accent_at_lightness(SELECTED_FILL_L))
                            .border_color(color::accent_at_lightness(SELECTED_BORDER_L))
                    } else {
                        chip.bg(linear_gradient(
                            180.0,
                            linear_color_stop(PALETTE.chip.0, 0.0),
                            linear_color_stop(PALETTE.chip.1, 1.0),
                        ))
                        .border_color(GLASS_EDGE)
                    }
                })
                .cursor_pointer()
                .child(
                    div()
                        .size(px(BADGE_SIZE))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(badge_fill(presentation.icon))
                        .border_1()
                        .border_color(if selected { GLYPH } else { GLASS_EDGE })
                        .child(svg().path(icon_path).size(px(17.0)).text_color(GLYPH)),
                )
                .child(
                    div()
                        .w_full()
                        .text_center()
                        .text_size(px(LABEL_SIZE))
                        .line_height(px(LABEL_SIZE + 2.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .line_clamp(2)
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
                        linear_color_stop(PALETTE.panel.0, 0.0),
                        linear_color_stop(PALETTE.panel.1, 1.0),
                    ))
                    .border_1()
                    .border_color(GLASS_EDGE),
            )
            .child(
                div()
                    .absolute()
                    .left(px(PANEL_INSET + 1.0))
                    .top(px(PANEL_INSET + 1.0))
                    .size(px(WINDOW_SIZE - 2.0 * PANEL_INSET - 2.0))
                    .rounded_full()
                    .bg(linear_gradient(
                        180.0,
                        linear_color_stop(SHEEN, 0.0),
                        linear_color_stop(neutral(1.0, 0.0), 0.5),
                    )),
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
                    .bg(PALETTE.cancel)
                    .border_1()
                    .border_color(GLASS_EDGE)
                    .text_color(CANCEL_GLYPH)
                    .cursor_pointer()
                    .child(
                        svg()
                            .path(RING_CANCEL_ICON)
                            .size(px(20.0))
                            .flex_none()
                            .text_color(CANCEL_GLYPH),
                    )
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        let _ = center_commands.send(OverlayCommand::Cancel { session_id });
                        window.remove_window();
                    }),
            )
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
        window_background: WindowBackgroundAppearance::Transparent,
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
            for &other in &slots[i + 1..] {
                let (l2, t2, r2, b2) = rect(other);
                let gap = (l2 - r).max(l - r2).max(t2 - b).max(t - b2);
                assert!(gap >= 4.0, "{a:?} and {other:?} are only {gap} px apart");
            }
        }
    }
}
