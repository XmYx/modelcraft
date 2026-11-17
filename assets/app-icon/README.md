# ModelCraft app icon

`modelcraft.svg` is the canonical icon (a faceted owl carved from a polygon cage) in the suite's
three-colour style: Ink `#0b0b0c`, Paper `#f2f1ee` and the ModelCraft colour `#8a6cf0`, on a 512 tile
with `rx=112` and the art clipped at `rx=106`. It is a contributor draft; the ArtCraft team may
replace it with an engraved-creature icon like the other Crafting Apps'.

Every other file here is rendered from it by `cargo xtask icons` (craft-svg's bounded renderer and
craft-image-codecs) and committed so packaging needs no tools: `hicolor/{16…512}/apps/*.png`,
`hicolor/scalable/apps/ai.storyteller.modelcraft.svg`, `modelcraft-1024.png`, `modelcraft.ico` and
`modelcraft.icns`. After changing the SVG, run `cargo xtask icons` and commit the results.
