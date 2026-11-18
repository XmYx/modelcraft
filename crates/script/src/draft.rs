//! The script editor's unsaved text, kept on disk so that quitting (or crashing) does not lose it.
//!
//! The app writes a [`Draft`] a moment after the text last changed and deletes it when the text is
//! saved or emptied; at the next start it offers what is left. One JSON file, replaced atomically,
//! so a crash during a write leaves the previous draft whole. The folder is the app's recovery
//! folder. Nothing here panics on a missing, unreadable or hostile file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Largest text kept (the editor itself refuses files over 4 MiB).
pub const MAX_DRAFT_BYTES: usize = 4 << 20;

/// The file the draft lives in, inside the recovery folder.
pub const FILE: &str = "script-draft.json";

/// An unsaved editor text and the file it belongs to, if it has one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    /// The script file being edited (what Save would write), when it has one.
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    #[serde(flatten)]
    draft: Draft,
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// Write the draft. Texts over [`MAX_DRAFT_BYTES`] are refused (the caller keeps the old draft).
pub fn save(dir: &Path, draft: &Draft) -> Result<(), String> {
    if draft.text.len() > MAX_DRAFT_BYTES {
        return Err(format!("the script is larger than {} MiB", MAX_DRAFT_BYTES >> 20));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let bytes = serde_json::to_vec(&Stored { version: 1, draft: draft.clone() }).map_err(|e| e.to_string())?;
    craft_atomic::atomic_write(&path_in(dir), &bytes).map_err(|e| e.to_string())
}

/// The draft left in `dir`, if there is a readable one (empty drafts do not count).
pub fn load(dir: &Path) -> Option<Draft> {
    let p = path_in(dir);
    let meta = std::fs::metadata(&p).ok()?;
    if !meta.is_file() || meta.len() > (MAX_DRAFT_BYTES as u64) * 2 {
        return None;
    }
    let bytes = std::fs::read(&p).ok()?;
    let s: Stored = serde_json::from_slice(&bytes).ok()?;
    (s.version == 1 && !s.draft.text.is_empty() && s.draft.text.len() <= MAX_DRAFT_BYTES).then_some(s.draft)
}

/// Remove the draft (the text was saved or discarded).
pub fn clear(dir: &Path) {
    let _ = std::fs::remove_file(path_in(dir));
}
