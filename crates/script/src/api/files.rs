//! The only functions that touch the disk, each checked against the run's permissions.

use std::rc::Rc;

use rhai::{Array, Dynamic, Engine};

use crate::error::{R, rt};
use crate::host::Shared;
use crate::perm::Access;

pub(crate) fn register(engine: &mut Engine, s: &Rc<Shared>) {
    let sh = s.clone();
    engine.register_fn("read_text", move |path: &str| -> R<String> {
        let p = sh.path(path, Access::Read)?;
        let meta = std::fs::metadata(&p).map_err(|e| rt(format!("{path}: {e}")))?;
        if meta.len() > sh.limits.max_file_bytes {
            return Err(rt(format!("{path} is larger than the {} bytes a script may read", sh.limits.max_file_bytes)));
        }
        let bytes = std::fs::read(&p).map_err(|e| rt(format!("{path}: {e}")))?;
        String::from_utf8(bytes).map_err(|_| rt(format!("{path} is not UTF-8 text")))
    });
    let sh = s.clone();
    engine.register_fn("write_text", move |path: &str, text: &str| -> R<()> {
        let p = sh.path(path, Access::Write)?;
        if text.len() as u64 > sh.limits.max_file_bytes {
            return Err(rt(format!("a script may write at most {} bytes to a file", sh.limits.max_file_bytes)));
        }
        craft_atomic::atomic_write(&p, text.as_bytes()).map_err(|e| rt(format!("{path}: {e}")))
    });
    let sh = s.clone();
    engine.register_fn("file_exists", move |path: &str| -> R<bool> { Ok(sh.path(path, Access::Read)?.exists()) });
    let sh = s.clone();
    engine.register_fn("list_dir", move |path: &str| -> R<Array> {
        let p = sh.path(path, Access::Read)?;
        let rd = std::fs::read_dir(&p).map_err(|e| rt(format!("{path}: {e}")))?;
        let mut names: Vec<String> = rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).take(sh.limits.max_array_len).collect();
        names.sort();
        Ok(names.into_iter().map(Dynamic::from).collect())
    });
    let sh = s.clone();
    engine.register_fn("export_scene", move |path: &str| -> R<()> {
        sh.check_time()?;
        let p = sh.path(path, Access::Write)?;
        sh.with_doc(|d| d.export(&p))?.map_err(|e| rt(e.to_string()))
    });
    let sh = s.clone();
    engine.register_fn("script_dir", move || -> R<String> { Ok(sh.perms.try_borrow().map(|p| p.base().to_string_lossy().into_owned()).unwrap_or_default()) });
    let sh = s.clone();
    engine.register_fn("may_write_files", move || -> R<bool> { Ok(sh.perms.try_borrow().map(|p| p.can_write_anything()).unwrap_or(false)) });
}
