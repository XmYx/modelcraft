# Feature names

User-visible labels are our own (craft-contrib `standards/naming.md`, RFC-0029). Each distinctive
label was checked on 2026-11-17 against the reference products' terminology and a web search of
trademark registers (USPTO TESS/EUIPO eSearch by term, class 9 software) for live marks covering
3D software. "Generic" means a common technical English term with no single owner. Unclear means no:
such a label is renamed. Internal ids (command names, file keys) may differ from labels.

| Label | Instead of a reference term for | Check | Result |
|---|---|---|---|
| Smooth Cage | a subdivision-surface generator object | No live software mark found; plain descriptive words | Use |
| Build History | construction history | Generic words; "construction history" avoided | Use |
| Scene (panel) | the object manager / outliner | Generic | Use |
| Properties (panel) | the attribute manager / channel box | Generic | Use |
| Patches | polygon groups / face sets | Generic geometry term in this sense; product terms avoided | Use |
| Stamp | brush alpha | Generic | Use |
| Detail levels | multiresolution / subdivision levels in sculpting | Generic words | Use |
| Shape layers | sculpt layers / 3D layers with an intensity | Generic words; the reference product's layer names avoided | Use |
| Live detail | dynamic topology | No live software mark found | Use |
| Even remesh | voxel remeshing products | Generic words; product names avoided | Use |
| Quad sphere | cube-mapped sphere primitive | Generic | Use |
| Round (deformer) | spherify | Generic | Use |
| Ripple (deformer) | wave / sine deformer | Generic | Use |
| Transform by Numbers | numeric transform dialogs | Generic phrase | Use |
| Four Views | quad view | Generic | Use |
| Command Palette | command search | Generic (widely used) | Use |
| Undo History (panel) | history/undo list panels | Generic | Use |
| Save Incremental | versioned save commands | Generic description | Use |
| Snap, Toggle Snapping | snapping | Generic | Use |
| Select Edge Loop, Select Edge Ring, Grow Selection, Shrink Selection | selection tools | Generic modeling terms | Use |
| Bridge Faces, Fill Hole, Mirror X/Y/Z | polygon tools | Generic modeling terms | Use |
| Keyboard Shortcuts (dialog) | shortcut editors | Generic; the dialog is the suite's shared craft-keymap one | Use |
| Render View, Update while editing | interactive / progressive render windows (the references use product-specific names for theirs) | Generic words; no live software mark found for the phrase in class 9; the references' own names avoided | Use |
| Studio, Daylight, Panorama image (Render View lighting) | dome lights, physical sun-and-sky systems, HDRI lighting | Generic words; the references' product names for their skies and domes avoided | Use |
| Mask, Draw, Clay, Smooth, Inflate, Grab, Pinch, Flatten, Crease | sculpt brushes | Generic sculpting verbs used across tools; no product-specific brush names (with trademarks or prefixes) used | Use |
| Bend, Twist, Taper | deformers | Generic | Use |
| Extrude, Inset, Bevel, Loop Cut, Merge, Weld, Subdivide, Triangulate | polygon tools | Generic modeling terms | Use |
| Model / Sculpt / Paint (workspaces) | workspace layouts | Generic | Use |

Names deliberately **not** used as labels: the reference products' names and their branded features
(their remesher, sculpt-mesh, layer and brush product names, object/attribute manager names,
"construction history", "channel box", "subdivision surface" object name, "polygroups", "alphas",
"dyntopo", "multires"). The parity document names the products only to describe the comparison.
