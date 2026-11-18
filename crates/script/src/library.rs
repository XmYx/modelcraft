//! Script libraries: `import "lib/shapes" as shapes;` loads `<scripts folder>/lib/shapes.rhai` as a
//! Rhai module whose functions and constants a script then calls as `shapes::ring(...)`.
//!
//! The resolver is sandboxed. A module name is a relative path of plain names (letters, digits,
//! `_`, `-`, space and inner dots), never starting at a root and never containing `..`, a hidden
//! name or a backslash; the file must sit inside the library folder once symbolic links are
//! followed; it must be a regular file of at most [`Limits::max_file_bytes`]. A library runs in
//! the same engine as the script that imports it, so it has the same operation, time, depth and
//! command limits and the same file permissions, and cannot reach anything the script could not.
//! Libraries may import libraries up to [`MAX_IMPORT_DEPTH`] deep; an import cycle is an error;
//! at most [`MAX_MODULES`] modules load in one run. With no library folder every import fails.
//!
//! Callbacks and tools that a script registers run after the script has ended, when its own
//! imports are gone. So the modules a script imports with `import "name" as alias;` at the top
//! level are also kept by the session as the namespace `alias` for as long as the session lives
//! (running a script again replaces what its aliases stood for; two scripts that use the same
//! alias for different libraries share the last one loaded, so give aliases that name the library).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rhai::{Engine, EvalAltResult, Module, ModuleResolver, Position, Scope, Shared};

/// Deepest chain of libraries importing libraries.
pub const MAX_IMPORT_DEPTH: usize = 8;
/// Most modules one run loads.
pub const MAX_MODULES: usize = 64;
/// Longest module name.
pub const MAX_NAME_BYTES: usize = 120;

/// The folder libraries are read from and what is loaded from it during a run.
pub(crate) struct Library {
    root: RefCell<Option<PathBuf>>,
    max_bytes: u64,
    /// Canonical file paths being loaded (outermost first), for cycle detection and depth.
    loading: RefCell<Vec<PathBuf>>,
    cache: RefCell<HashMap<PathBuf, Shared<Module>>>,
    loaded: Cell<usize>,
}

impl Library {
    pub fn new(max_bytes: u64) -> Self {
        Library { root: RefCell::new(None), max_bytes, loading: RefCell::new(Vec::new()), cache: RefCell::new(HashMap::new()), loaded: Cell::new(0) }
    }

    pub fn set_root(&self, root: Option<PathBuf>) {
        if let Ok(mut r) = self.root.try_borrow_mut() {
            *r = root.and_then(|p| std::fs::canonicalize(p).ok());
        }
        self.reset();
    }

    pub fn root(&self) -> Option<PathBuf> {
        self.root.try_borrow().ok().and_then(|r| r.clone())
    }

    /// Forget what was loaded (a new run reads the files again, so edits show).
    pub fn reset(&self) {
        if let Ok(mut c) = self.cache.try_borrow_mut() {
            c.clear();
        }
        if let Ok(mut l) = self.loading.try_borrow_mut() {
            l.clear();
        }
        self.loaded.set(0);
    }
}

/// Whether `name` is an acceptable module name (see the module docs).
pub fn valid_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the module name is empty".into());
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(format!("the module name is longer than {MAX_NAME_BYTES} bytes"));
    }
    if name.starts_with('/') || name.contains('\\') || name.contains(':') {
        return Err("a module name is a relative path of plain names, like \"lib/shapes\"".into());
    }
    for part in name.split('/') {
        if part.is_empty() {
            return Err("a module name has an empty part".into());
        }
        if part.starts_with('.') || part.ends_with('.') {
            return Err(format!("{part:?} is not a plain name (no leading or trailing dots, no ..)"));
        }
        if !part.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ' ' | '.')) {
            return Err(format!("{part:?} has characters a module name may not have (letters, digits, _ - . and space)"));
        }
    }
    Ok(())
}

/// The file a module name stands for under `root`, checked to be inside it.
pub fn locate(root: &Path, name: &str) -> Result<PathBuf, String> {
    valid_name(name)?;
    let base = name.strip_suffix(".rhai").unwrap_or(name);
    let file = root.join(format!("{base}.rhai"));
    let canon = std::fs::canonicalize(&file).map_err(|_| format!("there is no library {name:?} in the scripts folder"))?;
    if !canon.starts_with(root) {
        return Err(format!("{name:?} is outside the scripts folder"));
    }
    Ok(canon)
}

/// The `(module name, alias)` pairs of the `import "name" as alias;` statements in `source`, found
/// with the same tokenizer the editor highlights with (so comments and strings do not count).
pub fn imports(source: &str) -> Vec<(String, String)> {
    use crate::highlight::{TokenKind, tokenize};
    let toks: Vec<_> = tokenize(source).into_iter().filter(|t| !matches!(t.kind, TokenKind::Space | TokenKind::Comment)).collect();
    let text = |t: &crate::highlight::Token| source.get(t.start..t.end).unwrap_or("");
    let mut out = Vec::new();
    for w in toks.windows(4) {
        if w[0].kind == TokenKind::Keyword && text(&w[0]) == "import" && w[1].kind == TokenKind::Text && w[2].kind == TokenKind::Keyword && text(&w[2]) == "as" && w[3].kind == TokenKind::Ident {
            let lit = text(&w[1]);
            if let Some(name) = lit.strip_prefix('"').and_then(|l| l.strip_suffix('"')) {
                if !name.contains('\\') && out.len() < MAX_MODULES {
                    out.push((name.to_string(), text(&w[3]).to_string()));
                }
            }
        }
    }
    out
}

impl Library {
    /// Load `name` (cached for the run), for the session to keep under an alias.
    pub(crate) fn load(self: &std::rc::Rc<Self>, engine: &Engine, name: &str) -> Option<Shared<Module>> {
        Resolver(self.clone()).resolve(engine, None, name, Position::NONE).ok()
    }
}

pub(crate) struct Resolver(pub std::rc::Rc<Library>);

fn fail(msg: impl Into<String>, name: &str, pos: Position) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorInModule(name.to_string(), Box::new(EvalAltResult::ErrorRuntime(msg.into().into(), pos)), pos))
}

impl ModuleResolver for Resolver {
    fn resolve(&self, engine: &Engine, _source: Option<&str>, path: &str, pos: Position) -> Result<Shared<Module>, Box<EvalAltResult>> {
        let lib = &self.0;
        let Some(root) = lib.root() else {
            return Err(fail("imports need a scripts folder, and this run has none", path, pos));
        };
        let file = locate(&root, path).map_err(|e| fail(e, path, pos))?;
        if let Some(m) = lib.cache.try_borrow().ok().and_then(|c| c.get(&file).cloned()) {
            return Ok(m);
        }
        {
            let loading = lib.loading.try_borrow().map_err(|_| fail("the library loader is busy", path, pos))?;
            if let Some(i) = loading.iter().position(|p| *p == file) {
                let chain: Vec<String> = loading[i..].iter().chain(std::iter::once(&file)).filter_map(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).collect();
                return Err(fail(format!("libraries import each other in a cycle: {}", chain.join(" -> ")), path, pos));
            }
            if loading.len() >= MAX_IMPORT_DEPTH {
                return Err(fail(format!("libraries import libraries more than {MAX_IMPORT_DEPTH} deep"), path, pos));
            }
        }
        let n = lib.loaded.get() + 1;
        if n > MAX_MODULES {
            return Err(fail(format!("a run may load at most {MAX_MODULES} library modules"), path, pos));
        }
        lib.loaded.set(n);
        let meta = std::fs::metadata(&file).map_err(|e| fail(e.to_string(), path, pos))?;
        if !meta.is_file() || meta.len() > lib.max_bytes {
            return Err(fail(format!("{path:?} is not a regular file of at most {} bytes", lib.max_bytes), path, pos));
        }
        let bytes = std::fs::read(&file).map_err(|e| fail(e.to_string(), path, pos))?;
        let text = String::from_utf8(bytes).map_err(|_| fail("the library is not UTF-8 text", path, pos))?;
        let mut ast = engine.compile(&text).map_err(|e| Box::new(EvalAltResult::ErrorInModule(path.to_string(), Box::new(e.into()), pos)))?;
        ast.set_source(format!("{path}.rhai"));
        if let Ok(mut l) = lib.loading.try_borrow_mut() {
            l.push(file.clone());
        }
        let module = Module::eval_ast_as_new(Scope::new(), &ast, engine);
        if let Ok(mut l) = lib.loading.try_borrow_mut() {
            l.pop();
        }
        let module: Shared<Module> = module.map_err(|e| Box::new(EvalAltResult::ErrorInModule(path.to_string(), e, pos)))?.into();
        if let Ok(mut c) = lib.cache.try_borrow_mut() {
            c.insert(file, module.clone());
        }
        Ok(module)
    }
}
