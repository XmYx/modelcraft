//! File access for scripts. A script has no file or network access of its own; the few functions
//! that touch the disk ask a [`Permissions`] first, and the default grants nothing.

use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// Folders a script may read and write. Writing a folder also allows reading it.
///
/// Paths are resolved against `base` (the script's folder), `..` is resolved before the check and
/// symbolic links are followed, so a path cannot leave an allowed folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Permissions {
    read: Vec<PathBuf>,
    write: Vec<PathBuf>,
    base: PathBuf,
}

impl Permissions {
    /// No file access at all.
    pub fn none() -> Self {
        Permissions::default()
    }

    /// Relative paths in scripts start from `base`.
    pub fn with_base(mut self, base: impl Into<PathBuf>) -> Self {
        self.base = base.into();
        self
    }

    pub fn allow_read(mut self, dir: impl AsRef<Path>) -> Self {
        if let Ok(d) = std::fs::canonicalize(dir.as_ref()) {
            self.read.push(d);
        }
        self
    }

    pub fn allow_write(mut self, dir: impl AsRef<Path>) -> Self {
        if let Ok(d) = std::fs::canonicalize(dir.as_ref()) {
            self.write.push(d);
        }
        self
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    pub fn can_read_anything(&self) -> bool {
        !self.read.is_empty() || !self.write.is_empty()
    }

    pub fn can_write_anything(&self) -> bool {
        !self.write.is_empty()
    }

    /// The absolute path a script's `path` means, when it lies inside a folder allowed for
    /// `access`. The error says what to allow.
    pub fn check(&self, path: &str, access: Access) -> Result<PathBuf, String> {
        if path.is_empty() || path.contains('\0') {
            return Err("that is not a file path".into());
        }
        let roots: Vec<&PathBuf> = match access {
            Access::Read => self.read.iter().chain(self.write.iter()).collect(),
            Access::Write => self.write.iter().collect(),
        };
        let what = match access {
            Access::Read => "reading",
            Access::Write => "writing",
        };
        if roots.is_empty() {
            return Err(format!(
                "this script may not use {what} files here (scripts have no file access unless it is allowed: --allow-{} on the command line)",
                if access == Access::Read { "read" } else { "write" }
            ));
        }
        let joined = if Path::new(path).is_absolute() { PathBuf::from(path) } else { self.base.join(path) };
        let full = resolve(&joined).ok_or_else(|| format!("{path}: cannot be resolved"))?;
        if roots.iter().any(|r| full.starts_with(r)) { Ok(full) } else { Err(format!("{path}: outside the folders this script may use for {what} files")) }
    }
}

/// Make `p` absolute and free of `.`, `..` and symbolic links, also when its tail does not exist yet.
fn resolve(p: &Path) -> Option<PathBuf> {
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    let mut head = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Prefix(x) => head.push(x.as_os_str()),
            Component::RootDir => head.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(n) => parts.push(n.to_os_string()),
        }
    }
    let mut full = head;
    full.extend(&parts);
    // Canonicalize the longest prefix that exists (this resolves links), then add the rest.
    let mut existing = full.clone();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match std::fs::canonicalize(&existing) {
            Ok(c) => {
                let mut out = c;
                out.extend(rest.iter().rev());
                return Some(out);
            }
            Err(_) => {
                let name = existing.file_name()?.to_os_string();
                rest.push(name);
                if !existing.pop() {
                    return None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mc-script-perm-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn nothing_is_allowed_by_default() {
        let p = Permissions::none();
        assert!(p.check("a.txt", Access::Read).is_err());
        assert!(p.check("/etc/passwd", Access::Read).is_err());
        assert!(!p.can_read_anything() && !p.can_write_anything());
    }

    #[test]
    fn paths_stay_inside_the_allowed_folders() {
        let d = tmp("inside");
        let out = tmp("outside");
        let p = Permissions::none().with_base(&d).allow_read(&d);
        assert!(p.check("a.txt", Access::Read).is_ok());
        assert!(p.check("sub/dir/new.txt", Access::Read).is_ok(), "a path that does not exist yet");
        assert!(p.check("../x", Access::Read).is_err());
        assert!(p.check("sub/../../x", Access::Read).is_err());
        assert!(p.check(out.to_str().unwrap_or("/"), Access::Read).is_err());
        assert!(p.check("a.txt", Access::Write).is_err(), "read access is not write access");
        assert!(p.check("", Access::Read).is_err());
        assert!(p.check("a\0b", Access::Read).is_err());
        // Write access allows reading too.
        let w = Permissions::none().with_base(&d).allow_write(&d);
        assert!(w.check("a.txt", Access::Write).is_ok() && w.check("a.txt", Access::Read).is_ok());
        // An absolute path inside works; one with `..` back out of it does not.
        assert!(p.check(d.join("f").to_str().unwrap_or(""), Access::Read).is_ok());
        assert!(p.check(&format!("{}/../{}", d.display(), "z"), Access::Read).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_cannot_lead_out() {
        let d = tmp("link-in");
        let out = tmp("link-out");
        let _ = std::fs::write(out.join("secret.txt"), "x");
        let _ = std::os::unix::fs::symlink(&out, d.join("door"));
        let p = Permissions::none().with_base(&d).allow_read(&d);
        assert!(p.check("door/secret.txt", Access::Read).is_err());
        assert!(p.check("door", Access::Read).is_err());
    }
}
