//! Settings kept between runs: light / dark mode and the recently opened project folders.
//!
//! Stored as plain `key=value` lines (one `recent=` line per folder, newest first) in
//! `%APPDATA%\react-graph\settings.txt` on Windows, `$XDG_CONFIG_HOME/react-graph` or
//! `~/.config/react-graph` elsewhere. A missing or unreadable file just means defaults.

use std::path::{Path, PathBuf};

/// How many recent projects are remembered.
pub const MAX_RECENT: usize = 10;

#[derive(Default, Clone, Debug, PartialEq)]
pub struct Prefs {
    /// "light", "dark", or empty to follow the OS
    pub theme: String,
    /// project folders, most recent first
    pub recent: Vec<PathBuf>,
}

pub(crate) fn dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.map(|b| b.join("react-graph"))
}

fn file() -> Option<PathBuf> {
    dir().map(|d| d.join("settings.txt"))
}

impl Prefs {
    pub fn load() -> Self {
        file().and_then(|f| std::fs::read_to_string(f).ok()).map(|s| Self::parse(&s)).unwrap_or_default()
    }

    /// Best effort: a failed write only loses the setting for the next run.
    pub fn save(&self) {
        let (Some(d), Some(f)) = (dir(), file()) else { return };
        if std::fs::create_dir_all(&d).is_ok() {
            let _ = std::fs::write(f, self.serialize());
        }
    }

    pub fn parse(s: &str) -> Self {
        let mut p = Prefs::default();
        for line in s.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "theme" => p.theme = v.to_string(),
                "recent" if !v.is_empty() => {
                    let path = PathBuf::from(v);
                    if !p.recent.iter().any(|r| same(r, &path)) && p.recent.len() < MAX_RECENT {
                        p.recent.push(path);
                    }
                }
                _ => {}
            }
        }
        p
    }

    pub fn serialize(&self) -> String {
        let mut s = format!("theme={}\n", self.theme);
        for r in &self.recent {
            s.push_str(&format!("recent={}\n", r.display()));
        }
        s
    }

    /// Put `path` at the top of the recent list (moving it up if it is already there).
    pub fn push_recent(&mut self, path: PathBuf) {
        self.recent.retain(|r| !same(r, &path));
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT);
    }

    pub fn remove_recent(&mut self, path: &Path) {
        self.recent.retain(|r| !same(r, path));
    }
}

/// Paths as the user sees them: without the `\\?\` prefix `canonicalize` adds on Windows.
pub fn display(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

/// Same folder, ignoring the `\\?\` prefix, a trailing separator and (on Windows) case.
pub(crate) fn same(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = display(p);
        let s = s.trim_end_matches(['/', '\\']);
        if cfg!(windows) { s.replace('/', "\\").to_lowercase() } else { s.to_string() }
    };
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut p = Prefs { theme: "dark".into(), recent: vec![] };
        p.push_recent(PathBuf::from("/a"));
        p.push_recent(PathBuf::from("/b"));
        assert_eq!(Prefs::parse(&p.serialize()), p);
        assert_eq!(p.recent, vec![PathBuf::from("/b"), PathBuf::from("/a")]);
    }

    #[test]
    fn push_moves_to_front_and_caps() {
        let mut p = Prefs::default();
        for i in 0..15 {
            p.push_recent(PathBuf::from(format!("/p{i}")));
        }
        assert_eq!(p.recent.len(), MAX_RECENT);
        assert_eq!(p.recent[0], PathBuf::from("/p14"));
        p.push_recent(PathBuf::from("/p10/"));
        assert_eq!(p.recent[0], PathBuf::from("/p10/"));
        assert_eq!(p.recent.len(), MAX_RECENT);
        assert_eq!(p.recent.iter().filter(|r| r.to_string_lossy().starts_with("/p10")).count(), 1);
    }

    #[test]
    fn junk_lines_are_ignored() {
        let p = Prefs::parse("nonsense\ntheme = light\nrecent=\nrecent=/x\nrecent=/x\n");
        assert_eq!(p.theme, "light");
        assert_eq!(p.recent, vec![PathBuf::from("/x")]);
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_match_loosely() {
        let mut p = Prefs::default();
        p.push_recent(PathBuf::from(r"\\?\E:\Proj"));
        p.push_recent(PathBuf::from(r"e:\proj\"));
        assert_eq!(p.recent.len(), 1);
        assert_eq!(display(&p.recent[0]), r"e:\proj\");
    }
}
