//! Shortcut manager: registers SnapRelay's four shortcuts from the settings, and
//! swaps one for another while running (Settings window). If a default shortcut is
//! already taken by another app at start-up, a fallback is tried; a shortcut the
//! user picked themselves has no fallback, it's shown as "not active" instead.

use std::cell::RefCell;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT};

use crate::settings::{self, Combo, Shortcut, ALT, CTRL, SHIFT, WIN};

/// Hotkey ids as Windows reports them in WM_HOTKEY.
pub fn id(s: Shortcut) -> i32 {
    s.index() as i32 + 1
}

pub fn from_id(id: i32) -> Option<Shortcut> {
    Shortcut::ALL.into_iter().find(|s| self::id(*s) == id)
}

thread_local! {
    /// What's really registered for each shortcut right now (None = not active).
    static ACTIVE: RefCell<[Option<Combo>; 4]> = const { RefCell::new([None; 4]) };
    /// While the Settings window listens for a new combination, nothing is registered.
    static SUSPENDED: RefCell<bool> = const { RefCell::new(false) };
}

fn register(s: Shortcut, c: Combo) -> bool {
    unsafe { RegisterHotKey(HWND::default(), id(s), HOT_KEY_MODIFIERS(c.mods) | MOD_NOREPEAT, c.vk).is_ok() }
}

fn unregister(s: Shortcut) {
    unsafe {
        let _ = UnregisterHotKey(HWND::default(), id(s));
    }
}

/// The shortcuts to try for `s`: the chosen one, plus a fallback if it's the default.
fn candidates(s: Shortcut) -> Vec<Combo> {
    let chosen = settings::get().combos[s.index()];
    let mut list = vec![chosen];
    if chosen == s.default_combo() {
        list.push(match s {
            // No Alt for paste: a released Alt sends many apps to their menu bar.
            Shortcut::Paste => Combo { mods: WIN | SHIFT, vk: 0x56 }, // Win+Shift+V
            _ => Combo { mods: CTRL | ALT | SHIFT, vk: chosen.vk },
        });
    }
    list
}

/// Register all four from the settings (start-up). Prints what happened.
pub fn register_all() {
    println!("SnapRelay shortcuts:");
    for s in Shortcut::ALL {
        let mut got = None;
        for (i, c) in candidates(s).into_iter().enumerate() {
            if register(s, c) {
                println!("  {:<18} {}{}", settings::label(c), s.title(), if i > 0 { "   (fallback)" } else { "" });
                got = Some(c);
                break;
            }
            println!("  {:<18} taken by Windows or another app", settings::label(c));
        }
        ACTIVE.with(|a| a.borrow_mut()[s.index()] = got);
    }
    refresh_hint();
}

/// "Win+Shift+D", or "not set" if no shortcut is active for it.
pub fn label(s: Shortcut) -> String {
    ACTIVE.with(|a| a.borrow()[s.index()]).map(settings::label).unwrap_or_else(|| "not set".into())
}

pub fn is_active(s: Shortcut) -> bool {
    ACTIVE.with(|a| a.borrow()[s.index()].is_some())
}

/// Stop all shortcuts (so the Settings window can hear any combination).
pub fn suspend() {
    if SUSPENDED.with(|x| x.replace(true)) {
        return;
    }
    for s in Shortcut::ALL {
        unregister(s);
    }
}

/// Put the active shortcuts back after `suspend`.
pub fn resume() {
    if !SUSPENDED.with(|x| x.replace(false)) {
        return;
    }
    for s in Shortcut::ALL {
        if let Some(c) = ACTIVE.with(|a| a.borrow()[s.index()]) {
            if !register(s, c) {
                ACTIVE.with(|a| a.borrow_mut()[s.index()] = None);
            }
        }
    }
    refresh_hint();
}

/// Use `c` for `s` from now on. Checks it's usable and free; on failure the old
/// one stays. Call while suspended (the Settings window does).
pub fn change(s: Shortcut, c: Combo) -> Result<(), String> {
    if c.mods & (WIN | CTRL | ALT) == 0 {
        return Err("Use Win, Ctrl or Alt together with a key".into());
    }
    if s == Shortcut::Paste && c.mods & ALT != 0 {
        return Err("The paste shortcut can't use Alt (it moves focus off the chat box)".into());
    }
    if let Some(other) = Shortcut::ALL.into_iter().find(|o| *o != s && ACTIVE.with(|a| a.borrow()[o.index()]) == Some(c)) {
        return Err(format!("{} is already used for {}", settings::label(c), other.title()));
    }
    // Try it for real: only Windows knows if another app holds it.
    if !register(s, c) {
        return Err(format!("{} is taken by Windows or another app", settings::label(c)));
    }
    unregister(s); // registered again by `resume`
    ACTIVE.with(|a| a.borrow_mut()[s.index()] = Some(c));
    settings::update(|st| st.combos[s.index()] = c);
    Ok(())
}

/// Back to the default shortcuts (with fallbacks). Call while suspended.
pub fn reset() {
    settings::update(|st| st.combos = Shortcut::ALL.map(Shortcut::default_combo));
    for s in Shortcut::ALL {
        let got = candidates(s).into_iter().find(|c| {
            let ok = register(s, *c);
            unregister(s);
            ok
        });
        ACTIVE.with(|a| a.borrow_mut()[s.index()] = got);
    }
}

/// The queue window's empty state shows the capture shortcuts.
fn refresh_hint() {
    crate::queue::set_hint(format!(
        "{} screen  \u{00B7}  {} area",
        label(Shortcut::Screen), label(Shortcut::Area)
    ));
}
