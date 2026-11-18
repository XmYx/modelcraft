# Plugins

A ModelCraft plugin is a folder of Rhai scripts with a manifest. It adds menu commands and reacts to
events, runs in a sandbox with only the permissions the user granted, and has a version. There is
no native code and no dynamic library loading, by design: the plugin model stays pure Rust and
cannot do anything a script could not (see [scripting.md](scripting.md) for the language and the
limits every script runs under).

Row *Plugin SDK for native extensions* in [target-app-parity.md](target-app-parity.md) is
**partial** for that reason: the reference applications also load native plugins; ModelCraft offers
the script-based half of that, safely.

## A package

```
row.tools/
  plugin.json        the manifest
  main.rhai          the entry script (name given in the manifest)
  lib/shapes.rhai    libraries, imported as import "lib/shapes" as shapes;
  README.md
```

```json
{
  "id": "row.tools",
  "name": "Row Tools",
  "version": "1.2.0",
  "api": 1,
  "entry": "main.rhai",
  "description": "Makes rows of cubes",
  "author": "Ann",
  "permissions": ["register_tools", "edit_scene"]
}
```

| Field | Meaning |
|---|---|
| `id` | 3 to 64 lower-case letters, digits, `.`, `_`, `-`; the folder is named after it |
| `name`, `description`, `author` | shown in the Plugins window and the permission prompt |
| `version` | `major.minor.patch`; an installed version is only replaced by a higher one |
| `api` | the plugin API the plugin was written for (this build: 1). A build refuses a plugin that needs a newer API and keeps running older ones back to `MIN_API_VERSION` |
| `entry` | a `.rhai` file in the package, run once when the plugin starts: it registers tools and callbacks |
| `permissions` | what the plugin asks for, below. Unknown fields and unknown permissions are errors, so a typo never means "none" |

A package holds only `.rhai`, `.json`, `.md` and `.txt` files; at most 200 files, 4 folders deep,
1 MiB each and 4 MiB in all; no hidden files, no links, plain names. `modelcraft_script::plugin`
checks all of this before anything is copied.

## Permissions

| Permission | What the plugin may do | Prompt wording |
|---|---|---|
| `edit_scene` | run commands that change the scene (all of them, `batch` and `undo` included; every change is undoable) | Change your scene |
| `register_tools` | `register_tool`: commands in the Scripts menu | Add commands to the Scripts menu |
| `listen_events` | `on_selection_changed`, `on_scene_changed`, `on_frame_changed`, `on_scene_loaded` | Be told when the selection, scene or frame changes |
| `read_project_files` | read files in the folder of the open project | Read files in the folder of your project |
| `write_project_files` | write files in `plugin-out/<id>/` inside the project's folder, nowhere else | Write files in a plugin-out folder |

Without any permission a plugin can still read the scene (`object_count`, `evaluate`, queries) and
read its own files. It never gets the network, a process, an environment variable, the user's
scripts folder or another plugin's folder: Rhai has no such functions, and the file functions check
the folders above. Each plugin runs in a session of its own, with the usual limits (operations,
time, commands, memory budgets) and its own folder as the library root, so a plugin cannot import
anything outside itself.

## What the user sees

- **Installing.** Plugins window (Scripts menu, or the command palette): *Install from Folder…*
  checks and copies a package into `<data folder>/modelcraft/plugins/<id>/`. A package can also be
  put there by hand; *Reload Plugins* finds it, and a folder that is not a valid plugin is listed
  with the reason.
- **The prompt.** A plugin never runs before the user decides. The window shows the plugin's name,
  version, author and description, the permissions in plain words (the ones that change the user's
  work or read their files are marked), and two buttons. *Don't Allow* has the keyboard focus.
  Allow runs the plugin; Don't Allow leaves it installed and off.
- **Remembered.** The decision is kept in `plugin-grants.json` next to the plugins. An update that
  asks for no more than was granted runs without a prompt; one that asks for something new stops and
  asks about the new permissions only. Turning a refused plugin on asks again.
- **Using it.** Its tools are in the Scripts menu under the plugin's name, run as one undoable
  change; its callbacks run like the user's (same frame pacing, time limits, undo folding), and
  errors show in the Scripts window's output. *Turn Off* stops it; *Remove…* asks, then deletes the
  plugin's folder.

## For authors

Write the entry script like any script, registering what the plugin offers:

```rhai
import "lib/shapes" as shapes;
register_tool("Add Row", "Adds a row of cubes", || {
    undo_group("Add Row", || { shapes::row(5, "c"); });
});
```

Ask for as little as you need; each extra permission is a prompt your users must accept. Bump
`version` for every release, and `api` only when you use something newer. Test a package with
`modelcraft-cli`, which runs scripts without the prompt: `run-script new main.rhai --library .`
(the plugin sandbox itself needs the app's Plugins window).

## Design notes

- *Why scripts only.* Native plugins would need `unsafe` and dynamic loading, would run with the
  full rights of the user and could crash the app. A script cannot do any of that; the sandbox is
  the engine the app already trusts for the user's own scripts, with a policy on top.
- *What is enforced where.* `modelcraft_script::Policy` gates commands, tools and callbacks inside
  every session; `Permissions` gates files; `Library` gates imports; `plugin::sandbox` builds them
  from the grant. The permission prompt and the remembered grant are the app's.
- *Not done.* Plugins are installed from a folder: there is no archive format, signing, registry or
  automatic update. A granted plugin can still use as much of the scripting limits as any script;
  there is no per-plugin memory cap (see the Memory section of [scripting.md](scripting.md)).
