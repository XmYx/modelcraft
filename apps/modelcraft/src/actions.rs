//! Every user action, with its label and keyboard shortcut. Menus, the command palette and the
//! shortcut handler all read this one table, so every command has a keyboard path.

use egui::{Key, KeyboardShortcut, Modifiers};
use modelcraft_engine::sculpt::BrushKind;

use crate::app::Workspace;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    New,
    Open,
    Save,
    SaveAs,
    Import,
    ExportGlb,
    ExportObj,
    ExportStl,
    Undo,
    Redo,
    Delete,
    Duplicate,
    Rename,
    Frame,
    ToggleQuad,
    Workspace(Workspace),
    ModeObject,
    ModeVertex,
    ModeEdge,
    ModeFace,
    ToolSelect,
    ToolMove,
    ToolRotate,
    ToolScale,
    AddPrimitive(&'static str),
    AddCage,
    AddDeformer(&'static str),
    AddGroup,
    Extrude,
    Inset,
    Bevel,
    LoopCut,
    Merge,
    Subdivide,
    Triangulate,
    Flip,
    Collapse,
    SelectAll,
    SelectNone,
    Brush(BrushKind),
    Symmetry,
    MaskClear,
    MaskInvert,
    MaskFill,
    BrushBigger,
    BrushSmaller,
    ViewPerspective,
    ViewFront,
    ViewRight,
    ViewTop,
    Palette,
    NextPanel,
    TransformDialog,
    About,
}

pub const PRIMITIVES: &[&str] = &["cube", "plane", "sphere", "round_cube", "cylinder", "cone", "torus"];
pub const DEFORMERS: &[&str] = &["bend", "twist", "taper", "ripple", "round"];

pub fn all() -> Vec<Action> {
    use Action::*;
    let mut v = vec![
        New,
        Open,
        Save,
        SaveAs,
        Import,
        ExportGlb,
        ExportObj,
        ExportStl,
        Undo,
        Redo,
        Delete,
        Duplicate,
        Rename,
        Frame,
        ToggleQuad,
        Workspace(crate::app::Workspace::Model),
        Workspace(crate::app::Workspace::Sculpt),
        Workspace(crate::app::Workspace::Paint),
        ModeObject,
        ModeVertex,
        ModeEdge,
        ModeFace,
        ToolSelect,
        ToolMove,
        ToolRotate,
        ToolScale,
    ];
    v.extend(PRIMITIVES.iter().map(|p| AddPrimitive(p)));
    v.extend([AddCage, AddGroup]);
    v.extend(DEFORMERS.iter().map(|d| AddDeformer(d)));
    v.extend([Extrude, Inset, Bevel, LoopCut, Merge, Subdivide, Triangulate, Flip, Collapse, SelectAll, SelectNone]);
    v.extend(modelcraft_engine::sculpt::brush::KINDS.iter().map(|k| Brush(*k)));
    v.extend([
        Symmetry,
        MaskClear,
        MaskInvert,
        MaskFill,
        BrushBigger,
        BrushSmaller,
        ViewPerspective,
        ViewFront,
        ViewRight,
        ViewTop,
        TransformDialog,
        Palette,
        NextPanel,
        About,
    ]);
    v
}

fn prim_label(p: &str) -> String {
    match p {
        "cube" => tl!("Cube"),
        "plane" => tl!("Plane"),
        "sphere" => tl!("Sphere"),
        "round_cube" => tl!("Quad sphere"),
        "cylinder" => tl!("Cylinder"),
        "cone" => tl!("Cone"),
        "torus" => tl!("Torus"),
        _ => p.to_string(),
    }
}

pub fn deformer_label(d: &str) -> String {
    match d {
        "bend" => tl!("Bend"),
        "twist" => tl!("Twist"),
        "taper" => tl!("Taper"),
        "ripple" => tl!("Ripple"),
        "round" => tl!("Round"),
        _ => d.to_string(),
    }
}

pub fn brush_label(k: BrushKind) -> String {
    match k {
        BrushKind::Draw => tl!("Draw"),
        BrushKind::Clay => tl!("Clay"),
        BrushKind::Smooth => tl!("Smooth"),
        BrushKind::Inflate => tl!("Inflate"),
        BrushKind::Grab => tl!("Grab"),
        BrushKind::Pinch => tl!("Pinch"),
        BrushKind::Flatten => tl!("Flatten"),
        BrushKind::Crease => tl!("Crease"),
        BrushKind::Mask => tl!("Mask"),
    }
}

impl Action {
    pub fn label(self) -> String {
        use Action::*;
        match self {
            New => tl!("New"),
            Open => tl!("Open…"),
            Save => tl!("Save"),
            SaveAs => tl!("Save As…"),
            Import => tl!("Import…"),
            ExportGlb => tl!("Export glTF Binary (.glb)…"),
            ExportObj => tl!("Export OBJ…"),
            ExportStl => tl!("Export STL…"),
            Undo => tl!("Undo"),
            Redo => tl!("Redo"),
            Delete => tl!("Delete"),
            Duplicate => tl!("Duplicate"),
            Rename => tl!("Rename"),
            Frame => tl!("Frame Selection"),
            ToggleQuad => tl!("Toggle Four Views"),
            Workspace(crate::app::Workspace::Model) => tl!("Model Workspace"),
            Workspace(crate::app::Workspace::Sculpt) => tl!("Sculpt Workspace"),
            Workspace(crate::app::Workspace::Paint) => tl!("Paint Workspace"),
            ModeObject => tl!("Object Mode"),
            ModeVertex => tl!("Vertex Mode"),
            ModeEdge => tl!("Edge Mode"),
            ModeFace => tl!("Face Mode"),
            ToolSelect => tl!("Select Tool"),
            ToolMove => tl!("Move Tool"),
            ToolRotate => tl!("Rotate Tool"),
            ToolScale => tl!("Scale Tool"),
            AddPrimitive(p) => crate::tlf!("Add {shape}", shape = prim_label(p)),
            AddCage => tl!("Add Smooth Cage"),
            AddDeformer(d) => crate::tlf!("Add {shape} Deformer", shape = deformer_label(d)),
            AddGroup => tl!("Add Group"),
            Extrude => tl!("Extrude"),
            Inset => tl!("Inset"),
            Bevel => tl!("Bevel"),
            LoopCut => tl!("Loop Cut"),
            Merge => tl!("Merge Vertices"),
            Subdivide => tl!("Subdivide"),
            Triangulate => tl!("Triangulate"),
            Flip => tl!("Flip Normals"),
            Collapse => tl!("Collapse History"),
            SelectAll => tl!("Select All"),
            SelectNone => tl!("Select None"),
            Brush(k) => crate::tlf!("{brush} Brush", brush = brush_label(k)),
            Symmetry => tl!("Toggle X Symmetry"),
            MaskClear => tl!("Clear Mask"),
            MaskInvert => tl!("Invert Mask"),
            MaskFill => tl!("Mask All"),
            BrushBigger => tl!("Larger Brush"),
            BrushSmaller => tl!("Smaller Brush"),
            ViewPerspective => tl!("Perspective View"),
            ViewFront => tl!("Front View"),
            ViewRight => tl!("Right View"),
            ViewTop => tl!("Top View"),
            TransformDialog => tl!("Transform by Numbers…"),
            Palette => tl!("Command Palette…"),
            NextPanel => tl!("Focus Next Panel"),
            About => tl!("About ModelCraft"),
        }
    }

    /// The shortcut in a workspace (single letters mean different things when sculpting).
    pub fn shortcut(self, ws: Workspace) -> Option<KeyboardShortcut> {
        use Action::*;
        let cmd = |k| Some(KeyboardShortcut::new(Modifiers::COMMAND, k));
        let cmd_shift = |k| Some(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, k));
        let shift = |k| Some(KeyboardShortcut::new(Modifiers::SHIFT, k));
        let alt = |k| Some(KeyboardShortcut::new(Modifiers::ALT, k));
        let key = |k| Some(KeyboardShortcut::new(Modifiers::NONE, k));
        let model = ws == crate::app::Workspace::Model;
        let sculpt = ws == crate::app::Workspace::Sculpt;
        match self {
            New => cmd(Key::N),
            Open => cmd(Key::O),
            Save => cmd(Key::S),
            SaveAs => cmd_shift(Key::S),
            Import => cmd(Key::I),
            ExportGlb => cmd_shift(Key::E),
            Undo => cmd(Key::Z),
            Redo => cmd_shift(Key::Z),
            Delete => key(Key::Delete),
            Duplicate => cmd(Key::D),
            Rename => key(Key::F2),
            Frame => key(Key::F),
            ToggleQuad => key(Key::Space),
            Workspace(crate::app::Workspace::Model) => cmd(Key::Num1),
            Workspace(crate::app::Workspace::Sculpt) => cmd(Key::Num2),
            Workspace(crate::app::Workspace::Paint) => cmd(Key::Num3),
            ModeObject if model => key(Key::Num1),
            ModeVertex if model => key(Key::Num2),
            ModeEdge if model => key(Key::Num3),
            ModeFace if model => key(Key::Num4),
            ToolSelect if model => key(Key::Q),
            ToolMove if model => key(Key::W),
            ToolRotate if model => key(Key::E),
            ToolScale if model => key(Key::R),
            Extrude if model => shift(Key::E),
            Inset if model => shift(Key::I),
            Bevel if model => shift(Key::B),
            LoopCut if model => shift(Key::L),
            Merge if model => shift(Key::M),
            Subdivide if model => shift(Key::D),
            Collapse => cmd_shift(Key::C),
            SelectAll => cmd(Key::A),
            SelectNone => key(Key::Escape),
            Brush(k) if sculpt => key(match k {
                BrushKind::Draw => Key::D,
                BrushKind::Clay => Key::C,
                BrushKind::Smooth => Key::S,
                BrushKind::Inflate => Key::I,
                BrushKind::Grab => Key::G,
                BrushKind::Pinch => Key::P,
                BrushKind::Flatten => Key::L,
                BrushKind::Crease => Key::K,
                BrushKind::Mask => Key::M,
            }),
            Symmetry if sculpt => key(Key::X),
            MaskInvert if sculpt => cmd(Key::Num9),
            MaskClear if sculpt => alt(Key::M),
            BrushBigger => key(Key::CloseBracket),
            BrushSmaller => key(Key::OpenBracket),
            ViewPerspective => alt(Key::Num1),
            ViewFront => alt(Key::Num2),
            ViewRight => alt(Key::Num3),
            ViewTop => alt(Key::Num4),
            TransformDialog => cmd(Key::T),
            Palette => cmd(Key::K),
            NextPanel => key(Key::F6),
            _ => None,
        }
    }

    pub fn shortcut_text(self, ctx: &egui::Context, ws: Workspace) -> String {
        self.shortcut(ws).map(|s| ctx.format_shortcut(&s)).unwrap_or_default()
    }
}
