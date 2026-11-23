//! Scene Nodes: the scene's objects in the node editor ([`crate::node_editor`]).
//!
//! Each object is a node with a **Parent** input and a **Children** output; a link from one
//! object's Children to another's Parent puts the second under the first. Linking, unlinking,
//! adding (primitives and groups, from the node browser) and removing are the scene's own
//! commands (`reparent`, `add_primitive`, `add_group`, `delete`), so each is one undo step and is
//! journaled. Where a node sits on the canvas is the window's view only, like the pan and zoom.

use std::collections::{HashMap, HashSet};

use modelcraft_engine::Command;
use modelcraft_engine::scene::{ObjectId, ObjectKind, Scene};

use crate::actions::{Action, PRIMITIVES};
use crate::app::ModelApp;
use crate::node_editor::{Edit, Entry, GraphSource, Limits, Link, NodeEditor, NodeInfo, Part};

/// Most objects the window shows as nodes.
pub const MAX_SCENE_NODES: usize = 256;
const SALT: &str = "scene-nodes";

/// The window's state.
#[derive(Default)]
pub struct SceneNodesUi {
    pub open: bool,
    pub editor: NodeEditor,
    /// Where each object's node sits (view only).
    pub positions: HashMap<ObjectId, [f32; 2]>,
    /// Whether the keyboard focus was in the window last frame (egui drops it on Esc before
    /// the app sees the key).
    had_focus: bool,
}

/// What an object is, for its node's second line.
pub fn kind_text(k: &ObjectKind) -> String {
    match k {
        ObjectKind::Group => tl!("Group"),
        ObjectKind::Mesh { .. } => tl!("Mesh"),
        ObjectKind::SmoothCage { .. } => tl!("Smooth cage"),
        ObjectKind::Deformer { .. } | ObjectKind::PointDeformer { .. } => tl!("Deformer"),
        ObjectKind::Array { .. } => tl!("Array"),
        ObjectKind::Symmetry { .. } => tl!("Symmetry"),
        ObjectKind::Field { .. } => tl!("Field"),
        ObjectKind::Joint { .. } => tl!("Joint"),
        ObjectKind::Skin { .. } => tl!("Skin"),
        ObjectKind::CloneModifier { .. } => tl!("Clone modifier"),
        ObjectKind::Cloner { .. } => tl!("Cloner"),
        ObjectKind::Instance { .. } => tl!("Instance"),
        ObjectKind::Hair { .. } => tl!("Hair"),
        ObjectKind::Sim { .. } => tl!("Simulation"),
        ObjectKind::Generator { .. } | ObjectKind::SplineGenerator { .. } => tl!("Generator"),
        ObjectKind::Light { .. } => tl!("Light"),
        ObjectKind::Camera { .. } => tl!("Camera"),
        ObjectKind::Spline { .. } => tl!("Spline"),
    }
}

/// The scene as the node editor sees it.
struct SceneGraph<'a> {
    scene: &'a Scene,
    positions: &'a HashMap<ObjectId, [f32; 2]>,
}

impl GraphSource for SceneGraph<'_> {
    fn nodes(&self) -> Vec<NodeInfo> {
        self.scene
            .objects()
            .iter()
            .take(MAX_SCENE_NODES)
            .map(|o| NodeInfo {
                id: o.id.0,
                title: o.name.clone(),
                kind: kind_text(&o.kind),
                inputs: vec![tl!("Parent")],
                outputs: vec![tl!("Children")],
                pos: self.positions.get(&o.id).copied(),
            })
            .collect()
    }

    fn links(&self) -> Vec<Link> {
        let shown: HashSet<u64> = self.scene.objects().iter().take(MAX_SCENE_NODES).map(|o| o.id.0).collect();
        self.scene
            .objects()
            .iter()
            .take(MAX_SCENE_NODES)
            .filter_map(|o| o.parent.filter(|p| shown.contains(&p.0)).map(|p| Link { from: p.0, output: 0, to: o.id.0, input: 0 }))
            .collect()
    }

    fn can_link(&self, l: &Link) -> bool {
        // An object cannot go under itself or under anything inside it.
        let (parent, child) = (ObjectId(l.from), ObjectId(l.to));
        self.scene.get(parent).is_some() && self.scene.get(child).is_some() && !self.scene.subtree(child).contains(&parent)
    }

    fn catalog(&self) -> Vec<Entry> {
        let mut v: Vec<Entry> =
            PRIMITIVES.iter().map(|p| Entry { id: (*p).to_string(), title: Action::AddPrimitive(p).label(), category: tl!("Primitives") }).collect();
        v.push(Entry { id: "group".into(), title: tl!("Empty Group"), category: tl!("Objects") });
        v
    }
}

/// Open or close the window; opening puts the focus on its canvas.
pub fn toggle(app: &mut ModelApp) {
    let s = &mut app.scene_nodes;
    s.open = !s.open;
    if s.open {
        s.editor.focus(Part::Canvas);
    }
}

/// Esc closes the window when its canvas or a node has the focus and the editor does not want
/// the key itself (a link being made, the browser open).
pub fn escape(app: &mut ModelApp, ctx: &egui::Context) -> bool {
    let s = &mut app.scene_nodes;
    if !s.open || s.editor.wants_escape() {
        return false;
    }
    if s.had_focus && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        s.open = false;
        s.had_focus = false;
        return true;
    }
    false
}

pub fn show(app: &mut ModelApp, ctx: &egui::Context) {
    if !app.scene_nodes.open {
        return;
    }
    let mut open = true;
    let mut edits = Vec::new();
    egui::Window::new(tl!("Scene Nodes")).id(egui::Id::unique("scene-nodes")).open(&mut open).default_size([760.0, 480.0]).show(ctx, |ui| {
        let count = app.doc.scene.objects().len();
        if count > MAX_SCENE_NODES {
            ui.label(crate::tlf!("Showing the first {n} of {count} objects.", n = MAX_SCENE_NODES, count = count));
        }
        let s = &mut app.scene_nodes;
        // Keep where the editor first placed each node, so nodes stay put as the scene changes.
        let placed = {
            let graph = SceneGraph { scene: &app.doc.scene, positions: &s.positions };
            let nodes = graph.nodes();
            if nodes.iter().any(|n| n.pos.is_none()) { crate::node_editor::layout(&nodes, &graph.links()) } else { HashMap::new() }
        };
        for (id, p) in placed {
            s.positions.entry(ObjectId(id)).or_insert(p);
        }
        let graph = SceneGraph { scene: &app.doc.scene, positions: &s.positions };
        edits = s.editor.show(ui, SALT, &graph, Limits { max_nodes: MAX_SCENE_NODES, max_links: MAX_SCENE_NODES });
        let layer = ui.layer_id();
        s.had_focus = ui.memory(|m| m.focused()).and_then(|id| ui.ctx().read_response(id)).is_some_and(|r| r.layer_id == layer);
    });
    if !open {
        app.scene_nodes.open = false;
    }
    for e in edits {
        apply(app, e);
    }
}

/// Apply one editor edit as a scene command.
fn apply(app: &mut ModelApp, e: Edit) {
    match e {
        Edit::Add { kind, pos } => {
            let cmd = if kind == "group" { Command::AddGroup { name: None } } else { Command::AddPrimitive { kind, name: None, parent: None } };
            let before: HashSet<ObjectId> = app.doc.scene.objects().iter().map(|o| o.id).collect();
            app.run(cmd);
            if let Some(new) = app.doc.scene.objects().iter().map(|o| o.id).find(|id| !before.contains(id)) {
                app.scene_nodes.positions.insert(new, pos);
            }
        }
        Edit::Remove(id) => {
            app.run(Command::Delete { ids: vec![ObjectId(id)] });
        }
        Edit::Link(l) => {
            app.run(Command::Reparent { id: ObjectId(l.to), parent: Some(ObjectId(l.from)) });
        }
        Edit::Unlink { to, .. } => {
            app.run(Command::Reparent { id: ObjectId(to), parent: None });
        }
        Edit::Move { node, pos } => {
            app.scene_nodes.positions.insert(ObjectId(node), pos);
        }
    }
}
