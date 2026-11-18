//! The agent control channel in the window: craft-control's token-authenticated loopback socket
//! (`modelcraft_engine::control`), answered between frames on the open document. Off unless
//! `--control` is given or Edit ▸ Allow Agents to Control ModelCraft is on.
//! `modelcraft-cli call …` and `modelcraft-cli mcp --app` talk to it.

use modelcraft_engine::control;

use crate::app::ModelApp;

#[derive(Default)]
pub struct Agents {
    pub listening: Option<control::Listening>,
    /// The launch asked for this port (`--control-port`).
    pub port: Option<u16>,
    /// Requests answered since the channel opened.
    pub answered: u64,
    /// A token chosen in advance (tests); none writes a fresh one to the user's token file.
    pub token: Option<String>,
    /// The scripts agents run with `script.run`.
    pub script: modelcraft_script::channel::ScriptChannel,
}

impl Agents {
    pub fn is_on(&self) -> bool {
        self.listening.is_some()
    }
}

/// Open the channel; the status bar says where it listens or why it could not.
pub fn start(app: &mut ModelApp, ctx: &egui::Context) {
    if app.agents.is_on() {
        return;
    }
    let wake = ctx.clone();
    match control::listen(app.agents.port, app.agents.token.clone(), move || wake.request_repaint()) {
        Ok(l) => {
            app.status = crate::tlf!("Agents can control ModelCraft on port {port}", port = l.port);
            app.agents.listening = Some(l);
        }
        Err(e) => app.status = crate::tlf!("The agent channel could not open: {error}", error = e),
    }
}

pub fn stop(app: &mut ModelApp) {
    if let Some(l) = app.agents.listening.take() {
        l.close();
        app.status = crate::tl!("Agent control is off");
    }
}

/// Answer waiting requests. Not while a drag or stroke is under way: they wait for the next
/// frame after it, so an agent never edits under the pointer.
pub fn tick(app: &mut ModelApp) {
    if app.drag.is_some() || app.stroke.is_some() || app.prop_snapshot.is_some() {
        return;
    }
    let Some(l) = &app.agents.listening else { return };
    let before = app.doc.revision;
    let mut selection = app.selection.clone();
    let script = &mut app.agents.script;
    let n = control::answer_pending_with(&mut app.doc, l, &mut |doc, method, params| script.handle(doc, &mut selection, method, params));
    if n == 0 {
        return;
    }
    app.selection = selection;
    app.agents.answered += n as u64;
    if app.doc.revision != before {
        app.prune_selection();
        app.status = crate::tlf!("An agent changed the scene ({count} requests)", count = n);
    }
}
