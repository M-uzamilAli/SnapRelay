//! Settings store: what the user can change in the Settings window, kept in
//! %APPDATA%\SnapRelay\settings.txt as plain "name=value" lines. Anything missing
//! or unreadable falls back to the default, so a broken file never stops SnapRelay.
//! "Start with Windows" isn't in the file: it lives in Windows' own sign-in list.

use std::cell::RefCell;
use std::path::PathBuf;

use windows::core::{w, PCWSTR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyNameTextW, MapVirtualKeyW, MAPVK_VK_TO_VSC};

// Modifier bits, as RegisterHotKey wants them.
pub const ALT: u32 = 0x1;
pub const CTRL: u32 = 0x2;
pub const SHIFT: u32 = 0x4;
pub const WIN: u32 = 0x8;

/// A key combination, e.g. Win+Shift+D.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Combo {
    pub mods: u32,
    pub vk: u32,
}

/// SnapRelay's four shortcuts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shortcut {
    Screen,
    Area,
    Paste,
    Queue,
}

impl Shortcut {
    pub const ALL: [Shortcut; 4] = [Shortcut::Screen, Shortcut::Area, Shortcut::Paste, Shortcut::Queue];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn title(self) -> &'static str {
        match self {
            Shortcut::Screen => "Capture screen",
            Shortcut::Area => "Capture area",
            Shortcut::Paste => "Paste into chat",
            Shortcut::Queue => "Open queue",
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Shortcut::Screen => "screen",
            Shortcut::Area => "area",
            Shortcut::Paste => "paste",
            Shortcut::Queue => "queue",
        }
    }

    /// Z, X, D sit together under the left hand: paste, area, whole screen.
    pub fn default_combo(self) -> Combo {
        let vk = match self {
            Shortcut::Screen => 0x44, // D
            Shortcut::Area => 0x58,   // X
            Shortcut::Paste => 0x5A,  // Z
            Shortcut::Queue => 0x51,  // Q
        };
        Combo { mods: WIN | SHIFT, vk }
    }
}

/// Memory limit choices (MB) and "hide the queue when idle" choices (seconds, 0 = never).
pub const LIMITS_MB: [u32; 3] = [100, 200, 500];
pub const IDLE_CHOICES: [u32; 4] = [5, 8, 15, 0];

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub combos: [Combo; 4],
    pub sound: bool,
    pub flash: bool,
    pub limit_mb: u32,
    pub idle_secs: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            combos: Shortcut::ALL.map(Shortcut::default_combo),
            sound: true,
            flash: true,
            limit_mb: 200,
            idle_secs: 8,
        }
    }
}

thread_local! {
    static CURRENT: RefCell<Settings> = RefCell::new(Settings::default());
}

fn file() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("SnapRelay").join("settings.txt"))
}

/// Read the settings file (if any) into memory. Call once at start-up.
pub fn load() {
    let s = file().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| parse(&t)).unwrap_or_default();
    CURRENT.with(|c| *c.borrow_mut() = s);
}

pub fn get() -> Settings {
    CURRENT.with(|c| c.borrow().clone())
}

/// Change something and save it straight away.
pub fn update(change: impl FnOnce(&mut Settings)) {
    let s = CURRENT.with(|c| {
        let mut c = c.borrow_mut();
        change(&mut c);
        c.clone()
    });
    if let Some(p) = file() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&p, format(&s)) {
            println!("Couldn't save settings to {}: {e}", p.display());
        }
    }
}

fn format(s: &Settings) -> String {
    let mut out = String::from("# SnapRelay settings. Shortcuts are modifier bits,key code (Alt 1, Ctrl 2, Shift 4, Win 8).\n");
    for sc in Shortcut::ALL {
        let c = s.combos[sc.index()];
        out += &format!("{}={},{}\n", sc.file_name(), c.mods, c.vk);
    }
    let on = |b: bool| if b { "on" } else { "off" };
    out += &format!("sound={}\nflash={}\nqueue_limit_mb={}\nidle_secs={}\n", on(s.sound), on(s.flash), s.limit_mb, s.idle_secs);
    out
}

fn parse(text: &str) -> Settings {
    let mut s = Settings::default();
    for line in text.lines() {
        let Some((name, value)) = line.split_once('=') else { continue };
        let (name, value) = (name.trim(), value.trim());
        if let Some(sc) = Shortcut::ALL.into_iter().find(|sc| sc.file_name() == name) {
            if let Some((m, v)) = value.split_once(',') {
                if let (Ok(mods), Ok(vk)) = (m.trim().parse(), v.trim().parse()) {
                    if mods & (WIN | CTRL | ALT) != 0 && vk != 0 {
                        s.combos[sc.index()] = Combo { mods, vk };
                    }
                }
            }
            continue;
        }
        match name {
            "sound" => s.sound = value != "off",
            "flash" => s.flash = value != "off",
            "queue_limit_mb" => {
                if let Ok(v) = value.parse() {
                    if LIMITS_MB.contains(&v) {
                        s.limit_mb = v;
                    }
                }
            }
            "idle_secs" => {
                if let Ok(v) = value.parse() {
                    if IDLE_CHOICES.contains(&v) {
                        s.idle_secs = v;
                    }
                }
            }
            _ => {}
        }
    }
    s
}

/// "Win+Shift+D"
pub fn label(c: Combo) -> String {
    let mut parts = Vec::new();
    for (bit, name) in [(WIN, "Win"), (CTRL, "Ctrl"), (ALT, "Alt"), (SHIFT, "Shift")] {
        if c.mods & bit != 0 {
            parts.push(name.to_string());
        }
    }
    parts.push(key_name(c.vk));
    parts.join("+")
}

fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).map(String::from).unwrap_or_default(),
        0x60..=0x69 => format!("Num{}", vk - 0x60),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => unsafe {
            let scan = MapVirtualKeyW(vk, MAPVK_VK_TO_VSC);
            let mut buf = [0u16; 32];
            let n = GetKeyNameTextW((scan << 16) as i32, &mut buf);
            if n > 0 { String::from_utf16_lossy(&buf[..n as usize]) } else { format!("Key {vk}") }
        },
    }
}

// ---------- Start with Windows (the user's sign-in list in the registry) ----------

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_NAME: PCWSTR = w!("SnapRelay");

pub fn starts_with_windows() -> bool {
    let mut size = 0u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME, RRF_RT_REG_SZ, None, None, Some(&mut size)).is_ok() }
}

pub fn set_start_with_windows(on: bool) -> Result<(), String> {
    unsafe {
        let mut key = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_SET_VALUE, &mut key)
            .ok()
            .map_err(|e| format!("couldn't open the sign-in list: {e}"))?;
        let result = if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let value: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain([0]).collect();
            let bytes = std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2);
            RegSetValueExW(key, RUN_NAME, 0, REG_SZ, Some(bytes)).ok().map_err(|e| e.to_string())
        } else {
            let r = RegDeleteValueW(key, RUN_NAME);
            if r.is_ok() || !starts_with_windows() { Ok(()) } else { Err(format!("{:?}", r)) }
        };
        let _ = RegCloseKey(key);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_round_trip_and_bad_lines() {
        let mut s = Settings::default();
        s.combos[Shortcut::Area.index()] = Combo { mods: CTRL | ALT, vk: 0x53 };
        s.sound = false;
        s.limit_mb = 500;
        s.idle_secs = 0;
        assert_eq!(parse(&format(&s)), s);
        // Junk and out-of-range values fall back to the defaults.
        let t = "screen=0,68\nqueue_limit_mb=7\nidle_secs=abc\nnonsense\nflash=off";
        let p = parse(t);
        assert_eq!(p.combos[0], Shortcut::Screen.default_combo());
        assert_eq!((p.limit_mb, p.idle_secs, p.flash), (200, 8, false));
    }

    #[test]
    fn labels() {
        assert_eq!(label(Combo { mods: WIN | SHIFT, vk: 0x44 }), "Win+Shift+D");
        assert_eq!(label(Combo { mods: CTRL | ALT | SHIFT, vk: 0x70 }), "Ctrl+Alt+Shift+F1");
        assert_eq!(label(Combo { mods: CTRL, vk: 0x63 }), "Ctrl+Num3");
    }
}
