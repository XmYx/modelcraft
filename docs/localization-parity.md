# Localization parity

> **Last reviewed:** 2026-11-17 · **Last updated:** 2026-11-17 · **Change:** major (first version)

| | Value |
|---|---|
| Interface source texts (`tl!`/`tlf!`) | 264 |
| Already in craft-i18n's shared `common.tsv` | 75 |
| ModelCraft's own catalog (`locales/<locale>/modelcraft.tsv` in craft-i18n) | not yet created |
| Languages offered | English; the 14 left-to-right languages in `apps/modelcraft/src/i18n.rs` as soon as craft-i18n has `modelcraft.tsv` for them |

Every user-visible string goes through `tl!`/`tlf!` with the English text as the key.
`cargo xtask strings <craft-i18n checkout>` writes the list to
[`docs/i18n/modelcraft-strings.tsv`](i18n/modelcraft-strings.tsv), marking the shared ones. Labels
built from data (object type names, step names, primitive kinds) also go through `tl!` with their
English name; the strings tool lists only literal texts, so those are added to the catalog by hand:
Group, Mesh, Smooth cage, Deformer, Extrude, Inset, Bevel, Loop cut, Merge, Weld, Delete faces,
Delete edges, Delete vertices, Move components, Flip normals, Triangulate, Subdivide, Set patch,
Sculpt layer.

Builds embed catalogs only when `CRAFT_I18N_DIR` points at a craft-i18n checkout
(`CRAFT_I18N_REQUIRED=1` for releases), following craft-i18n `docs/integration.md`; without it the
app is English-only.
