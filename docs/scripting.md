# Scripting

ModelCraft embeds [Rhai](https://rhai.rs), a pure-Rust scripting language. A script drives the
same command registry as the app, `modelcraft-cli` and the control channel, reads the scene, and
runs inside a sandbox with limits. This page records the language choice, the model, the sandbox
and what is measured. The function-by-function reference is the table in
`crates/script/src/docs.rs`; a test requires it to list exactly the functions the engine registers.

```
modelcraft-cli run-script scene.modelcraft stairs.rhai -o out.modelcraft
modelcraft-cli run-script new scatter.rhai -o scatter.modelcraft --time-limit 5
modelcraft-cli run-script scene.modelcraft export.rhai --allow-write exports/ --no-save
```

```rhai
let step_count = 12;
undo_group("Staircase", || {
    for i in 0..step_count {
        let s = add_primitive("cube", "Step " + i);
        scale_to(s, 1.0, 0.1, 0.3);
        move_to(s, 0, 0.2 * i, 0.3 * i);
    }
});
print(`${object_count()} objects`);
```

## Why Rhai

The choice was between the two mature pure-Rust embeddable languages whose licences are on the
allowlist: Rhai 1.27 and Rune 0.14.2 (both `MIT OR Apache-2.0`). Both were built and run on the
same machine (release build, `CARGO_BUILD_JOBS=3`), with the same hostile scripts. Rows marked
"not evaluated" are things only Rhai was taken far enough to judge.

| | Rhai 1.27 | Rune 0.14.2 |
|---|---|---|
| Version stability | 1.x | 0.x, breaking changes between minor versions |
| 2 million loop iterations (release build) | 291 ms | 181 ms |
| Endless loop | stopped by the operation limit in 2.5 ms (`set_max_operations`) | stopped by `budget::with` in 16 ms |
| Runaway recursion | stopped at the call-depth limit (81 µs) | stopped by its allocation limit (64 ms) |
| Runaway string and array growth | string and array size limits (nested containers count together) | the allocation limit stops both |
| Memory | no allocator cap: container size limits, an operation limit, and a per-run budget for the geometry the API hands out (see below) | a real allocation limit for the whole VM (`alloc::limit::with`) |
| Command arguments from script values | `rhai::serde` converts a script map straight into `serde_json::Value`, so `Command`'s JSON form is the script form | not evaluated (a bridge to `Command` would have to be written) |
| Closures and function pointers (callbacks, `undo_group`) | built in | built in |
| Errors with a place | every error has line and column, parse and run time | not evaluated beyond compile diagnostics, which have spans |
| Limits a script cannot catch | operation, depth, size and our own limits are not catchable by `try`/`catch` | not evaluated |

Rune is faster, and its allocator limit is a better memory guarantee than anything Rhai offers.
Rhai was chosen because a 1.x API will not churn under the app, because its map and array literals
map one-to-one onto the command registry's JSON (no per-command bindings to write or keep in step),
and because every error carries the file, line and column the script editor needs. The speed
difference does not matter for scripts whose time goes into commands; if scripts ever need the
raw speed or a hard memory cap, Rune is the language to evaluate again.

Dependencies Rhai adds: `rhai`, `rhai_codegen` (proc-macro), `smartstring` (`MPL-2.0+`, which the
house rules allow for unmodified third-party use), `thin-vec`, `ahash`, `once_cell`, `smallvec`,
`num-traits` and their dependencies, all `MIT OR Apache-2.0` except `smartstring` and
`tiny-keccak` (`CC0-1.0`). `cargo xtask licenses` checks them.

## The Scripts window

Scripts menu, or Ctrl+J. The editor colours keywords, the script API's function names, strings,
numbers and comments (the tokenizer is `modelcraft_script::highlight`, UI-free and tested to cover
any text exactly), numbers the lines and marks the line of the last error.

| Key (in the editor) | Does |
|---|---|
| Ctrl+Enter | Run the script |
| Ctrl+Shift+Enter | Run the selection; errors keep the line numbers of the file |
| Ctrl+S | Save the script file |
| Esc | Close the window and return to the viewport |

The Output panel lists `print` and `debug` lines, command warnings and errors as
`file:line:column: message`; an error is a button that moves the cursor to its line. A run happens
on the UI thread and is bounded by the limits below, so a runaway script cannot freeze the window
for more than the time limit; Undo reverts what it did. Unsaved edits are never discarded without
a prompt (New, Open and choosing a file ask first).

**Files.** The scripts folder is `<user data folder>/modelcraft/scripts`. Its `*.rhai` files and
one level of subfolders become the Scripts menu (the first comment line is the tooltip); the
`quick` subfolder becomes the Quick Scripts strip under the tool options; `startup.rhai` runs once
when the app starts. A script run from the window may read the script's folder, the project's
folder and the scripts folder, and write nowhere unless "Allow writing files" is on. Then the
window lists the folders it grants: the project's folder (a `scripts-out` folder inside it when the
project is saved straight into the user's home folder or a filesystem root) and the scripts folder.

## The model

- **Everything a script changes is a command.** Every entry of
  `modelcraft_engine::COMMANDS` is a function `cmd::<name>(map)` taking the command's JSON as a
  map (`import` is `cmd::import_file`, because `import` is a Rhai keyword), and
  `run(#{ cmd: "name", ... })` takes the whole command. The functions are generated from the
  registry, so a command added there appears in scripts by itself, and a test requires the two
  lists to be equal. A script's changes are journaled and undoable exactly like the app's.
- **Friendly helpers** wrap the common commands (`add_primitive`, `move_to`, `set_attr`, ...);
  they run the same commands.
- **Queries** read the scene without changing it: objects, attributes, selection, the dependency
  graph (`dependencies`, `dependents`, `graph_edges`), the evaluated mesh of an object or of one
  of its history steps (`evaluate`, `evaluate_step`), and evaluation errors.
- **Meshes** are handles: `Mesh` (read-only, from `evaluate`), `MeshBuilder` (positions and faces
  to a new object) and `PositionEdit` (positions of an object that stores its own mesh). Every
  index is checked and named in its error: `vertex 8 is out of range (there are 8, numbered from
  0)`.
- **Undo grouping.** `undo_group("label", || { ... })` makes everything inside one undo step. If
  the closure fails, its changes are rolled back and nothing is left to redo. `batch` is the same
  thing as a command: a list of commands that run all or none as one step.
- **Selection** belongs to the app, not the document. A run gets the selection in a
  `RunContext` and hands back the changed selection.

## The sandbox

A script has no file or network access and no way to start a process: the engine is built from
Rhai's standard package only, `eval` and modules are switched off, and the functions that touch the
disk (`read_text`, `write_text`, `list_dir`, `file_exists`, `export_scene`, and the `import`
command) check a `Permissions` first. The default grants nothing. A path is made absolute, `..`
is resolved and symbolic links are followed before it is compared with the granted folders, so a
path cannot leave them. A command that names a file is checked too, also inside a `batch`.

`Limits` (defaults in `crates/script/src/limits.rs`):

| Limit | Default | Stops |
|---|---|---|
| Operations | 10,000,000 | endless loops |
| Wall-clock time | 10 s, checked as it runs and before every command | slow commands in a loop, anything the operation limit misses |
| Call depth / expression depth | 24 / 48 | runaway recursion, deeply nested source |
| String bytes (all strings of one container together) | 256 KiB | string doubling |
| Array / map entries (nested ones count) | 20,000 / 2,000 | array growth |
| Commands per run | 100,000 | command floods |
| Geometry built or copied through mesh handles, per run | 4,000,000 vertices, faces and corners | memory from meshes |
| Output lines | 5,000 | console floods |
| File size read or written | 4 MiB | |

Limits a script hits end it with a `Limit` error that `try`/`catch` cannot catch. Plain errors
can be caught.

What is measured, and what is not (Linux, test profile of this workspace):

- An endless loop stops at the operation limit in a few milliseconds; a loop that only counts
  stops at the 10 s time limit.
- The worst memory growth found in a hostile script is adding keys to a map in a loop (Rhai does
  not measure a map when a key is assigned, only when it is built by a function): at the default
  limits it grew the process by about 67 MB (integer values) to 153 MB (an array per key) in about
  one second before the operation limit stopped it. That is the figure to read as the memory bound
  of a hostile script; there is no allocator cap, so it is a measurement, not a guarantee.
- Array growth by `push` is measured per call and rejected at 20,000 entries (Rhai re-measures the
  array on every call, so the cap is kept small; bulk geometry stays in mesh handles).
- A single native command that is already running is not interrupted. Commands bound their own
  work (`MAX_RESULT_FACES`, subdivision levels, the polygon limits), and the time limit is
  checked before the next one.
- Scripts run on the thread that calls `Session::run` (the UI thread in the app). The time limit
  is what keeps the window responsive; a script cannot be stopped from outside while it runs.

## Layering

`modelcraft-script` is an L5 crate beside the app, below it in the stack: it depends on
`modelcraft-engine` and Rhai, never on egui. The app and `modelcraft-cli` use it; nothing in the
core depends on it.
