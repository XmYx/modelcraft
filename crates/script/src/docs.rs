//! The API reference: one row per function a script can call, grouped. `registered_names` reads
//! the names back from the engine, and a test requires the table and the engine to agree, so the
//! reference cannot drift from what is registered. The `cmd::` functions are not in the table:
//! they come from the command registry ([`modelcraft_engine::COMMANDS`]) and are listed from it.

use std::collections::BTreeSet;

use modelcraft_engine::COMMANDS;

use crate::api::script_name;
use crate::limits::Limits;
use crate::perm::Permissions;
use crate::session::Session;

/// (group, signature, summary). A signature is `name(args) -> result`; methods are written
/// `Type.name(args)`. Positions and vectors are `[x, y, z]` arrays of numbers; ids are integers.
pub const API: &[(&str, &str, &str)] = &[
    (
        "Commands",
        "run(command) -> map",
        "Run a command given as a map with a \"cmd\" field, exactly the JSON the CLI and control channel take. Returns #{created: [ids], warnings: [text]}. The cmd:: functions below are the same thing with the name filled in.",
    ),
    ("Commands", "commands() -> array", "The command registry: an array of #{name, script_name, summary}."),
    (
        "Commands",
        "undo_group(label, fn)",
        "Run `fn` and make everything it changes one undo step named `label`. If `fn` throws, its changes are rolled back and the error continues. Groups nest. undo and redo are not available inside one.",
    ),
    ("Objects", "object_count() -> int", "How many objects the scene has."),
    (
        "Objects",
        "object_ids() -> array",
        "Every object id in tree order (parents before children). Errors above the array limit: walk with roots() and children(id) instead.",
    ),
    ("Objects", "roots() -> array", "The top-level object ids."),
    ("Objects", "children(id) -> array", "The ids of an object's children, in order."),
    ("Objects", "parent(id) -> id | ()", "The parent id, or () at the top level."),
    ("Objects", "exists(id) -> bool", "Whether an object with this id exists."),
    ("Objects", "find(name) -> id | ()", "The first object with exactly this name, in tree order."),
    (
        "Objects",
        "find_all(pattern) -> array",
        "Ids of the objects whose name matches the pattern (`*` any run of characters, `?` one character; case matters), in tree order.",
    ),
    ("Objects", "name_of(id) -> string", "The object's name."),
    ("Objects", "kind_of(id) -> string", "\"Mesh\", \"Group\", \"Smooth cage\" or \"Deformer\"."),
    (
        "Objects",
        "object_info(id) -> map",
        "One map with id, name, kind, parent, children, visible, translation, rotation_euler, scale, material, steps (count), vertices and faces (of the evaluated mesh).",
    ),
    (
        "Objects",
        "attributes(id) -> array",
        "The attribute names of this object: name, visible, translation, rotation_euler (degrees), scale, material, plus primitive, levels or deformer for those kinds.",
    ),
    ("Objects", "get_attr(id, name) -> value", "Read an attribute. Vectors are [x, y, z]; material, primitive and deformer are maps."),
    (
        "Objects",
        "set_attr(id, name, value)",
        "Set an attribute through its command (one undo step). A material, primitive or deformer map changes only the keys it names.",
    ),
    ("Edit", "add_primitive(kind) -> id", "Add a primitive: cube, plane, sphere, round_cube, cylinder, cone or torus."),
    ("Edit", "add_primitive(kind, name) -> id", "Add a primitive with a name (made unique when taken)."),
    ("Edit", "add_group() -> id", "Add an empty group."),
    ("Edit", "add_group(name) -> id", "Add an empty group with a name."),
    ("Edit", "delete(ids)", "Delete an object (or an array of them) and its children."),
    ("Edit", "duplicate(id) -> id", "Duplicate an object with its children; returns the new root."),
    ("Edit", "rename(id, name)", "Rename an object."),
    ("Edit", "reparent(id, parent)", "Move an object under another."),
    ("Edit", "unparent(id)", "Move an object to the top level."),
    ("Edit", "set_visible(id, visible)", "Show or hide an object."),
    ("Transforms", "get_translation(id) -> [x, y, z]", "The object's translation, in its parent's space."),
    ("Transforms", "get_rotation_euler(id) -> [x, y, z]", "The object's rotation as XYZ Euler angles in degrees."),
    ("Transforms", "get_scale(id) -> [x, y, z]", "The object's scale."),
    ("Transforms", "move_to(id, x, y, z)", "Set the translation."),
    ("Transforms", "move_by(id, dx, dy, dz)", "Add to the translation."),
    ("Transforms", "rotate_to(id, x, y, z)", "Set the rotation from XYZ Euler angles in degrees."),
    ("Transforms", "scale_to(id, x, y, z)", "Set the scale."),
    ("Transforms", "world_position(id) -> [x, y, z]", "Where the object's origin is in the world."),
    ("Transforms", "world_bounds(id) -> map | ()", "#{min, max} of the world-space box around the object and its children, or () when they have no geometry."),
    ("Selection", "selection() -> array", "The selected object ids."),
    ("Selection", "set_selection(ids)", "Replace the selection with an id or an array of ids."),
    ("Selection", "select(id)", "Add an object to the selection."),
    ("Selection", "deselect(id)", "Remove an object from the selection."),
    ("Selection", "clear_selection()", "Empty the selection."),
    ("Selection", "is_selected(id) -> bool", "Whether an object is selected."),
    (
        "Dependency graph",
        "dependencies(id) -> array",
        "The objects whose values this object's evaluation reads: its visible deformer children (for a mesh), its first mesh child (for a smooth cage) and its parent (for its world transform).",
    ),
    ("Dependency graph", "dependents(id) -> array", "The objects that read this one: the inverse of dependencies."),
    (
        "Dependency graph",
        "graph_edges() -> array",
        "Every edge as #{from, to, kind} meaning `from` reads `to`; kind is \"deformer\", \"input\" or \"transform\".",
    ),
    (
        "Evaluation",
        "evaluate(id) -> Mesh",
        "The object's final evaluated mesh in its own space (history steps, deformers, smooth cage). Groups and deformers give an empty mesh. Cached, so repeated calls are cheap.",
    ),
    ("Evaluation", "evaluate_step(id, index) -> Mesh", "The mesh after history step `index` (0 is the first), before deformers."),
    ("Evaluation", "steps(id) -> array", "The object's history steps as #{index, name, enabled}."),
    ("Evaluation", "evaluation_errors(id) -> array", "Steps that could not run, as #{step, message} (step is () for the object itself)."),
    ("Mesh", "Mesh.vertex_count() -> int", "Number of vertices."),
    ("Mesh", "Mesh.face_count() -> int", "Number of faces."),
    ("Mesh", "Mesh.edge_count() -> int", "Number of edges."),
    ("Mesh", "Mesh.triangle_count() -> int", "Number of triangles after triangulating the faces."),
    ("Mesh", "Mesh.position(i) -> [x, y, z]", "One vertex position. Errors when i is outside 0..vertex_count."),
    ("Mesh", "Mesh.face(i) -> array", "The vertex numbers of face i, in order."),
    ("Mesh", "Mesh.face_normal(i) -> [x, y, z]", "Unit normal of face i."),
    ("Mesh", "Mesh.face_center(i) -> [x, y, z]", "Centre of face i."),
    ("Mesh", "Mesh.triangle(i) -> array", "The three vertex numbers of triangle i."),
    ("Mesh", "Mesh.bounds() -> map | ()", "#{min, max}, or () for an empty mesh."),
    ("Mesh", "Mesh.surface_area() -> float", "Total area."),
    ("Mesh", "Mesh.is_closed() -> bool", "True when every edge has two faces."),
    ("Mesh", "Mesh.volume() -> float", "Enclosed volume; errors unless the mesh is closed."),
    ("Mesh", "Mesh.stats() -> map", "#{vertices, faces, edges, triangles, closed, area, volume} (volume is () when open)."),
    ("Mesh", "Mesh.positions() -> array", "Every position as [x, y, z]. Errors above the array limit; use positions_range."),
    ("Mesh", "Mesh.positions_range(start, count) -> array", "Positions start..start+count (cut at the end)."),
    ("Mesh", "Mesh.faces() -> array", "Every face as an array of vertex numbers. Errors above the array limit; use faces_range."),
    ("Mesh", "Mesh.faces_range(start, count) -> array", "Faces start..start+count (cut at the end)."),
    ("Mesh", "Mesh.indices() -> array", "Triangle indices, three per triangle, flat. Errors above the array limit."),
    ("Mesh", "Mesh.to_builder() -> MeshBuilder", "A copy of the geometry to change and build as a new object."),
    ("Mesh builder", "mesh_builder() -> MeshBuilder", "An empty builder."),
    ("Mesh builder", "MeshBuilder.vertex_count() -> int", "Vertices added so far."),
    ("Mesh builder", "MeshBuilder.face_count() -> int", "Faces added so far."),
    ("Mesh builder", "MeshBuilder.vertex(x, y, z) -> int", "Add a vertex; returns its number."),
    (
        "Mesh builder",
        "MeshBuilder.face(vertices) -> int",
        "Add a face from an array of vertex numbers (counter-clockwise seen from outside). The vertices must exist.",
    ),
    ("Mesh builder", "MeshBuilder.triangle(a, b, c) -> int", "Add a triangle."),
    ("Mesh builder", "MeshBuilder.quad(a, b, c, d) -> int", "Add a quad."),
    ("Mesh builder", "MeshBuilder.add_box(min, max)", "Add an axis-aligned box (8 vertices, 6 quads) between two [x, y, z] corners."),
    ("Mesh builder", "MeshBuilder.build(name) -> id", "Make a mesh object (the add_mesh command, one undo step)."),
    ("Mesh builder", "MeshBuilder.build(name, parent) -> id", "Make a mesh object under a parent."),
    (
        "Mesh positions",
        "edit_positions(id) -> PositionEdit",
        "Copy the vertex positions of an object that stores its own mesh (made by build, an import, or collapse). Primitives: collapse them first.",
    ),
    ("Mesh positions", "PositionEdit.len() -> int", "Number of vertices."),
    ("Mesh positions", "PositionEdit.get(i) -> [x, y, z]", "One position; errors outside 0..len."),
    ("Mesh positions", "PositionEdit.set(i, x, y, z)", "Change one position; errors outside 0..len or for a non-finite value."),
    ("Mesh positions", "PositionEdit.translate_all(dx, dy, dz)", "Move every vertex."),
    ("Mesh positions", "PositionEdit.commit()", "Write the positions back (the set_positions command, one undo step)."),
    ("Files", "read_text(path) -> string", "Read a UTF-8 text file inside a folder the run may read."),
    ("Files", "write_text(path, text)", "Write a text file atomically inside a folder the run may write."),
    ("Files", "file_exists(path) -> bool", "Whether a path inside a readable folder exists."),
    ("Files", "list_dir(path) -> array", "File names in a readable folder, sorted."),
    ("Files", "export_scene(path)", "Export the visible scene to .glb, .obj or .stl inside a writable folder."),
    ("Files", "script_dir() -> string", "The folder relative paths start from."),
    ("Files", "may_write_files() -> bool", "Whether this run may write anywhere."),
    (
        "Tools and callbacks",
        "register_tool(label, fn)",
        "Add a command to the app's Scripts menu that runs `fn`. Registering the same label again replaces it; running the script again replaces everything it registered.",
    ),
    ("Tools and callbacks", "register_tool(label, tip, fn)", "The same with a tooltip."),
    ("Tools and callbacks", "tool_count() -> int", "How many tools are registered."),
    (
        "Tools and callbacks",
        "on_event(name, fn)",
        "Call `fn(event)` when an event happens. Names: scene_loaded, selection_changed, scene_changed, frame_changed. `event` is #{event, selection} or #{event, frame} where it has them.",
    ),
    ("Tools and callbacks", "on_scene_loaded(fn)", "Call `fn(event)` when a project is opened, imported or started new."),
    ("Tools and callbacks", "on_selection_changed(fn)", "Call `fn(event)` when the selection changes; `event.selection` is the new selection."),
    (
        "Tools and callbacks",
        "on_scene_changed(fn)",
        "Call `fn(event)` when anything in the scene changed, whoever changed it (callbacks do not trigger themselves).",
    ),
    ("Tools and callbacks", "on_frame_changed(fn)", "Call `fn(event)` when the current frame changed; `event.frame` is the new frame."),
    ("Tools and callbacks", "clear_callbacks()", "Remove the callbacks this script registered."),
    ("Arguments", "args() -> map", "The `--arg name=value` pairs of the run, with values read as true/false, whole numbers, numbers or text."),
    ("Arguments", "arg(name, default) -> value", "One argument, or the default when it was not given."),
    ("Arguments", "current_file() -> string | ()", "The file name of the open project, or () when it has none."),
    ("Files", "make_dir(path)", "Create a folder (and its parents) inside a folder the run may write."),
    ("Math", "round_to(x, digits) -> float", "Round a number to 0 to 12 decimal places."),
    ("Random numbers", "rng(seed) -> Rng", "A deterministic random generator: the same seed gives the same numbers on every machine."),
    ("Random numbers", "Rng.next() -> float", "A number in [0, 1)."),
    ("Random numbers", "Rng.range(lo, hi) -> float", "A number in [lo, hi)."),
    ("Random numbers", "Rng.int_range(lo, hi) -> int", "A whole number from lo to hi, both included."),
];

/// The function name of a signature: `Mesh.vertex_count() -> int` is `vertex_count`.
pub fn name_of(signature: &str) -> &str {
    let head = signature.split('(').next().unwrap_or(signature);
    head.rsplit('.').next().unwrap_or(head).trim()
}

/// Function names registered on the engine: (global functions, `cmd::` functions).
pub fn registered_names() -> (BTreeSet<String>, BTreeSet<String>) {
    let session = Session::new(Limits::default(), Permissions::none());
    let json = session.engine().gen_fn_metadata_to_json(false).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
    let names = |f: Option<&serde_json::Value>| -> BTreeSet<String> {
        f.and_then(|f| f.as_array()).into_iter().flatten().filter_map(|f| f["name"].as_str().map(str::to_string)).collect()
    };
    (names(v.get("functions")), names(v.pointer("/modules/cmd/functions")))
}

/// The names `cmd::` has, from the command registry.
pub fn command_script_names() -> BTreeSet<String> {
    COMMANDS.iter().map(|(n, _)| script_name(n).to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_and_the_engine_agree() {
        let (global, cmd) = registered_names();
        let documented: BTreeSet<String> = API.iter().map(|(_, s, _)| name_of(s).to_string()).collect();
        let undocumented: Vec<_> = global.difference(&documented).collect();
        let missing: Vec<_> = documented.difference(&global).collect();
        assert!(undocumented.is_empty(), "registered but not documented: {undocumented:?}");
        assert!(missing.is_empty(), "documented but not registered: {missing:?}");
        assert_eq!(cmd, command_script_names(), "cmd:: must expose exactly the command registry");
    }

    #[test]
    fn every_row_is_well_formed() {
        for (group, sig, summary) in API {
            assert!(!group.is_empty() && !summary.is_empty(), "{sig}");
            assert!(sig.contains('('), "{sig}");
            assert!(summary.ends_with('.') || summary.ends_with(')'), "{sig}: summaries are sentences");
        }
        let mut sigs: Vec<&str> = API.iter().map(|r| r.1).collect();
        sigs.sort_unstable();
        let n = sigs.len();
        sigs.dedup();
        assert_eq!(n, sigs.len(), "duplicate signatures");
    }
}
