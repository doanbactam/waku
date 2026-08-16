//! Waku's single keyboard-accessibility convention, shared verbatim across
//! the macOS, Windows and Linux builds.
//!
//! The rules (also recorded in AGENTS.md, "Keyboard accessibility"):
//! - The primary shortcut modifier is `Modifiers::secondary()` — Cmd on
//!   macOS, Ctrl on Windows and Linux. Never use `Modifiers::platform` for a
//!   shortcut: on Linux it is the Super key and on Windows the Win key, not
//!   the primary. Prefer binding shortcuts with the `secondary-` keybinding
//!   prefix so the keymap resolves the right modifier per platform.
//! - A control activates on Enter or Space pressed without the primary
//!   modifier. [`keyboard_activate`] wires that up in one place so every
//!   widget behaves the same on all three platforms.

use gpui::{Context, Div, InteractiveElement, KeyDownEvent, Keystroke, Stateful, Styled, Window};

use crate::theme::Theme;

/// True when the keystroke is the standard "activate the focused control"
/// gesture: Enter or Space without the platform primary modifier (Cmd on
/// macOS, Ctrl on Windows/Linux).
pub fn is_activation_keystroke(keystroke: &Keystroke) -> bool {
    !keystroke.modifiers.secondary() && matches!(keystroke.key.as_str(), "enter" | "space")
}

/// Attach the canonical Enter/Space activation handler to a focusable control.
///
/// `on_activate` runs only for an [`is_activation_keystroke`] press, and the
/// keystroke is stopped so it can't fall through to an outer surface. This is
/// the one shared place a control learns to react to the keyboard; every
/// widget should route its key activation here so the feel is consistent
/// across platforms and the primary-modifier guard stays correct.
pub fn keyboard_activate<T, F>(
    element: Stateful<Div>,
    cx: &mut Context<T>,
    on_activate: F,
) -> Stateful<Div>
where
    T: 'static,
    F: FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
{
    // `cx.listener` takes `Fn`, so a mutable activation callback is parked in
    // a `RefCell` at one shared call site instead of forcing each widget to
    // duplicate the Enter/Space logic.
    let on_activate = std::rc::Rc::new(std::cell::RefCell::new(on_activate));
    element.on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
        if is_activation_keystroke(&event.keystroke) {
            on_activate.borrow_mut()(this, window, cx);
            cx.stop_propagation();
        }
    }))
}

/// The standard focusable-control surface: reachable by Tab, and the window
/// draws a visible ring only when focus was reached by the keyboard — GPUI
/// suppresses it for mouse clicks, so it reads as keyboard guidance rather
/// than permanent decoration.
pub fn focus_ring(element: Stateful<Div>, theme: &Theme) -> Stateful<Div> {
    element
        .tab_index(0)
        .focus_visible(|style| style.border_1().border_color(theme.accent))
}
