<!-- craft-house-rules:start core source=279b2c09ef2b2f0b4f5e616a47d39e29db0a1eb3 sha256=bee0ffafad196b68c76425770e344e588463bbda685ab41581fcbabb85dfc930 -->
## House rules

These apply in every Craft repository. A later rule may be stricter, not weaker. The source is `standards/house-rules-core.md` in `storytold/craft-contrib`. Propose changes there, not in a copy.

1. **A person is accountable.** Whoever runs an agent answers for what it opens and for its review comments.
2. **Check, then claim.** Before writing code, read the issue comments, search pull requests for `#N` and the keywords, and check recent commits on `main`. Comment on the issue first. Release a claim you will not finish.
3. **Verify, don't guess.** Open a definition before using it. Confirm the cause in the code, even when someone names a location.
4. **Keep fixes narrow and tested.** Fix what the issue describes, where the bug is. Add a test that fails without the fix.
5. **Review the diff before opening a pull request.** Identifiers exist as used, the test fails without the fix, nothing unrelated changed, and the description is true.
6. **Look for a better pull request.** If another fixes the same thing, close yours when it is better or equal, and say why.
7. **Report only what ran.** Say what you tested and what you could not run. Without a local build or test, say so and rely on CI before marking ready.
8. **`main` changes through a pull request.** Do not push to it. Branch protection enforces this. There is no exception.
9. **Stop and ask only about** licensing outside the allowlist, publishing, releases, a new remote, secrets, spending money, destructive git, and anything else hard to undo. Otherwise keep working. `plan/` is a gitignored notebook, not rules. Without it, do not ask for it and do not stop.
10. **Keep secrets and personal details out** of the repository, logs and requests.
11. **CI stays shared.** Workflows call `storytold/craft-common-actions`. Do not change CI in a feature or fix pull request. If CI cannot cover a change, open an issue there.
12. **Guards say why.** A check that looks removable says what it prevents. Read that before changing it.
13. **Instructions must be readable by everyone.** No notes about one machine, and no links to files a reader cannot open. `craftrules` is retired. Do not link to it.
14. **Keep pull requests moving.** Concurrent pull requests are fine; unanswered actionable feedback is not. Respond in 7 days, a reminder at 14, closure eligible at 30, from when an actionable request waits on the author. Suggestions and unconfirmed bot findings do not start the clock. A fix, explanation or agreed plan resets it; unrelated activity does not. Waiting on a reviewer or maintainer pauses it and does not count against the author. A maintainer decides closure; it is never automatic. Reuse search results, do not poll, and stop at a rate limit.
15. **No agent attribution.** A pull request does not say an agent wrote it. A repository rule may not require that.
16. **Shared libraries first for Craft apps.** Before adding or changing reusable file-format, codec, protocol or UI code, check `storytold/craft-libs` and relevant open pull requests. Reuse its public APIs or extend the appropriate shared library instead of duplicating an implementation in an app. Keep app-specific policy and model mapping local; preserve behavior, security and resource limits, attribution, layering and regression coverage. Document API gaps and maintainer-approved exceptions rather than silently dropping behavior. This does not authorize dependencies on sibling application crates or override existing platform, licensing or dependency-source requirements.
<!-- craft-house-rules:end -->
<!-- craft-house-rules:start rust source=279b2c09ef2b2f0b4f5e616a47d39e29db0a1eb3 sha256=56ef2b873de68940cf903069bbd0927b119f5c9ec2717f7f627041d2ebe8e4f2 -->
## Rust Craft repositories

These apply to native Craft apps and to shared Rust libraries. Select them by repository type. A repository that merely contains some Rust does not take this block. Rules later in a repository's file may be stricter, but not weaker. The source is `standards/house-rules-rust.md` in [storytold/craft-contrib](https://github.com/storytold/craft-contrib).

1. **Never crash.** Non-test code does not panic and has no `unsafe`, except where the repository file names an exception. Return errors, treat input as hostile, and bound recursion and allocations. New production crate roots copy the `deny` attribute from an existing crate: `clippy::unwrap_used`, `clippy::expect_used`, `clippy::panic`, `clippy::unimplemented`, `clippy::todo`, `clippy::unreachable`. Every crash fix comes with a test that panicked before the fix.
2. **Clean room.** Learn from observed behaviour and public specifications. Do not copy another product's code, assets, strings or artwork. Check a dependency's licence before adding it.
3. **Assets and fonts.** Every asset is original or suitably licensed, and is recorded in `ATTRIBUTION.md` in the same change. Font files live in [storytold/craft-fonts](https://github.com/storytold/craft-fonts), not in the app repository.
4. **Rust only for native Craft apps.** No handwritten JavaScript or TypeScript, and no Tauri, Electron or webview shell, in those apps. Shared tooling follows its documented technical scope and maintainer-approved exceptions. Craft PR Review's existing Tauri and TypeScript UI is one such exception. It does not relax this rule for native apps. Preserve any existing web build.
5. **Time goes through `chrono`.** Do not hand-roll calendar conversion. For dates beyond what `chrono` can represent, follow [standards/time.md](https://github.com/storytold/craft-contrib/blob/main/standards/time.md).
6. **Contributor credits.** Add or edit only your own person's entry in `contributors/people.toml`, and only after asking them. Never guess a name or hand-edit `contributors.json`.

## Licence allowlist

Dependencies and assets use SPDX identifiers. An expression is allowed when every identifier is in this list. A repository may name a shorter list. It may not add an identifier here by editing its own copy of these rules.

- `ISC`
- `MIT`
- `BSD-2-Clause`
- `BSD-3-Clause`
- `Apache-2.0`
- `Unicode-3.0`
- `OFL-1.1`
- `Ubuntu-font-1.0`
- `Bitstream-Vera`
- `CC0-1.0`
- `CC-BY-4.0`
- `CC-BY-SA-4.0`
- `Zlib`
- `BSL-1.0`
- `0BSD`
- `Unlicense`
- `IJG`
- `NCSA`
- `CDLA-Permissive-2.0`
- `Apache-2.0 WITH LLVM-exception`
- `LicenseRef-Public-Domain`

`MPL-2.0` is allowed for third-party dependencies used unmodified; vendoring, patching or copying its code needs a maintainer's approval. `MPL-2.0+` counts as `MPL-2.0`.

`LicenseRef-Storyteller-Trademark` is not in this list. It covers Storyteller brand marks only, where a repository already says so.
<!-- craft-house-rules:end -->

# ModelCraft — rules for agents and contributors

The generated blocks above are the suite-wide house rules from
[storytold/craft-contrib](https://github.com/storytold/craft-contrib) (`standards/house-rules-core.md`
and `standards/house-rules-rust.md`); never edit them by hand, the house-rules action regenerates
them. The accessibility rules below are copied unchanged from craft-contrib
`standards/house-rules-accessibility.md` (the action does not manage that block yet). Naming follows
[standards/naming.md](https://github.com/storytold/craft-contrib/blob/main/standards/naming.md).
The rules after them are ModelCraft's own; when they disagree with the house rules, the house rules
win. `CLAUDE.md` only includes this file.

## Accessibility

These apply to native Craft apps and to shared libraries that draw interface. The source is `standards/house-rules-accessibility.md` in `storytold/craft-contrib`. The full rules are [standards/accessibility.md](https://github.com/storytold/craft-contrib/blob/main/standards/accessibility.md).

People use the apps with screen readers, braille displays, magnifiers, high contrast, large text, voice control or only a keyboard. They reach the interface only through its AccessKit tree, keyboard focus and colours. A UI change is not done until it works for them. The bar is WCAG 2.2 AA as applied to software.

1. **Design first.** Name the pattern (menu, tab list, toolbar, tree, grid, list, dialog, slider, canvas) and follow its keyboard model and roles from the WAI-ARIA Authoring Practices Guide and the platform's guidelines. Observe how the reference app exposes it with Accessibility Insights, Accessibility Inspector or Accerciser.
2. **Keyboard.** Every action works from the keyboard, in visual order, with no trap. Esc closes and returns focus. Alt or F10 and F6 behave as on the platform. Move egui's focus (`request_focus`), never only a highlight.
3. **Semantics.** Every control has a role, a translated name that starts with its visible label, and its state and value. Shortcuts and access keys are set on the node, in the current platform's form. A drawn surface publishes nodes for its content, with caret, selection and correct editability.
4. **Feedback.** Announce changes away from focus through a live region.
5. **Seeing.** Contrast at least 4.5:1 for text and 3:1 for controls and focus, in both themes. Never colour alone. Follow the system's high contrast, text size and reduced motion settings.
6. **Pointer.** Anything done by dragging can be done without dragging.
7. **Check the tree, not only the screenshot.** Each new or changed control, dialog or keyboard path gets a kittest test that drives it by keyboard and asserts roles, names, states, focus and announcements. If the helpers do not exist yet, say so in the pull request. Keep `with_accessibility_check` on and CI's platform checks green. With `ui.screenshot`, also read the accessibility tree.
8. **Rule out known failures:** focus left in the document behind a menu, ribbon or KeyTips; editable surfaces reported read-only; dialogs that do not take or return focus, or leave controls behind them reachable; painted rows with no node; identical names; another platform's shortcut text in a name; silent state changes; drag-only actions.
9. **Be honest in the pull request.** Its Accessibility section says what the tests and the tree cover. Tree tests never prove "works with screen readers". Ask for `needs screen reader test` only for an accessibility fix or accessibility logic of its own, and never hold a pull request back for that test.


## 0. Never crash

People trust ModelCraft with hours of modeling and sculpting. A malformed project or mesh file, a
stale history step, an odd pen event, a 20-million-vertex import or a full disk must give an error
the user can act on, never a crash and never lost work.

1. Fail with `Result`; the UI reports errors in the status bar and carries on. A history step that
   no longer fits its mesh is skipped and flagged, never fatal (`modelcraft_scene::StepError`).
2. No `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!` or `unimplemented!` outside tests. Every
   crate root carries the `#![deny(clippy::unwrap_used, …)]` line; `clippy.toml` allows them in tests.
3. No `unsafe` (`unsafe_code = "forbid"` for the workspace). **One named exception:**
   `crates/craft-render-gpu/src/hw.rs`, compiled only with the off-by-default cargo feature
   `hw-ray-query`, calls `wgpu::ExperimentalFeatures::enabled()` (an `unsafe fn` whose contract is
   that the caller accepts wgpu's experimental features may have bugs) behind one
   `#[allow(unsafe_code)]` function with a `// SAFETY:` comment. It is the only `#[allow(unsafe_code)]`
   in the crate; the default build, and CI, contain no `unsafe` (the crate forbids it unless the
   feature is on). See `docs/architecture.md`.
4. Every input is hostile: project JSON and blobs, glTF/OBJ/STL/PLY (read through `craft-mesh` with
   its `Limits`), selections in history steps, primitive parameters, pointer and pressure values.
   Clamp, validate and cap allocations (`MAX_ELEMENTS`, `MAX_RESULT_FACES`, `subdivide::MAX_LEVELS`,
   the zip limits in `formats/src/project.rs`).
5. Mesh operations rebuild through `PolyMesh::build`, which validates every half-edge invariant: an
   operation cannot leave broken topology behind. Property tests run random edit sequences.
6. Saves are atomic (`craft-atomic`); every command is journaled before the next one runs, so a crash
   loses nothing (`Document::recover`). Undo history is bounded by memory, not by count.

## 1. Assets

No artwork, icons, screenshots, meshes, brushes, stamps, matcaps or sample files from the reference
apps or any other commercial 3D tool, in any form (traced, redrawn or "inspired by" included). Every
asset is original work or openly licensed and listed in [`ATTRIBUTION.md`](ATTRIBUTION.md) in the
same change; `cargo xtask assets` checks it. Demo scenes are built in code (`apps/modelcraft/src/demo.rs`).
Fonts live in [storytold/craft-fonts](https://github.com/storytold/craft-fonts), never here.

## 2. Clean room

ModelCraft is a clean-room 3D modeling and sculpting app. Behaviour may be learned from the reference
apps' public manuals, videos and observed behaviour; never read, disassemble or copy their code. The
same holds for GPL 3D tools and libraries (Blender, including its sculpting and mesh-editing code,
Wings3D, MeshLab and their libraries): do not read their source. Algorithms come from public papers
and textbooks (half-edge meshes, Catmull–Clark subdivision, Möller–Trumbore, BVHs). File formats come
from public specifications (glTF 2.0, Wavefront OBJ, STL, PLY, ZIP) through `craft-mesh`.
Dependencies use permissive licences only (`cargo xtask licenses`).

## 3. Names

User-visible feature names are our own (craft-contrib `standards/naming.md`): no other company's
product or feature names as labels. The list and its trademark check are in
[`docs/feature-names.md`](docs/feature-names.md); add a row before adding a label.

## 4. Interface text and languages

Every user-visible string goes through `tl!("English source")` or `tlf!("… {name} …", name = v)`
(`apps/modelcraft/src/i18n.rs`, the `craft-i18n` crate from craft-libs). The English text is the
key. Translations come from [storytold/craft-i18n](https://github.com/storytold/craft-i18n)
(`locales/<locale>/modelcraft.tsv`) through the optional build input `CRAFT_I18N_DIR`; never add
craft-i18n or craft-fonts to a `Cargo.toml`. `cargo xtask strings` lists the source texts.

## 5. Progress docs and release notes (RFC-0017)

A pull request never edits `ROADMAP.md`'s status line or `## Progress log`: it adds
`docs/progress.d/<issue>-<branch-slug>.md` with its log row, and a user-visible change adds
`packaging/release-notes.d/<issue>-<branch-slug>.md`. `cargo xtask progress` shows the joined log;
`cargo xtask ci` checks the fragments. Maintainers cut releases (craft-common-actions
`scripts/cut_release.py`); never create release branches or tags.

**Parity table.** A feature PR flips rows in `docs/target-app-parity.md` (D/P/M, size) and nothing
else there: it must not commit the generated block between the `parity:` markers. `cargo xtask ci`
validates the rows strictly but only warns when the block is stale. A maintainer refreshes it with
`cargo xtask parity --write` in a separate PR.

## 6. Engineering

- **Everything is a command.** Document changes are `modelcraft_engine::Command`s (JSON with a
  `"cmd"` tag). The app, `modelcraft-cli run/exec/control`, the crash journal and the tests use the
  same commands. A new feature adds its command and a test. See [docs/cli.md](docs/cli.md).
- **Shared libraries first.** ModelCraft uses `craft-mesh` (glTF/GLB/OBJ/STL/PLY), `craft-zip`,
  `craft-atomic`, `craft-image-codecs`, `craft-i18n` and `craft-fonts-build` from craft-libs. Gaps and
  the crates we expect to share with MaterialCraft and TextureCraft (RFC-0032) are in
  [docs/architecture.md](docs/architecture.md); never copy code across instead.
- **Layering.** `modelcraft-mesh`, `-ops`, `-scene`, `-sculpt`, `-formats`, `-engine` and `-render`
  never depend on egui, eframe, winit or rfd; `cargo xtask layers` enforces it. The app
  (`apps/modelcraft`) is the only UI.
- **GPU (RFC-0022).** wgpu 30, the version egui-wgpu uses in the suite's egui fork. Capabilities come
  from the created device; without a device the viewport falls back to a CPU wireframe and every test
  still runs. Move to `craft-gpu` when it lands in craft-libs.
- **Accessibility.** See the block above. The scene tree publishes tree items; the viewport publishes
  a node per object with its selected state; every gizmo drag has a numeric path (Transform by
  Numbers, the Properties fields); every command has a shortcut or the command palette (Ctrl+K).
- **Toolchain.** Rust ≥ 1.95 (edition 2024). egui/eframe 0.37 from the suite's fork, pinned in the
  root `Cargo.toml` `[patch.crates-io]`, the same revisions as craft-libs.
- **Gates.** `cargo xtask ci` (fmt check, clippy `-D warnings`, tests, layering, assets, licences,
  progress fragments) before every commit; GitHub CI runs the same through craft-common-actions.
- **Versioning.** `[workspace.package] version` is the only version; every crate inherits it.
- **Build output** stays in the repository's `target/`.

## See also

- [README.md](README.md), [ROADMAP.md](ROADMAP.md), [docs/gaps.md](docs/gaps.md),
  [docs/target-app-parity.md](docs/target-app-parity.md), [docs/roadmap.md](docs/roadmap.md)
- [docs/architecture.md](docs/architecture.md): crates, data model, evaluation, rendering
- [docs/file-format.md](docs/file-format.md): the `.modelcraft` project and its journal
- [docs/cli.md](docs/cli.md): `modelcraft-cli`, the command reference and the control channel
- [docs/performance.md](docs/performance.md): measured viewport and sculpt numbers
- [docs/localization-parity.md](docs/localization-parity.md), [docs/feature-names.md](docs/feature-names.md)
