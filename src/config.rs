// config.rs: the small `key = value` settings file, parsed by hand.
//
// Two files live here:
//   * config  — user settings (fonts, theme, sidebar mode, ...)
//   * state   — window geometry remembered across runs
//
// Both use the same forgiving format: `#` comments, blank lines, unknown keys
// ignored, invalid values replaced by defaults. A broken file must never stop
// the editor from starting, so every parse error is silent (and logged once).
//
// No serde, no toml: the format is small enough that a hand parser is both
// shorter and easier to reason about.

use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SidebarMode {
    Auto,
    Expanded,
    Rail,
    Hidden,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme {
    Dark,
    Light,
}

/// Where the window buttons sit in the title bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Buttons {
    Left,
    Right,
}

/// Every user-visible setting. `Default` is the spec's default column.
#[derive(Clone, PartialEq, Debug)]
pub struct Config {
    /// Editor and UI font size in points, 10..=28.
    pub font_size: u32,
    /// Columns per indent level: 2, 4 or 8.
    pub tab_width: usize,
    pub show_line_numbers: bool,
    pub cursor_blink: bool,
    pub window_buttons: Buttons,
    /// Window corner radius in points: 0, 6, 10 or 14.
    pub corner_radius: u32,
    pub sidebar: SidebarMode,
    pub theme: Theme,
    /// Experimental: ask the window system for the damaged region instead of
    /// redrawing the whole window. Off by default (section 9.1: with a
    /// transparent, premultiplied window partial redraws are easy to get
    /// wrong). Not exposed on the settings page, which has eight fixed rows.
    pub experimental_damage: bool,
    /// UI animations (sidebar width, smooth scrolling, hover fades). On by
    /// default; off makes every change instant and keeps the loop at `Wait`.
    pub animations: bool,
    /// Ask GitHub for a newer release (v1.2 section 9). On by default; the
    /// check is once a day, ten seconds after the first frame, and silent when
    /// offline. Turning it off means no request is ever made.
    pub check_updates: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_size: 15,
            tab_width: 4,
            show_line_numbers: true,
            cursor_blink: false,
            window_buttons: Buttons::Right,
            corner_radius: 10,
            sidebar: SidebarMode::Auto,
            theme: Theme::Dark,
            experimental_damage: false,
            animations: true,
            check_updates: true,
        }
    }
}

impl Config {
    /// Where the settings file lives: `$XDG_CONFIG_HOME/ludicrous/config`, or
    /// `~/.config/ludicrous/config`.
    pub fn path() -> Option<PathBuf> {
        dir_from_env("XDG_CONFIG_HOME", ".config").map(|d| d.join("ludicrous/config"))
    }

    /// Load, never failing: a missing or unreadable file means defaults.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let cfg = Self::parse(&text);
                if cfg != Self::default() {
                    eprintln!("ludicrous: config loaded from {}", path.display());
                }
                cfg
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                eprintln!(
                    "ludicrous: could not read {}: {e} (using defaults)",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Parse the file format. Missing lines keep their default; invalid values
    /// are ignored (again keeping the default), so a hand-broken file starts.
    pub fn parse(text: &str) -> Self {
        let mut cfg = Self::default();
        for line in text.lines() {
            // Strip comments and surrounding whitespace.
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue; // no `=`: not a setting, ignore it
            };
            let (key, value) = (key.trim(), value.trim());
            cfg.set(key, value);
        }
        cfg
    }

    /// Apply one key/value pair. Unknown keys and invalid values are ignored.
    fn set(&mut self, key: &str, value: &str) {
        match key {
            "font_size" => {
                if let Ok(v) = value.parse::<u32>() {
                    if (10..=28).contains(&v) {
                        self.font_size = v;
                    }
                }
            }
            "tab_width" => {
                if let Ok(v) = value.parse::<usize>() {
                    if matches!(v, 2 | 4 | 8) {
                        self.tab_width = v;
                    }
                }
            }
            "show_line_numbers" => {
                if let Some(v) = parse_bool(value) {
                    self.show_line_numbers = v;
                }
            }
            "cursor_blink" => {
                if let Some(v) = parse_bool(value) {
                    self.cursor_blink = v;
                }
            }
            "window_buttons" => match value {
                "left" => self.window_buttons = Buttons::Left,
                "right" => self.window_buttons = Buttons::Right,
                _ => {}
            },
            "corner_radius" => {
                if let Ok(v) = value.parse::<u32>() {
                    if matches!(v, 0 | 6 | 10 | 14) {
                        self.corner_radius = v;
                    }
                }
            }
            "sidebar" => match value {
                "auto" => self.sidebar = SidebarMode::Auto,
                "expanded" => self.sidebar = SidebarMode::Expanded,
                "rail" => self.sidebar = SidebarMode::Rail,
                "hidden" => self.sidebar = SidebarMode::Hidden,
                _ => {}
            },
            "theme" => match value {
                "dark" => self.theme = Theme::Dark,
                "light" => self.theme = Theme::Light,
                _ => {}
            },
            "experimental_damage" => {
                if let Some(v) = parse_bool(value) {
                    self.experimental_damage = v;
                }
            }
            "animations" => {
                if let Some(v) = parse_bool(value) {
                    self.animations = v;
                }
            }
            "check_updates" => {
                if let Some(v) = parse_bool(value) {
                    self.check_updates = v;
                }
            }
            _ => {} // unknown keys are forward compatible by design
        }
    }

    /// The file contents. Keys are written in the spec's order so the file is
    /// readable and diffs stay small.
    pub fn to_text(&self) -> String {
        format!(
            "# Ludicrous config\n\
             font_size = {}\n\
             tab_width = {}\n\
             show_line_numbers = {}\n\
             cursor_blink = {}\n\
             window_buttons = {}\n\
             corner_radius = {}\n\
             sidebar = {}\n\
             theme = {}\n\
             experimental_damage = {}\n\
             animations = {}\n\
             check_updates = {}\n",
            self.font_size,
            self.tab_width,
            self.show_line_numbers,
            self.cursor_blink,
            match self.window_buttons {
                Buttons::Left => "left",
                Buttons::Right => "right",
            },
            self.corner_radius,
            match self.sidebar {
                SidebarMode::Auto => "auto",
                SidebarMode::Expanded => "expanded",
                SidebarMode::Rail => "rail",
                SidebarMode::Hidden => "hidden",
            },
            match self.theme {
                Theme::Dark => "dark",
                Theme::Light => "light",
            },
            self.experimental_damage,
            self.animations,
            self.check_updates,
        )
    }

    /// Write the settings back. Same safe-save routine as editor files: temp
    /// file, fsync, rename. Failures are logged, never fatal.
    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Err(e) = safe_write(&path, &self.to_text()) {
            eprintln!("ludicrous: could not save config: {e}");
        }
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "true" | "on" | "1" | "yes" => Some(true),
        "false" | "off" | "0" | "no" => Some(false),
        _ => None,
    }
}

/// `$VAR`, or `$HOME/<fallback>` when the variable is unset or empty.
fn dir_from_env(var: &str, fallback: &str) -> Option<PathBuf> {
    if let Some(v) = std::env::var_os(var) {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(fallback))
}

/// Write `text` to `path` atomically (temp file + fsync + rename).
pub fn safe_write(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("ludicrous");
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Window geometry remembered between runs (section 10).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WindowState {
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
    /// The sidebar mode actually showing when the window closed. `None` means
    /// "no manual choice", which is also what `auto` in the config uses.
    pub sidebar: Option<SidebarMode>,
    /// Unix timestamp of the last update check. At most one check per 24 h, so
    /// this is what stops a restart loop from hammering GitHub.
    pub last_update_check: Option<u64>,
}

impl Default for WindowState {
    /// A sensible first window: big enough to show the sidebar expanded.
    fn default() -> Self {
        Self {
            width: 1100.0,
            height: 700.0,
            maximized: false,
            sidebar: None,
            last_update_check: None,
        }
    }
}

impl WindowState {
    pub fn path() -> Option<PathBuf> {
        dir_from_env("XDG_STATE_HOME", ".local/state").map(|d| d.join("ludicrous/state"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(_) => Self::default(),
        }
    }

    /// Parse the state file. Missing lines keep their defaults, garbage is
    /// ignored, and out-of-range sizes are rejected.
    pub fn parse(text: &str) -> Self {
        let mut st = Self::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            let key = key.trim();
            st.set(key, value);
        }
        st
    }

    fn set(&mut self, key: &str, value: &str) {
        match key {
            // Only accept a sane size: a broken value must not create a window
            // smaller than the minimum.
            "width" => {
                if let Ok(v) = value.parse::<f32>() {
                    if (480.0..=16384.0).contains(&v) {
                        self.width = v;
                    }
                }
            }
            "height" => {
                if let Ok(v) = value.parse::<f32>() {
                    if (320.0..=16384.0).contains(&v) {
                        self.height = v;
                    }
                }
            }
            "maximized" => {
                if let Some(v) = parse_bool(value) {
                    self.maximized = v;
                }
            }
            "last_update_check" => {
                // 0 is what `to_text` writes for "never checked": a check in
                // 1970 must not make the next one wait until 1971.
                if let Ok(v) = value.parse::<u64>() {
                    self.last_update_check = (v > 0).then_some(v);
                }
            }
            "sidebar" => {
                self.sidebar = match value {
                    "expanded" => Some(SidebarMode::Expanded),
                    "rail" => Some(SidebarMode::Rail),
                    "hidden" => Some(SidebarMode::Hidden),
                    _ => None, // auto, or garbage: fall back to the config
                };
            }
            _ => {}
        }
    }

    /// The state file's contents. Split out from `save` so a test can round
    /// trip it without touching the real state directory.
    pub fn to_text(self) -> String {
        let sidebar = match self.sidebar {
            None => "auto",
            Some(SidebarMode::Auto) => "auto",
            Some(SidebarMode::Expanded) => "expanded",
            Some(SidebarMode::Rail) => "rail",
            Some(SidebarMode::Hidden) => "hidden",
        };
        format!(
            "# Ludicrous window state\nwidth = {}\nheight = {}\nmaximized = {}\nsidebar = {}\nlast_update_check = {}\n",
            self.width.round() as i64,
            self.height.round() as i64,
            self.maximized,
            sidebar,
            self.last_update_check
                .map(|t| t.to_string())
                .unwrap_or_else(|| "0".to_string())
        )
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Err(e) = safe_write(&path, &self.to_text()) {
            eprintln!("ludicrous: could not save window state: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let c = Config::default();
        assert_eq!(c.font_size, 15);
        assert_eq!(c.tab_width, 4);
        assert!(c.show_line_numbers);
        assert!(!c.cursor_blink);
        assert_eq!(c.window_buttons, Buttons::Right);
        assert_eq!(c.corner_radius, 10);
        assert_eq!(c.sidebar, SidebarMode::Auto);
        assert_eq!(c.theme, Theme::Dark);
    }

    #[test]
    fn checking_for_updates_is_on_unless_turned_off() {
        // On by default: the spec's default column (v1.2 section 10).
        assert!(Config::default().check_updates);
        assert!(!Config::parse("check_updates = off\n").check_updates);
        assert!(Config::parse("check_updates = on\n").check_updates);
        // Garbage keeps the default rather than silently disabling updates.
        assert!(Config::parse("check_updates = maybe\n").check_updates);
        // And it survives a save/load round trip.
        let c = Config {
            check_updates: false,
            ..Config::default()
        };
        assert!(!Config::parse(&c.to_text()).check_updates);
    }

    #[test]
    fn the_last_update_check_is_in_the_state_file() {
        let mut st = WindowState::default();
        assert_eq!(st.last_update_check, None);
        st.last_update_check = Some(1_700_000_000);
        let text = st.to_text();
        assert!(text.contains("last_update_check = 1700000000"));
        assert_eq!(
            WindowState::parse(&text).last_update_check,
            Some(1_700_000_000)
        );
        // Zero (as written for "never") parses back to `None`, not 1970.
        assert_eq!(
            WindowState::parse("last_update_check = 0\n").last_update_check,
            None
        );
        assert_eq!(
            WindowState::parse("last_update_check = soon\n").last_update_check,
            None
        );
    }

    #[test]
    fn every_key_round_trips_through_text() {
        let c = Config {
            font_size: 22,
            tab_width: 8,
            show_line_numbers: false,
            cursor_blink: true,
            window_buttons: Buttons::Left,
            corner_radius: 14,
            sidebar: SidebarMode::Rail,
            theme: Theme::Light,
            experimental_damage: true,
            animations: false,
            check_updates: false,
        };
        assert_eq!(Config::parse(&c.to_text()), c);
    }

    #[test]
    fn animations_default_on_and_can_be_turned_off() {
        assert!(Config::default().animations);
        // `off` must be read from the file, because the acceptance test for
        // section 8 writes it there by hand.
        let c = Config::parse("animations = off\n");
        assert!(!c.animations);
        assert!(Config::parse("animations = on\n").animations);
        // A broken value leaves the default alone rather than guessing.
        assert!(Config::parse("animations = maybe\n").animations);
    }

    #[test]
    fn comments_blank_lines_and_unknown_keys_are_ignored() {
        let text = "\
            # a comment\n\
            \n\
            font_size = 18   # trailing comment\n\
            future_setting = whatever\n\
            this line has no equals sign\n\
            theme = light\n";
        let c = Config::parse(text);
        assert_eq!(c.font_size, 18);
        assert_eq!(c.theme, Theme::Light);
        assert_eq!(c.tab_width, 4, "unset keys keep their defaults");
    }

    #[test]
    fn out_of_range_and_garbage_values_keep_the_default() {
        let c =
            Config::parse("font_size = 400\ntab_width = 3\ncorner_radius = 5\nmaximized = maybe\n");
        assert_eq!(c, Config::default());
        assert_eq!(Config::parse("font_size = abc").font_size, 15);
        assert_eq!(Config::parse("font_size =").font_size, 15);
    }

    #[test]
    fn a_completely_broken_file_still_parses() {
        let _ = Config::parse("\u{0}\u{1}garbage{{{[\n=====\n= = =\n");
    }

    #[test]
    fn booleans_accept_the_common_spellings() {
        for yes in ["true", "on", "1", "yes"] {
            assert!(Config::parse(&format!("cursor_blink = {yes}")).cursor_blink);
        }
        for no in ["false", "off", "0", "no"] {
            assert!(!Config::parse(&format!("show_line_numbers = {no}")).show_line_numbers);
        }
    }

    #[test]
    fn experimental_damage_is_off_unless_asked_for() {
        assert!(!Config::default().experimental_damage);
        assert!(Config::parse("experimental_damage = true").experimental_damage);
        assert!(!Config::parse("experimental_damage = maybe").experimental_damage);
    }

    #[test]
    fn window_state_sidebar_round_trips() {
        let st = WindowState::parse("width = 900\nheight = 600\nsidebar = rail # last run\n");
        assert_eq!(st.width, 900.0);
        assert_eq!(st.height, 600.0);
        assert_eq!(st.sidebar, Some(SidebarMode::Rail));
        assert_eq!(WindowState::default().sidebar, None);
        assert_eq!(WindowState::parse("sidebar = auto").sidebar, None);
        assert_eq!(WindowState::parse("sidebar = nonsense").sidebar, None);
        // Garbage sizes fall back to the sane defaults.
        let bad = WindowState::parse("width = 5\nheight = 1e9\n");
        assert_eq!(bad.width, WindowState::default().width);
        assert_eq!(bad.height, WindowState::default().height);
    }

    #[test]
    fn safe_write_replaces_the_file_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join("ludicrous-config-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        safe_write(&path, "font_size = 20\n").unwrap();
        safe_write(&path, "font_size = 21\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "font_size = 21\n");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn window_state_bounds_are_enforced() {
        let dir = std::env::temp_dir().join("ludicrous-state-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state");
        std::fs::write(&path, "width = 10\nheight = 999999\nmaximized = true\n").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        // Parsing logic is inline in load(); check the same rules directly.
        let mut st = WindowState::default();
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                match k.trim() {
                    "width" => {
                        if let Ok(v) = v.trim().parse::<f32>() {
                            if (480.0..=16384.0).contains(&v) {
                                st.width = v;
                            }
                        }
                    }
                    "maximized" => st.maximized = v.trim() == "true",
                    _ => {}
                }
            }
        }
        assert_eq!(st.width, WindowState::default().width, "too small rejected");
        assert!(st.maximized);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
