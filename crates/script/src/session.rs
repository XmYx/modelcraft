//! A script session: one configured engine, run against a document one script at a time.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::time::{Duration, Instant};

use modelcraft_engine::Document;
use modelcraft_engine::scene::ObjectId;
use rhai::packages::{Package, StandardPackage};
use rhai::{AST, Dynamic, Engine, Scope};

use crate::api::events::{Event, EventKind, MAX_FAILURES, ToolInfo};
use crate::api::{self, events};
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

    /// Arguments the next runs read with `args()` and `arg(name, default)`.
    pub fn set_args(&self, args: BTreeMap<String, String>) {
        if let Ok(mut a) = self.shared.args.try_borrow_mut() {
            *a = args;
        }
    }

    /// The document's file name, which scripts read with `current_file()`.
    pub fn set_current_file(&self, file: Option<String>) {
        if let Ok(mut f) = self.shared.file.try_borrow_mut() {
            *f = file;
        }
    }

    /// Run `source` against `doc`. `name` is what error messages call the script (a file name).
    /// Never panics and never runs past the limits; see [`RunReport`]. Tools and callbacks the
    /// script registered earlier under the same name are replaced by the ones it registers now.
    pub fn run(&mut self, doc: &mut Document, ctx: &mut RunContext, name: &str, source: &str) -> RunReport {
        let limit = self.shared.limits.max_time;
        self.drive(doc, ctx, name, limit, |engine, shared| {
            let mut ast = engine.compile(source).map_err(Failure::Parse)?;
            ast.set_source(name);
            if let Ok(mut r) = shared.registry.try_borrow_mut() {
                r.forget(name);
                r.owner = name.to_string();
                r.ast = Some(Rc::new(ast.clone_functions_only()));
            }
            let mut scope = Scope::new();
            engine.eval_ast_with_scope::<Dynamic>(&mut scope, &ast).map_err(Failure::Eval)
        })
    }

    /// The tools scripts registered, in registration order.
    pub fn tools(&self) -> Vec<ToolInfo> {
        self.shared.registry.try_borrow().map(|r| r.tools.iter().map(|t| t.info.clone()).collect()).unwrap_or_default()
    }

    /// How many callbacks are registered for `kind`.
    pub fn callback_count(&self, kind: EventKind) -> usize {
        self.shared.registry.try_borrow().map(|r| r.handlers.iter().filter(|h| h.kind == kind).count()).unwrap_or(0)
    }

    /// Whether any callback listens at all (the app skips its change detection when none does).
    pub fn has_callbacks(&self) -> bool {
        self.shared.registry.try_borrow().is_ok_and(|r| !r.handlers.is_empty())
    }

    /// Run the tool registered under `label` (a menu command a script added).
    pub fn run_tool(&mut self, doc: &mut Document, ctx: &mut RunContext, label: &str) -> RunReport {
        let found = self
            .shared
            .registry
            .try_borrow()
            .ok()
            .and_then(|r| r.tools.iter().find(|t| t.info.label == label).map(|t| (t.info.owner.clone(), t.f.clone(), t.ast.clone())));
        let Some((owner, f, ast)) = found else {
            let error =
                ScriptError { file: label.into(), line: None, column: None, message: format!("there is no tool called {label:?}"), kind: ErrorKind::Runtime };
            return RunReport { error: Some(error), ..RunReport::default() };
        };
        let limit = self.shared.limits.max_time;
        self.drive(doc, ctx, &owner, limit, |engine, _| f.call::<Dynamic>(engine, &ast, ()).map_err(Failure::Eval))
    }

    /// Run the callbacks registered for an event, one report each. They run with the shorter
    /// event time limit; a callback that fails [`MAX_FAILURES`] times in a row is switched off
    /// (its last report says so).
    pub fn fire(&mut self, doc: &mut Document, ctx: &mut RunContext, event: &Event) -> Vec<RunReport> {
        let kind = event.kind();
        let calls: Vec<(usize, String, rhai::FnPtr, Rc<AST>)> = match self.shared.registry.try_borrow() {
            Ok(r) => r.handlers.iter().enumerate().filter(|(_, h)| h.kind == kind).map(|(i, h)| (i, h.owner.clone(), h.f.clone(), h.ast.clone())).collect(),
            Err(_) => return Vec::new(),
        };
        let limit = self.shared.limits.max_event_time.min(self.shared.limits.max_time);
        let mut reports = Vec::new();
        let mut off: Vec<usize> = Vec::new();
        for (i, owner, f, ast) in calls {
            let selection = ctx.selection.clone();
            let arg = Dynamic::from_map(events::event_map(event, &selection));
            let mut report = self.drive(doc, ctx, &owner, limit, |engine, _| f.call::<Dynamic>(engine, &ast, (arg,)).map_err(Failure::Eval));
            if let Ok(mut r) = self.shared.registry.try_borrow_mut()
                && let Some(h) = r.handlers.get_mut(i)
            {
                if report.error.is_some() {
                    h.failures += 1;
                    if h.failures >= MAX_FAILURES {
                        off.push(i);
                        report.lines.push(OutLine {
                            kind: LineKind::Warning,
                            text: format!("the {} callback of {owner} failed {MAX_FAILURES} times in a row and was switched off", kind.name()),
                            line: None,
                        });
                    }
                } else {
                    h.failures = 0;
                }
            }
            reports.push(report);
        }
        if !off.is_empty()
            && let Ok(mut r) = self.shared.registry.try_borrow_mut()
        {
            let mut i = 0;
            r.handlers.retain(|_| {
                let keep = !off.contains(&i);
                i += 1;
                keep
            });
        }
        reports
    }

    /// The shared part of every run: limits, the document on loan, panics caught, the report.
    fn drive(
        &mut self,
        doc: &mut Document,
        ctx: &mut RunContext,
        name: &str,
        time_limit: Duration,
        body: impl FnOnce(&Engine, &Shared) -> Result<Dynamic, Failure>,
    ) -> RunReport {
        let s = &self.shared;
        if let Ok(mut o) = s.out.try_borrow_mut() {
            *o = Default::default();
        }
        s.deadline.set(Some(Instant::now() + time_limit));
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
        let limit_secs = time_limit.as_secs_f64();
        let r = std::panic::catch_unwind(AssertUnwindSafe(|| body(&self.engine, s)));
        let result = match r {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(Failure::Parse(e))) => Err(from_parse(name, &e)),
            Ok(Err(Failure::Eval(e))) => Err(from_eval(name, &e, limit_secs)),
            Err(_) => Err(ScriptError {
                file: name.into(),
                line: None,
                column: None,
                message: "internal error while running the script".into(),
                kind: ErrorKind::Runtime,
            }),
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

/// Why a run's body did not give a value.
enum Failure {
    Parse(rhai::ParseError),
    Eval(Box<rhai::EvalAltResult>),
}
