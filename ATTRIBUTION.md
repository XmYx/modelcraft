# Attribution

Every non-code asset in ModelCraft, with its author, source and licence. The rules are in
[AGENTS.md](AGENTS.md): no artwork, meshes, brushes or stamps from commercial 3D tools, open licences
only, every asset listed here in the same change. `cargo xtask assets` checks that every file under
`assets/`, `docs/images/` and `docs/brand/` is listed. ModelCraft contains no assets from Maxon,
Autodesk or any other 3D software vendor.

## In this repository

| Asset | Author | Source | Licence | Used for |
|---|---|---|---|---|
| `assets/app-icon/modelcraft.svg` | ModelCraft contributors | Original work: draft app icon (faceted owl in a polygon cage) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) | App icon, README |
| `assets/app-icon/hicolor/scalable/apps/ai.storyteller.modelcraft.svg` | ModelCraft contributors | Copy of `assets/app-icon/modelcraft.svg` under the Linux icon-theme name | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) | Linux desktop icon |
| `assets/app-icon/hicolor/16x16/apps/ai.storyteller.modelcraft.png` | ModelCraft contributors | Rendered from `assets/app-icon/modelcraft.svg` by `cargo xtask icons` (16 px) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) | Linux icon theme, window icon |
| `assets/app-icon/hicolor/24x24/apps/ai.storyteller.modelcraft.png` | ModelCraft contributors | Rendered from `assets/app-icon/modelcraft.svg` by `cargo xtask icons` (24 px) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) | Linux icon theme, window icon |
| `assets/app-icon/hicolor/32x32/apps/ai.storyteller.modelcraft.png` | ModelCraft contributors | Rendered from `assets/app-icon/modelcraft.svg` by `cargo xtask icons` (32 px) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) | Linux icon theme, window icon |
| `assets/app-icon/hicolor/48x48/apps/ai.storyteller.modelcraft.png` | ModelCraft contributors | Rendered from `assets/app-icon/modelcraft.svg` by `cargo xtask icons` (48 px) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) | Linux icon theme, window icon |
| `assets/app-icon/hicolor/64x64/apps/ai.storyteller.modelcraft.png` | ModelCraft contributors | Rendered from `assets/app-icon/modelcraft.svg` by `cargo xtask icons` (64 px) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) | Linux icon theme, window icon |
| `assets/app-icon/hicolor/128x128/apps/ai.storyteller.modelcraft.png` | ModelCraft contributors | Rendered from `assets/app-icon/modelcraft.svg` by `cargo xtask icons` (128 px) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) | Linux icon theme, window icon |
| `assets/app-icon/hicolor/256x256/apps/ai.storyteller.modelcraft.png` | ModelCraft contributors | Rendered from `assets/app-icon/modelcraft.svg` by `cargo xtask icons` (256 px) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) | Linux icon theme, window icon |
| `assets/app-icon/hicolor/512x512/apps/ai.storyteller.modelcraft.png` | ModelCraft contributors | Rendered from `assets/app-icon/modelcraft.svg` by `cargo xtask icons` (512 px) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) | Linux icon theme, window icon |
| `assets/app-icon/modelcraft-1024.png` | ModelCraft contributors | Rendered from `assets/app-icon/modelcraft.svg` by `cargo xtask icons` (1024 px) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) | Packaging |
| `assets/app-icon/modelcraft.ico` | ModelCraft contributors | Rendered from `assets/app-icon/modelcraft.svg` by `cargo xtask icons` (Windows icon, PNG entries 16–256 px) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) | Packaging |
| `assets/app-icon/modelcraft.icns` | ModelCraft contributors | Rendered from `assets/app-icon/modelcraft.svg` by `cargo xtask icons` (macOS icon, PNG entries 16–1024 px) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) | Packaging |
| `docs/images/modelcraft-model.png` | ModelCraft contributors | Original work: screenshot (`--demo model --screenshot`) of the built-in modeling demo, built in code in `apps/modelcraft/src/demo.rs` | MIT OR Apache-2.0 | README, docs |
| `docs/images/modelcraft-quad.png` | ModelCraft contributors | Original work: screenshot (`--demo model --quad --screenshot`) of the built-in modeling demo in four views | MIT OR Apache-2.0 | README, docs |
| `docs/images/modelcraft-sculpt.png` | ModelCraft contributors | Original work: screenshot (`--demo sculpt --screenshot`) of the built-in sculpting demo, strokes scripted in `apps/modelcraft/src/demo.rs` | MIT OR Apache-2.0 | README, docs |

## Drawn in code

| Asset | Author | Source | Licence | Used for |
|---|---|---|---|---|
| Brush stamp pictures: Dots, Scales, Cracks, Weave, Pores, Star, Stripes, Honeycomb | ModelCraft contributors | Original work: computed at start-up by `crates/sculpt/src/alpha.rs`; no image files | MIT OR Apache-2.0 | Sculpt brush stamps |

## Bundled through dependencies

| Asset | Author | Source | Licence | Used for |
|---|---|---|---|---|
| Ubuntu Light, Hack, Noto Emoji, emoji-icon-font | Canonical; Source Foundry; Google; Emil Ernerfeldt | egui's `epaint_default_fonts` | Ubuntu Font Licence 1.0; MIT (DejaVu parts public domain); OFL-1.1; MIT | Interface text |

## Embedded at build time from craft-fonts (`CRAFT_FONTS_DIR`)

Release builds embed the faces listed in craft-fonts' `fonts/manifest.txt`; their authors and
licences are in that repository's ATTRIBUTION.md, and packaging copies each `OFL.txt`.
