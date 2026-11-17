//! A script session: one configured engine, run against a document one script at a time.

use std::cell::Cell;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::time::{Duration, Instant};

use modelcraft_engine::Document;
use modelcraft_engine::scene::ObjectId;
use rhai::packages::{Package, StandardPackage};
use rhai::{Dynamic, Engine, Scope};

use crate::api;
use crate::error::{ErrorKind, ScriptError, from_eval, from_parse};
use crate::host::{LineKind, OutLine, RunContext, Shared};
use crate::limits::Limits;
use crate::perm::Permissions;

/// What a run did.
#[derive(Debug, Default)]
pub struct RunReport {
    /// Why it stopped early. Commands that ran before the error stay applied (an `undo_group`
    /// that was open rolled back).
    pub error: Option<ScriptError>,
    /// The value of the script's last expression, as text (none for `()`).
    pub value: Option<String>,
    /// Console lines: `print`, `debug` and command warnings.
    pub lines: Vec<OutLine>,
    /// The selection after the run, when the script changed it.
    pub selection: Option<Vec<ObjectId>>,
    pub commands: u64,
    pub operations: u64,
    pub elapsed: Duration,
}

impl RunReport {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }
}

pub struct Session {
    engine: Engine,
    shared: Rc<Shared>,
}

/// Puts the document back even if the run unwinds.
struct Loan<'a> {
    shared: &'a Shared,
    doc: &'a mut modelcraft_engine::Document,
    ctx: &'a mut RunContext,
}

impl<'a> Loan<'a> {
    fn new(shared: &'a Shared, doc: &'a mut Document, ctx: &'a mut RunContext) -> Option<Self> {
        let mut d = shared.doc.try_borrow_mut().ok()?;
        let mut c = shared.ctx.try_borrow_mut().ok()?;
        std::mem::swap(&mut *d, doc);
        std::mem::swap(&mut *c, ctx);
        Some(Loan { shared, doc, ctx })
    }
}

impl Drop for Loan<'_> {
    fn drop(&mut self) {
        if let Ok(mut d) = self.shared.doc.try_borrow_mut() {
            while d.group_depth() > 0 {
                d.end_group(true);
            }
            std::mem::swap(&mut *d, self.doc);
        }
        if let Ok(mut c) = self.shared.ctx.try_borrow_mut() {
            std::mem::swap(&mut *c, self.ctx);
        }
    }
}

impl Session {
    pub fn new(limits: Limits, perms: Permissions) -> Self {
        let shared = Rc::new(Shared::new(limits.clone(), perms));
        let mut engine = Engine::new_raw();
        engine.register_global_module(StandardPackage::new().as_shared_module());
        engine.set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new());
        engine.set_max_operations(limits.max_operations);
        engine.set_max_call_levels(limits.max_call_depth);
        engine.set_max_expr_depths(limits.max_expr_depth, limits.max_expr_depth);
        engine.set_max_string_size(limits.max_string_bytes);
        engine.set_max_array_size(limits.max_array_len);
        engine.set_max_map_size(limits.max_map_len);
        engine.set_max_modules(0);
        engine.set_strict_variables(true);
        engine.disable_symbol("eval");
        let sh = shared.clone();
        engine.on_print(move |t| sh.emit(LineKind::Print, t, None));
        let sh = shared.clone();
        engine.on_debug(move |t, _src, pos| sh.emit(LineKind::Debug, t, pos.line()));
        let sh = shared.clone();
        let tick = Cell::new(0u64);
        engine.on_progress(move |ops| {
            sh.operations.set(ops);
            let n = tick.get() + 1;
            tick.set(n);
            if n.is_multiple_of(256) && sh.check_time().is_err() {
                return Some(Dynamic::UNIT);
            }
            None
        });
        api::register_all(&mut engine, &shared);
        Session { engine, shared }
    }

    pub fn limits(&self) -> &Limits {
        &self.shared.limits
    }

    /// Change what files the next runs may touch.
    pub fn set_permissions(&self, perms: Permissions) {
        if let Ok(mut p) = self.shared.perms.try_borrow_mut() {
            *p = perms;
        }
    }

    pub(crate) fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Parse `source` without running it.
    pub fn check(&self, name: &str, source: &str) -> Result<(), ScriptError> {
        self.engine.compile(source).map(|_| ()).map_err(|e| from_parse(name, &e))
    }

    /// Run `source` against `doc`. `name` is what error messages call the script (a file name).
    /// Never panics and never runs past the limits; see [`RunReport`].
    pub fn run(&mut self, doc: &mut Document, ctx: &mut RunContext, name: &str, source: &str) -> RunReport {
        let s = &self.shared;
        if let Ok(mut o) = s.out.try_borrow_mut() {
            *o = Default::default();
        }
        s.deadline.set(Some(Instant::now() + s.limits.max_time));
        s.native_left.set(s.limits.max_native_items);
        s.commands_run.set(0);
        s.operations.set(0);
        s.selection_changed.set(false);
        let started = Instant::now();
        let mut report = RunReport::default();

        let Some(loan) = Loan::new(s, doc, ctx) else {
            report.error =
                Some(ScriptError { file: name.into(), line: None, column: None, message: "a script is already running".into(), kind: ErrorKind::Runtime });
            return report;
        };
        let limit_secs = s.limits.max_time.as_secs_f64();
        let result = match self.engine.compile(source) {
            Err(e) => Err(from_parse(name, &e)),
            Ok(mut ast) => {
                ast.set_source(name);
                let mut scope = Scope::new();
                let r = std::panic::catch_unwind(AssertUnwindSafe(|| self.engine.eval_ast_with_scope::<Dynamic>(&mut scope, &ast)));
                match r {
                    Ok(Ok(v)) => Ok(v),
                    Ok(Err(e)) => Err(from_eval(name, &e, limit_secs)),
                    Err(_) => Err(ScriptError {
                        file: name.into(),
                        line: None,
                        column: None,
                        message: "internal error while running the script".into(),
                        kind: ErrorKind::Runtime,
                    }),
                }
            }
        };
        let changed = s.selection_changed.get();
        // The run's selection lives in the shared state until the loan ends.
        let selection = if changed { s.ctx.try_borrow().ok().map(|c| c.selection.clone()) } else { None };
        // `loan` puts the document back here; groups a failed script left open roll back.
        drop(loan);

        match result {
            Ok(v) => {
                if !v.is_unit() {
                    report.value = Some(v.to_string().chars().take(4096).collect());
                }
            }
            Err(e) => report.error = Some(e),
        }
        report.selection = selection;
        report.lines = s.out.try_borrow_mut().map(|mut o| std::mem::take(&mut o.lines)).unwrap_or_default();
        report.commands = s.commands_run.get();
        report.operations = s.operations.get();
        report.elapsed = started.elapsed();
        report
    }
}
