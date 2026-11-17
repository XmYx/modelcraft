//! The crash journal: one JSON object per line, `{"seq":n,"at":"<RFC 3339>","cmd":…}`, appended
//! and flushed after every command. A torn last line (the crash happened mid-write) is ignored
//! on reading.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{FormatError, Result};

/// Longest journal line read back (a long sculpt stroke is a few MiB).
pub const MAX_LINE: usize = 64 << 20;
/// Most entries replayed.
pub const MAX_ENTRIES: usize = 1_000_000;

#[derive(Serialize, Deserialize)]
struct Line<T> {
    seq: u64,
    at: String,
    cmd: T,
}

/// Path of the journal for a project file (`x.modelcraft` → `x.modelcraft.journal`).
pub fn path_for(project: &Path) -> PathBuf {
    let mut s = project.as_os_str().to_owned();
    s.push(".journal");
    PathBuf::from(s)
}

pub struct Journal {
    file: File,
    path: PathBuf,
    seq: u64,
}

impl Journal {
    /// Start a fresh journal (truncating any old one).
    pub fn create(path: &Path) -> Result<Journal> {
        let file = File::create(path).map_err(|e| FormatError::io(path, e))?;
        Ok(Journal { file, path: path.to_path_buf(), seq: 0 })
    }

    /// Continue an existing journal (after a recovery replay).
    pub fn append(path: &Path, seq: u64) -> Result<Journal> {
        let file = OpenOptions::new().create(true).append(true).open(path).map_err(|e| FormatError::io(path, e))?;
        Ok(Journal { file, path: path.to_path_buf(), seq })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one command and flush it to the operating system.
    pub fn write<T: Serialize>(&mut self, cmd: &T) -> Result<()> {
        self.seq += 1;
        let line = Line { seq: self.seq, at: chrono::Utc::now().to_rfc3339(), cmd };
        let mut s = serde_json::to_vec(&line).map_err(|e| FormatError::Malformed(e.to_string()))?;
        s.push(b'\n');
        self.file.write_all(&s).map_err(|e| FormatError::io(&self.path, e))?;
        self.file.flush().map_err(|e| FormatError::io(&self.path, e))
    }

    /// Force the journal to disk (called on idle and before risky work).
    pub fn sync(&mut self) -> Result<()> {
        self.file.sync_data().map_err(|e| FormatError::io(&self.path, e))
    }
}

/// Read every complete entry of a journal. Unreadable lines stop the replay there (everything
/// after a torn write is suspect).
pub fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>> {
    let bytes = std::fs::read(path).map_err(|e| FormatError::io(path, e))?;
    Ok(parse(&bytes))
}

pub fn parse<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Vec<T> {
    let mut out = Vec::new();
    for line in bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_LINE || out.len() >= MAX_ENTRIES {
            break;
        }
        match serde_json::from_slice::<Line<T>>(line) {
            Ok(l) => out.push(l.cmd),
            Err(_) => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torn_tail_is_dropped() {
        let dir = std::env::temp_dir().join(format!("mc-journal-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("a.modelcraft.journal");
        let mut j = Journal::create(&p).unwrap();
        j.write(&"one").unwrap();
        j.write(&"two").unwrap();
        drop(j);
        let mut f = OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"{\"seq\":3,\"at\":\"x\",\"cmd\":\"thr").unwrap();
        let got: Vec<String> = read(&p).unwrap();
        assert_eq!(got, vec!["one", "two"]);
        assert_eq!(path_for(Path::new("/x/a.modelcraft")), PathBuf::from("/x/a.modelcraft.journal"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
