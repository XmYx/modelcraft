# Contributing to ModelCraft

Read [AGENTS.md](AGENTS.md) first: the suite's house rules and ModelCraft's own rules (never crash,
clean room, assets, names, layering).

1. Rust 1.95 or newer through `rustup`.
2. `cargo run -p modelcraft -- --demo model` (or `--demo sculpt`) to try the app.
3. `cargo xtask ci` before every commit (fmt, clippy `-D warnings`, tests, layering, assets, licences).
4. Changes to `main` go through pull requests.
5. New assets go in [ATTRIBUTION.md](ATTRIBUTION.md) in the same change.
6. Screenshots for `docs/images/`: `cargo run --release -p modelcraft -- --demo model --screenshot docs/images/<name>.png`.
   Add `--screenshot-state <name>` to picture a window first (timeline, curve-editor, render-view,
   scripts, plugins, preferences, shortcuts, palette, layers, statistics, icon-palettes).
