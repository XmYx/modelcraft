//! Plugins: script packages with a manifest, permissions and a version. See `docs/plugins.md`.
//!
//! A plugin is a folder `<plugins folder>/<id>/` holding `plugin.json`, the entry script and
//! whatever Rhai libraries and text files it needs. There is no native or dynamic code: a plugin
//! is Rhai, run by the same sandboxed engine as every script, in a session of its own with
//!
//! - a [`Policy`](crate::Policy) built from the permissions the user granted (changing the scene,
//!   registering menu tools, listening to events),
//! - file access limited to its own folder, plus the project's folder to read and a
//!   `plugin-out/<id>` folder inside it to write when those permissions were granted,
//! - its own folder as the library root for `import`.
//!
//! This module is UI-free: reading and checking manifests, installing a package from a folder,
//! the record of what the user granted, and starting a plugin. The app asks the user (the
//! permission prompt) and keeps the loaded plugins.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use modelcraft_engine::Document;
use serde::{Deserialize, Serialize};

use crate::{Limits, Permissions, Policy, RunContext, RunReport, Session};

/// The plugin API this build offers. A plugin names the version it was written for (`api`); a
/// build refuses plugins that need a newer one and keeps accepting older ones.
pub const API_VERSION: u32 = 1;
/// The oldest `api` this build still runs.
pub const MIN_API_VERSION: u32 = 1;
pub const MANIFEST_FILE: &str = "plugin.json";
pub const GRANTS_FILE: &str = "plugin-grants.json";
/// The folder inside the project's folder a plugin with `write_project_files` writes to.
pub const OUTPUT_FOLDER: &str = "plugin-out";

pub const MAX_MANIFEST_BYTES: u64 = 64 << 10;
pub const MAX_FILES: usize = 200;
pub const MAX_FILE_BYTES: u64 = 1 << 20;
pub const MAX_PACKAGE_BYTES: u64 = 4 << 20;
pub const MAX_DEPTH: usize = 4;
/// Most plugins one scan reads.
pub const MAX_PLUGINS: usize = 256;
/// File extensions a package may contain.
pub const ALLOWED_EXTENSIONS: &[&str] = &["rhai", "json", "md", "txt"];

/// What a plugin may ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Run the commands that change the scene.
    EditScene,
    /// Add commands to the Scripts menu (`register_tool`).
    RegisterTools,
    /// Be told when the selection, scene or frame changes (`on_*` callbacks).
    ListenEvents,
    /// Read files in the project's folder.
    ReadProjectFiles,
    /// Write files in `plugin-out/<id>` inside the project's folder.
    WriteProjectFiles,
}

impl Permission {
    pub const ALL: [Permission; 5] =
        [Permission::EditScene, Permission::RegisterTools, Permission::ListenEvents, Permission::ReadProjectFiles, Permission::WriteProjectFiles];

    pub fn name(self) -> &'static str {
        match self {
            Permission::EditScene => "edit_scene",
            Permission::RegisterTools => "register_tools",
            Permission::ListenEvents => "listen_events",
            Permission::ReadProjectFiles => "read_project_files",
            Permission::WriteProjectFiles => "write_project_files",
        }
    }

    /// What the permission lets the plugin do, in the words of the prompt.
    pub fn describe(self) -> &'static str {
        match self {
            Permission::EditScene => "Change your scene: add, edit and delete objects (every change can be undone)",
            Permission::RegisterTools => "Add commands to the Scripts menu",
            Permission::ListenEvents => "Be told when the selection, the scene or the current frame changes",
            Permission::ReadProjectFiles => "Read files in the folder of your project",
            Permission::WriteProjectFiles => "Write files in a plugin-out folder inside the folder of your project",
        }
    }

    /// Permissions that let a plugin change something outside its own sandbox or your work.
    pub fn is_sensitive(self) -> bool {
        matches!(self, Permission::EditScene | Permission::WriteProjectFiles | Permission::ReadProjectFiles)
    }
}

/// A `major.minor.patch` version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    pub fn parse(s: &str) -> Result<Version, String> {
        let mut it = s.split('.');
        let mut part = |what: &str| -> Result<u32, String> {
            let p = it.next().ok_or_else(|| format!("version {s:?} needs major.minor.patch"))?;
            if p.is_empty() || p.len() > 6 || !p.bytes().all(|b| b.is_ascii_digit()) || (p.len() > 1 && p.starts_with('0')) {
                return Err(format!("version {s:?}: {what} must be a number without leading zeros (up to 6 digits)"));
            }
            p.parse().map_err(|_| format!("version {s:?}: bad {what}"))
        };
        let v = Version(part("major")?, part("minor")?, part("patch")?);
        if it.next().is_some() {
            return Err(format!("version {s:?} has more than three parts"));
        }
        Ok(v)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// `plugin.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Lower-case letters, digits, `.`, `_` and `-`, 3 to 64 characters, starting with a letter or digit.
    pub id: String,
    pub name: String,
    /// `major.minor.patch`.
    pub version: String,
    /// The plugin API version the plugin was written for.
    pub api: u32,
    pub entry: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub permissions: Vec<Permission>,
}

impl Manifest {
    /// Read and check a manifest from JSON text.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let m: Manifest = serde_json::from_str(text).map_err(|e| format!("{MANIFEST_FILE}: {e}"))?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<(), String> {
        valid_id(&self.id)?;
        if self.name.trim().is_empty() || self.name.chars().count() > 60 || self.name.chars().any(char::is_control) {
            return Err("name must be 1 to 60 characters without control characters".into());
        }
        if self.description.chars().count() > 400
            || self.author.chars().count() > 80
            || self.description.chars().chain(self.author.chars()).any(|c| c.is_control() && c != '\n')
        {
            return Err("description (400 characters) and author (80) are too long or have control characters".into());
        }
        Version::parse(&self.version)?;
        if self.api < MIN_API_VERSION {
            return Err(format!("plugin API {} is no longer supported (this build runs {MIN_API_VERSION} to {API_VERSION})", self.api));
        }
        if self.api > API_VERSION {
            return Err(format!("the plugin needs plugin API {}, this build offers {API_VERSION}: update ModelCraft", self.api));
        }
        crate::library::valid_name(self.entry.strip_suffix(".rhai").ok_or("entry must be a .rhai file")?).map_err(|e| format!("entry: {e}"))?;
        let mut seen = self.permissions.clone();
        seen.sort();
        seen.dedup();
        if seen.len() != self.permissions.len() {
            return Err("permissions lists the same permission twice".into());
        }
        Ok(())
    }

    pub fn version(&self) -> Version {
        Version::parse(&self.version).unwrap_or(Version(0, 0, 0))
    }

    pub fn requested(&self) -> Vec<Permission> {
        let mut p = self.permissions.clone();
        p.sort();
        p.dedup();
        p
    }
}

/// Whether `id` is a plugin id.
pub fn valid_id(id: &str) -> Result<(), String> {
    let ok = (3..=64).contains(&id.len())
        && id.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
        && !id.contains("..");
    if ok { Ok(()) } else { Err(format!("id {id:?}: use 3 to 64 lower-case letters, digits, '.', '_' or '-', starting with a letter or digit")) }
}

/// A plugin found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub manifest: Manifest,
    pub dir: PathBuf,
}

/// Every plugin in `plugins_dir` (one folder each, named after its id) and, for those that could
/// not be read, the folder and why. At most [`MAX_PLUGINS`] are read.
pub fn scan(plugins_dir: &Path) -> (Vec<Installed>, Vec<(PathBuf, String)>) {
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(plugins_dir)
        .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir() && !is_hidden(p)).take(MAX_PLUGINS).collect())
        .unwrap_or_default();
    dirs.sort();
    for dir in dirs {
        match read_installed(&dir) {
            Ok(i) => ok.push(i),
            Err(e) => bad.push((dir, e)),
        }
    }
    (ok, bad)
}

fn is_hidden(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

fn read_installed(dir: &Path) -> Result<Installed, String> {
    let file = dir.join(MANIFEST_FILE);
    let meta = std::fs::symlink_metadata(&file).map_err(|_| format!("no {MANIFEST_FILE}"))?;
    if !meta.is_file() || meta.len() > MAX_MANIFEST_BYTES {
        return Err(format!("{MANIFEST_FILE} is not a regular file of at most {MAX_MANIFEST_BYTES} bytes"));
    }
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{MANIFEST_FILE}: {e}"))?;
    let manifest = Manifest::parse(&text)?;
    let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if folder != manifest.id {
        return Err(format!("the folder is called {folder:?} but the plugin's id is {:?}", manifest.id));
    }
    let entry = dir.join(&manifest.entry);
    if !std::fs::symlink_metadata(&entry).is_ok_and(|m| m.is_file()) {
        return Err(format!("the entry script {:?} is missing", manifest.entry));
    }
    Ok(Installed { manifest, dir: dir.to_path_buf() })
}

/// The files of a package folder, checked: regular files only, allowed extensions, bounded count,
/// depth and size, plain names. Returns paths relative to `src`.
pub fn check_package(src: &Path) -> Result<(Manifest, Vec<PathBuf>), String> {
    let installed = read_package_manifest(src)?;
    let mut files = Vec::new();
    let mut total = 0u64;
    walk(src, src, 0, &mut files, &mut total)?;
    if !files.iter().any(|f| f == Path::new(&installed.entry)) {
        return Err(format!("the entry script {:?} is missing", installed.entry));
    }
    Ok((installed, files))
}

fn read_package_manifest(src: &Path) -> Result<Manifest, String> {
    let file = src.join(MANIFEST_FILE);
    let meta = std::fs::symlink_metadata(&file).map_err(|_| format!("{} has no {MANIFEST_FILE}", src.display()))?;
    if !meta.is_file() || meta.len() > MAX_MANIFEST_BYTES {
        return Err(format!("{MANIFEST_FILE} is not a regular file of at most {MAX_MANIFEST_BYTES} bytes"));
    }
    Manifest::parse(&std::fs::read_to_string(&file).map_err(|e| format!("{MANIFEST_FILE}: {e}"))?)
}

fn walk(root: &Path, dir: &Path, depth: usize, files: &mut Vec<PathBuf>, total: &mut u64) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("folders nest more than {MAX_DEPTH} deep"));
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let rel = path.strip_prefix(root).map(Path::to_path_buf).map_err(|_| "a file outside the package".to_string())?;
        if name.starts_with('.') {
            return Err(format!("{}: hidden files are not allowed in a package", rel.display()));
        }
        crate::library::valid_name(name.strip_suffix(".rhai").unwrap_or(&name)).map_err(|e| format!("{}: {e}", rel.display()))?;
        let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", rel.display()))?;
        if meta.file_type().is_symlink() {
            return Err(format!("{}: links are not allowed in a package", rel.display()));
        }
        if meta.is_dir() {
            walk(root, &path, depth + 1, files, total)?;
        } else if meta.is_file() {
            let ext = path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !ALLOWED_EXTENSIONS.contains(&ext.as_str()) {
                return Err(format!("{}: a package holds only {} files", rel.display(), ALLOWED_EXTENSIONS.join(", ")));
            }
            if meta.len() > MAX_FILE_BYTES {
                return Err(format!("{}: larger than {MAX_FILE_BYTES} bytes", rel.display()));
            }
            *total += meta.len();
            if *total > MAX_PACKAGE_BYTES {
                return Err(format!("the package is larger than {MAX_PACKAGE_BYTES} bytes"));
            }
            files.push(rel);
            if files.len() > MAX_FILES {
                return Err(format!("the package has more than {MAX_FILES} files"));
            }
        } else {
            return Err(format!("{}: not a regular file", rel.display()));
        }
    }
    Ok(())
}

/// What installing did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installation {
    /// A new plugin.
    New(Installed),
    /// A newer version of one that was installed (the old version).
    Updated(Installed, Version),
}

impl Installation {
    pub fn installed(&self) -> &Installed {
        match self {
            Installation::New(i) | Installation::Updated(i, _) => i,
        }
    }
}

/// Copy the package in `src` into `plugins_dir/<id>`. The package is checked first (see
/// [`check_package`]); it is copied beside the destination and moved into place, so a failure
/// leaves what was installed untouched. An installed version that is the same or newer is not
/// replaced.
pub fn install(src: &Path, plugins_dir: &Path) -> Result<Installation, String> {
    let (manifest, files) = check_package(src)?;
    let dest = plugins_dir.join(&manifest.id);
    let previous = read_installed(&dest).ok().map(|i| i.manifest.version());
    if let Some(old) = previous {
        if manifest.version() <= old {
            return Err(format!("{} {old} is already installed; this is {}", manifest.name, manifest.version));
        }
    } else if dest.exists() {
        return Err(format!("{} exists and is not a plugin; remove it first", dest.display()));
    }
    std::fs::create_dir_all(plugins_dir).map_err(|e| format!("{}: {e}", plugins_dir.display()))?;
    let staging = plugins_dir.join(format!(".installing-{}", manifest.id));
    let _ = std::fs::remove_dir_all(&staging);
    let copy = || -> Result<(), String> {
        for rel in &files {
            let to = staging.join(rel);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            std::fs::copy(src.join(rel), &to).map_err(|e| format!("{}: {e}", rel.display()))?;
        }
        Ok(())
    };
    if let Err(e) = copy() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    let backup = plugins_dir.join(format!(".replaced-{}", manifest.id));
    let _ = std::fs::remove_dir_all(&backup);
    if previous.is_some() {
        std::fs::rename(&dest, &backup).map_err(|e| format!("could not replace the old version: {e}"))?;
    }
    if let Err(e) = std::fs::rename(&staging, &dest) {
        if previous.is_some() {
            let _ = std::fs::rename(&backup, &dest);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("could not install: {e}"));
    }
    let _ = std::fs::remove_dir_all(&backup);
    let installed = read_installed(&dest)?;
    Ok(match previous {
        Some(old) => Installation::Updated(installed, old),
        None => Installation::New(installed),
    })
}

/// Delete the installed plugin `id` (its folder, nothing else).
pub fn remove(plugins_dir: &Path, id: &str) -> Result<(), String> {
    valid_id(id)?;
    let dir = plugins_dir.join(id);
    if read_installed(&dir).is_err() {
        return Err(format!("{id} is not an installed plugin"));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// What the user decided about one plugin.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// The permissions the user allowed.
    pub permissions: Vec<Permission>,
    /// Switched off by the user (still installed, not run).
    #[serde(default)]
    pub disabled: bool,
}

/// What should happen with an installed plugin at start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Run it with these permissions.
    Ready(Vec<Permission>),
    /// The user has switched it off.
    Disabled,
    /// Ask the user: these permissions were not granted yet (all of them for a new plugin).
    NeedsApproval(Vec<Permission>),
}

/// The user's decisions, kept in `plugin-grants.json` beside the plugins.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grants {
    #[serde(default)]
    plugins: BTreeMap<String, Grant>,
}

impl Grants {
    pub fn load(plugins_dir: &Path) -> Grants {
        let p = plugins_dir.join(GRANTS_FILE);
        match std::fs::metadata(&p) {
            Ok(m) if m.is_file() && m.len() <= MAX_MANIFEST_BYTES * 8 => {}
            _ => return Grants::default(),
        }
        std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Grants>(&b).ok()).map(Grants::sanitized).unwrap_or_default()
    }

    fn sanitized(mut self) -> Grants {
        self.plugins.retain(|id, _| valid_id(id).is_ok());
        for g in self.plugins.values_mut() {
            g.permissions.sort();
            g.permissions.dedup();
        }
        self
    }

    pub fn save(&self, plugins_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(plugins_dir).map_err(|e| format!("{}: {e}", plugins_dir.display()))?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        craft_atomic::atomic_write(&plugins_dir.join(GRANTS_FILE), &bytes).map_err(|e| e.to_string())
    }

    pub fn get(&self, id: &str) -> Option<&Grant> {
        self.plugins.get(id)
    }

    /// What to do with `plugin`: what was asked is compared with what was granted. A plugin that
    /// asks for something new (an update, or a manifest that changed) is asked about again, for
    /// the new permissions only; one that asks for no more than before runs without a prompt.
    pub fn status(&self, plugin: &Manifest) -> Status {
        let requested = plugin.requested();
        let Some(g) = self.plugins.get(&plugin.id) else { return Status::NeedsApproval(requested) };
        if g.disabled {
            return Status::Disabled;
        }
        let new: Vec<Permission> = requested.iter().copied().filter(|p| !g.permissions.contains(p)).collect();
        if new.is_empty() { Status::Ready(requested) } else { Status::NeedsApproval(new) }
    }

    /// The user allowed `permissions` for the plugin (added to what was allowed before).
    pub fn approve(&mut self, id: &str, permissions: &[Permission]) {
        let g = self.plugins.entry(id.to_string()).or_default();
        g.disabled = false;
        g.permissions.extend_from_slice(permissions);
        g.permissions.sort();
        g.permissions.dedup();
    }

    /// The user said no: the plugin stays installed and switched off.
    pub fn deny(&mut self, id: &str) {
        self.plugins.entry(id.to_string()).or_default().disabled = true;
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        self.plugins.entry(id.to_string()).or_default().disabled = !enabled;
    }

    /// Forget everything about `id` (it was removed).
    pub fn forget(&mut self, id: &str) {
        self.plugins.remove(id);
    }
}

/// The policy and file permissions a plugin runs with, from what it was granted.
pub fn sandbox(id: &str, plugin_dir: &Path, project_dir: Option<&Path>, granted: &[Permission]) -> (Policy, Permissions) {
    let has = |p: Permission| granted.contains(&p);
    let policy =
        Policy { edit_scene: has(Permission::EditScene), register_tools: has(Permission::RegisterTools), listen_events: has(Permission::ListenEvents) };
    let mut perms = Permissions::none().with_base(plugin_dir).allow_read(plugin_dir);
    if let Some(project) = project_dir {
        if has(Permission::ReadProjectFiles) {
            perms = perms.allow_read(project);
        }
        if has(Permission::WriteProjectFiles) {
            let out = project.join(OUTPUT_FOLDER).join(id);
            if std::fs::create_dir_all(&out).is_ok() {
                perms = perms.allow_write(&out);
            }
        }
    }
    (policy, perms)
}

/// A plugin that has been started.
pub struct LoadedPlugin {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub granted: Vec<Permission>,
    /// Taken out while its callbacks run.
    pub session: Option<Session>,
}

/// Start `installed` with `granted` permissions: a session of its own with the sandbox, the entry
/// script run once (it registers its tools and callbacks). The report says how the entry script
/// went; a plugin whose entry script failed is returned anyway so the user can see the error.
pub fn start(installed: &Installed, granted: &[Permission], project_dir: Option<&Path>, doc: &mut Document, ctx: &mut RunContext) -> (LoadedPlugin, RunReport) {
    let (policy, perms) = sandbox(&installed.manifest.id, &installed.dir, project_dir, granted);
    let mut session = Session::new(Limits::default(), perms);
    session.set_policy(policy);
    session.set_library(Some(installed.dir.clone()));
    session.set_current_file(None);
    let name = format!("{}/{}", installed.manifest.id, installed.manifest.entry);
    let report = match std::fs::read_to_string(installed.dir.join(&installed.manifest.entry)) {
        Ok(text) if text.len() as u64 <= MAX_FILE_BYTES => session.run(doc, ctx, &name, &text),
        Ok(_) => failure(&name, "the entry script is too large"),
        Err(e) => failure(&name, &e.to_string()),
    };
    let loaded = LoadedPlugin { manifest: installed.manifest.clone(), dir: installed.dir.clone(), granted: granted.to_vec(), session: Some(session) };
    (loaded, report)
}

fn failure(name: &str, message: &str) -> RunReport {
    RunReport {
        error: Some(crate::ScriptError { file: name.into(), line: None, column: None, message: message.into(), kind: crate::ErrorKind::Runtime }),
        ..RunReport::default()
    }
}
