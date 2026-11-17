//! User preferences through craft-settings (`config.toml` in the platform config folder, atomic
//! saves, debounced): navigation preset, recent files, snapping, brush size and shortcut overrides.

use serde::{Deserialize, Serialize};

/// Most recent files kept.
pub const MAX_RECENT: usize = 12;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// "alt", "middle" or "right".
    pub navigation: String,
    pub recent: Vec<String>,
    pub snap: bool,
    pub snap_move: f32,
    pub snap_rotate_deg: f32,
    pub snap_scale: f32,
    pub brush_px: f32,
    /// Panel layouts per workspace (`model`, `sculpt`, `paint`), as JSON.
    pub layouts: std::collections::BTreeMap<String, String>,
    #[serde(with = "craft_keymap::keymap::overrides_list")]
    pub shortcuts: craft_keymap::Overrides,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            navigation: "alt".into(),
            recent: Vec::new(),
            snap: false,
            snap_move: 0.25,
            snap_rotate_deg: 15.0,
            snap_scale: 0.1,
            brush_px: 60.0,
            layouts: Default::default(),
            shortcuts: Default::default(),
        }
    }
}

impl craft_settings::Settings for Prefs {
    const APP_ID: &'static str = "modelcraft";
    const SCHEMA_VERSION: u32 = 1;
}

impl Prefs {
    /// Put a file at the top of the recent list.
    pub fn touch_recent(&mut self, path: &std::path::Path) {
        let p = path.display().to_string();
        self.recent.retain(|r| r != &p);
        self.recent.insert(0, p);
        self.recent.truncate(MAX_RECENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_list_is_unique_and_bounded() {
        let mut p = Prefs::default();
        for i in 0..20 {
            p.touch_recent(std::path::Path::new(&format!("/x/{i}.modelcraft")));
        }
        p.touch_recent(std::path::Path::new("/x/5.modelcraft"));
        assert_eq!(p.recent.len(), MAX_RECENT);
        assert_eq!(p.recent[0], "/x/5.modelcraft");
        assert_eq!(p.recent.iter().filter(|r| r.ends_with("/5.modelcraft")).count(), 1);
    }
}
