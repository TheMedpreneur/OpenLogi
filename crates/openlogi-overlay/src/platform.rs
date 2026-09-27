//! Native window policy for the standalone Actions Ring overlay.

#[cfg(not(target_os = "windows"))]
mod placement;
// Keep Windows geometry tests runnable on the host; only native.rs needs Win32.
#[cfg(any(target_os = "windows", test))]
mod windows;

#[cfg(not(target_os = "windows"))]
pub(crate) use placement::RingPlacement;
#[cfg(target_os = "windows")]
pub(crate) use windows::RingPlacement;

/// Keep the overlay out of the Dock and app switcher.
#[cfg(target_os = "macos")]
pub fn configure_application() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    if let Some(marker) = MainThreadMarker::new() {
        NSApplication::sharedApplication(marker)
            .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    }
}

/// Make the transparent ring panel borderless and remove its native shadow.
#[cfg(target_os = "macos")]
pub fn configure_windows() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSWindowStyleMask};

    if let Some(marker) = MainThreadMarker::new() {
        for window in NSApplication::sharedApplication(marker).windows() {
            window.setStyleMask(NSWindowStyleMask::NonactivatingPanel);
            window.setHasShadow(false);
            shape_glass(&window);
        }
    }
}

/// Whether the ring can be drawn as glass: macOS blurs behind the window unless
/// the user turned on "Reduce transparency", which leaves no blur to read over.
#[cfg(target_os = "macos")]
pub fn glass_enabled() -> bool {
    !objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceTransparency()
}

/// No behind-window blur away from macOS.
#[cfg(not(target_os = "macos"))]
pub fn glass_enabled() -> bool {
    false
}

/// Turn gpui's window-sized behind-window blur into a frosted disc under the
/// ring panel by masking it to the panel circle, so the square window's
/// corners stay clear. The blur's own material tint is not set here: gpui's
/// blur view strips every sublayer fill on update, so the ring's gpui-drawn
/// gradient provides all of the tint.
#[cfg(target_os = "macos")]
fn shape_glass(window: &objc2_app_kit::NSWindow) {
    use objc2_app_kit::NSVisualEffectView;

    let Some(content) = window.contentView() else {
        return;
    };
    for view in content.subviews() {
        let Ok(glass) = view.downcast::<NSVisualEffectView>() else {
            continue;
        };
        let mask = circle_mask(glass.bounds().size, f64::from(crate::ring::PANEL_INSET));
        glass.setMaskImage(Some(&mask));
    }
}

/// An image the size of the blur view whose only opaque pixels are the panel
/// circle (`inset` from every edge).
#[cfg(target_os = "macos")]
fn circle_mask(
    size: objc2_foundation::NSSize,
    inset: f64,
) -> objc2::rc::Retained<objc2_app_kit::NSImage> {
    use objc2::runtime::Bool;
    use objc2_app_kit::{NSBezierPath, NSColor, NSImage};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let draw = block2::RcBlock::new(move |bounds: NSRect| -> Bool {
        let oval = NSRect::new(
            NSPoint::new(bounds.origin.x + inset, bounds.origin.y + inset),
            NSSize::new(
                bounds.size.width - 2.0 * inset,
                bounds.size.height - 2.0 * inset,
            ),
        );
        NSColor::blackColor().setFill();
        NSBezierPath::bezierPathWithOvalInRect(oval).fill();
        Bool::YES
    });
    NSImage::imageWithSize_flipped_drawingHandler(size, false, &draw)
}

/// No native application policy is required away from macOS.
#[cfg(not(target_os = "macos"))]
pub fn configure_application() {}

/// Linux needs no additional native window configuration here.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn configure_windows() {}

/// Owner of the native click-away event monitor; dropping it removes the
/// monitor. Create and drop on the main thread.
#[cfg(target_os = "macos")]
pub struct ClickAwayMonitor(objc2::rc::Retained<objc2::runtime::AnyObject>);

#[cfg(target_os = "macos")]
impl Drop for ClickAwayMonitor {
    #[expect(
        unsafe_code,
        reason = "NSEvent::removeMonitor is plain AppKit FFI; the token is exactly what addGlobalMonitor returned"
    )]
    fn drop(&mut self) {
        // SAFETY: `self.0` is the monitor token returned by
        // `addGlobalMonitorForEventsMatchingMask_handler`, removed only once.
        unsafe { objc2_app_kit::NSEvent::removeMonitor(&self.0) };
    }
}

/// Invoke `on_mouse_down` for every mouse-down that macOS delivers to *other*
/// applications, for as long as the returned monitor is held.
///
/// Global `NSEvent` monitors never see events routed to this process's own
/// windows and cannot consume the events they observe — together exactly the
/// ring's click-away contract: clicks on the ring keep hitting the GPUI
/// handlers they always did, while a click anywhere else can dismiss the ring
/// without being swallowed. Must be called on the main thread (returns `None`
/// off it); the handler runs on the main run loop.
#[cfg(target_os = "macos")]
pub fn watch_clicks_outside(on_mouse_down: impl Fn() + 'static) -> Option<ClickAwayMonitor> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSEvent, NSEventMask};

    MainThreadMarker::new()?;
    let handler: block2::RcBlock<dyn Fn(std::ptr::NonNull<NSEvent>)> =
        block2::RcBlock::new(move |_event| on_mouse_down());
    NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
        NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown,
        &handler,
    )
    .map(ClickAwayMonitor)
}

/// Away from macOS no global click monitor is available; the ring keeps its
/// in-window dismissal paths (center ×, slot activation, timeout).
#[cfg(not(target_os = "macos"))]
pub struct ClickAwayMonitor(());

#[cfg(not(target_os = "macos"))]
pub fn watch_clicks_outside(_on_mouse_down: impl Fn() + 'static) -> Option<ClickAwayMonitor> {
    None
}

/// One display's global geometry, in the same top-left-origin global point
/// space that `openlogi_hook::cursor_position()` reports.
#[cfg(not(target_os = "windows"))]
pub struct CursorDisplay {
    /// Native display id; on macOS the `CGDirectDisplayID`, numerically equal
    /// to GPUI's `DisplayId` for the same display.
    pub id: u64,
    /// Global origin (top-left corner) of the display, in points.
    pub origin: (f64, f64),
    /// Display size in points.
    pub size: (f64, f64),
}

/// Find the display whose global bounds contain the point `(x, y)`.
///
/// GPUI's `PlatformDisplay::bounds()` zeroes every display's origin (window
/// bounds are display-relative), so mapping a global cursor position to its
/// display has to go through CoreGraphics.
#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "CGGetActiveDisplayList/CGDisplayBounds are plain C FFI; GPUI exposes no global display bounds"
)]
pub fn display_containing(x: f64, y: f64) -> Option<CursorDisplay> {
    use core_graphics::display::{CGDisplayBounds, CGGetActiveDisplayList};

    const MAX_DISPLAYS: u32 = 32;
    let mut ids = [0u32; MAX_DISPLAYS as usize];
    let mut count = 0u32;
    // SAFETY: the list write is bounded by the capacity we pass; `count`
    // reports how many entries were actually filled.
    let result = unsafe { CGGetActiveDisplayList(MAX_DISPLAYS, ids.as_mut_ptr(), &raw mut count) };
    if result != 0 {
        return None;
    }
    ids.iter().take(count as usize).find_map(|&id| {
        // SAFETY: side-effect-free C getter on an id from the active list.
        let bounds = unsafe { CGDisplayBounds(id) };
        let contains = x >= bounds.origin.x
            && x < bounds.origin.x + bounds.size.width
            && y >= bounds.origin.y
            && y < bounds.origin.y + bounds.size.height;
        contains.then(|| CursorDisplay {
            id: u64::from(id),
            origin: (bounds.origin.x, bounds.origin.y),
            size: (bounds.size.width, bounds.size.height),
        })
    })
}

/// On Linux the GPUI display list already carries global origins, so there is
/// nothing to resolve natively.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn display_containing(_x: f64, _y: f64) -> Option<CursorDisplay> {
    None
}
