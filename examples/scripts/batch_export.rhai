// Batch export: writes every top-level object to its own file in a folder.
// Arguments: dir ("exports"), format ("glb", "obj" or "stl"). Needs permission to write files.
let dir = arg("dir", "exports");
let format = arg("format", "glb");
if !may_write_files() {
    throw "batch export writes files: allow writing files (command line: --allow-write DIR)";
}
make_dir(dir);

let tops = roots();
let was_visible = tops.map(|r| get_attr(r, "visible"));
let written = [];
undo_group("Batch export", || {
    for keep in tops {
        // Show only this object, export, and move on.
        for other in tops { set_visible(other, other == keep); }
        let stem = name_of(keep);
        stem.replace("/", "_");
        let file = dir + "/" + stem + "." + format;
        export_scene(file);
        written.push(file);
    }
    for i in 0..tops.len() { set_visible(tops[i], was_visible[i]); }
});
print(`wrote ${written.len()} files to ${dir}`);
written
