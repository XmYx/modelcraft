//! Scripted tools and event callbacks.
//!
//! A script can register a tool (`register_tool("Label", || { ... })`), which the app lists in its
//! Scripts menu, and callbacks for events: `on_selection_changed`, `on_scene_changed`,
//! `on_scene_loaded` and `on_frame_changed`. They are kept by the [`Session`](crate::Session);
//! running a script again replaces what that script registered before.
//!
//! Callbacks run with their own, shorter limits, one at a time, never inside another run. A
//! callback that fails three times in a row is switched off and says so, so a broken handler
//! cannot flood the console or slow every frame.

use std::rc::Rc;

use rhai::{AST, Dynamic, Engine, FnPtr, INT, Map};

use super::conv::{id_dyn, map_of};
use crate::error::{R, rt};
use crate::host::Shared;

/// Most tools and callbacks scripts may have registered at once.
pub const MAX_REGISTERED: usize = 128;
/// A callback is switched off after this many failures in a row.
pub const MAX_FAILURES: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    SceneLoaded,
    SelectionChanged,
    SceneChanged,
    FrameChanged,
}

impl EventKind {
    pub const ALL: [EventKind; 4] = [EventKind::SceneLoaded, EventKind::SelectionChanged, EventKind::SceneChanged, EventKind::FrameChanged];

    pub fn name(self) -> &'static str {
        match self {
            EventKind::SceneLoaded => "scene_loaded",
            EventKind::SelectionChanged => "selection_changed",
            EventKind::SceneChanged => "scene_changed",
            EventKind::FrameChanged => "frame_changed",
        }
    }

    pub fn from_name(s: &str) -> Option<EventKind> {
        EventKind::ALL.into_iter().find(|k| k.name() == s)
    }
}

/// An event the app reports to the session.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A project was opened, imported or started new.
    SceneLoaded,
    SelectionChanged,
    /// Something in the scene changed (by the user, a script or an agent).
    SceneChanged,
    /// The current frame of the timeline changed.
    FrameChanged {
        frame: f64,
        /// Frame changes folded into this one while the timeline played (0 when none were).
        skipped: u64,
    },
}

impl Event {
    pub fn kind(&self) -> EventKind {
        match self {
            Event::SceneLoaded => EventKind::SceneLoaded,
            Event::SelectionChanged => EventKind::SelectionChanged,
            Event::SceneChanged => EventKind::SceneChanged,
            Event::FrameChanged { .. } => EventKind::FrameChanged,
        }
    }
}

/// A registered tool, as the app lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolInfo {
    pub label: String,
    pub tip: String,
    /// The script that registered it.
    pub owner: String,
}

pub(crate) struct Tool {
    pub info: ToolInfo,
    pub f: FnPtr,
    pub ast: Rc<AST>,
}

pub(crate) struct Handler {
    pub kind: EventKind,
    pub owner: String,
    pub f: FnPtr,
    pub ast: Rc<AST>,
    pub failures: u32,
}

#[derive(Default)]
pub(crate) struct Registry {
    pub tools: Vec<Tool>,
    pub handlers: Vec<Handler>,
    /// The script being run (what new registrations belong to) and its functions.
    pub owner: String,
    pub ast: Option<Rc<AST>>,
}

impl Registry {
    /// Forget what `owner` registered, before it runs again.
    pub fn forget(&mut self, owner: &str) {
        self.tools.retain(|t| t.info.owner != owner);
        self.handlers.retain(|h| h.owner != owner);
    }
}

/// The map a callback receives.
pub fn event_map(e: &Event, selection: &[modelcraft_engine::scene::ObjectId]) -> Map {
    let mut pairs = vec![("event", Dynamic::from(e.kind().name().to_string()))];
    match e {
        Event::SelectionChanged => pairs.push(("selection", Dynamic::from_array(selection.iter().map(|i| id_dyn(*i)).collect()))),
        Event::FrameChanged { frame, skipped } => {
            pairs.push(("frame", Dynamic::from_float(*frame)));
            pairs.push(("skipped", Dynamic::from_int(INT::try_from(*skipped).unwrap_or(INT::MAX))));
        }
        _ => {}
    }
    map_of(pairs)
}

fn add_tool(s: &Shared, label: &str, tip: &str, f: FnPtr) -> R<()> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 80 {
        return Err(rt("a tool needs a label of 1 to 80 characters"));
    }
    let mut r = s.registry.try_borrow_mut().map_err(|_| rt("the tool list is busy"))?;
    let owner = r.owner.clone();
    let Some(ast) = r.ast.clone() else { return Err(rt("tools can only be registered while a script runs")) };
    r.tools.retain(|t| !(t.info.owner == owner && t.info.label == label));
    if r.tools.len() + r.handlers.len() >= MAX_REGISTERED {
        return Err(rt(format!("scripts may register at most {MAX_REGISTERED} tools and callbacks")));
    }
    r.tools.push(Tool { info: ToolInfo { label: label.to_string(), tip: tip.chars().take(200).collect(), owner }, f, ast });
    Ok(())
}

fn add_handler(s: &Shared, kind: EventKind, f: FnPtr) -> R<()> {
    let mut r = s.registry.try_borrow_mut().map_err(|_| rt("the callback list is busy"))?;
    let owner = r.owner.clone();
    let Some(ast) = r.ast.clone() else { return Err(rt("callbacks can only be registered while a script runs")) };
    if r.tools.len() + r.handlers.len() >= MAX_REGISTERED {
        return Err(rt(format!("scripts may register at most {MAX_REGISTERED} tools and callbacks")));
    }
    r.handlers.push(Handler { kind, owner, f, ast, failures: 0 });
    Ok(())
}

pub(crate) fn register(engine: &mut Engine, s: &Rc<Shared>) {
    let sh = s.clone();
    engine.register_fn("register_tool", move |label: &str, f: FnPtr| add_tool(&sh, label, "", f));
    let sh = s.clone();
    engine.register_fn("register_tool", move |label: &str, tip: &str, f: FnPtr| add_tool(&sh, label, tip, f));
    let sh = s.clone();
    engine.register_fn("on_event", move |name: &str, f: FnPtr| {
        let kind =
            EventKind::from_name(name).ok_or_else(|| rt(format!("unknown event {name:?} (events: {})", EventKind::ALL.map(EventKind::name).join(", "))))?;
        add_handler(&sh, kind, f)
    });
    for kind in EventKind::ALL {
        let sh = s.clone();
        engine.register_fn(format!("on_{}", kind.name()), move |f: FnPtr| add_handler(&sh, kind, f));
    }
    let sh = s.clone();
    engine.register_fn("clear_callbacks", move || -> R<()> {
        let mut r = sh.registry.try_borrow_mut().map_err(|_| rt("the callback list is busy"))?;
        let owner = r.owner.clone();
        r.handlers.retain(|h| h.owner != owner);
        Ok(())
    });
    let sh = s.clone();
    engine.register_fn("tool_count", move || -> R<INT> { Ok(sh.registry.try_borrow().map_err(|_| rt("the tool list is busy"))?.tools.len() as INT) });
}
