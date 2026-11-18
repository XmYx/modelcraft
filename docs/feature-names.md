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
| Pull, Plateau, Buildup, Surface grab, Gouge, Slide, Bulge (brushes) | the reference sculpting app's hook, layer, clay build-up, topological move, groove, nudge and blob brushes | Plain verbs and nouns; the reference brush names avoided | Use |
| Trim, Fill, Polish (brushes) | planar trim, fill and polish brushes | Generic tool words | Use |
| Hardness | brush focal shift | Generic | Use |
| Spaced, Dots, Spray, Trail, Drag out (stroke modes) | freehand, dots, spray, lazy-mouse and drag-rectangle strokes | Generic words; "lazy" naming avoided | Use |
| Shoulder, Needle (falloff curves) | curve presets | Generic | Use |
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
| Animation (panel), Key Sheet, Curve Editor | timeline, dope sheet and graph/function-curve editors | "Key sheet" and "curve editor" are generic descriptions; the reference apps' editor names are not used | Use |
| Joint, Add Joint, Orient Joints, Mirror Joints, Skin, Bind Skin, Unbind Skin, Normalize / Prune / Mirror Skin Weights | joints, skin binding and weight tools | Generic rigging terms | Use |
| Add IK to Joint Chain, IK blend, Match FK to IK, Match IK to FK, Constrain Position / Rotation / Scale, Constrain as Child, Constrain to Look At, Bake Constraints to Keys, Driven key, Formula | inverse kinematics, FK/IK switching, point / orient / scale / parent / aim constraints, baking, driven keys and expressions | Generic rigging terms and plain descriptions of what each constraint does; the solver names (Two bones, Reaching, Joint by joint) describe the methods, which the docs cite by their paper names | Use |
| Blend shapes, In-between shape, Lattice, Control point, Cell by cell, Cluster, Move Deformer Earlier / Later | morph targets with in-betweens, free-form deformation (Bézier volume or trilinear), cluster deformer, deformer stack order | Generic terms; "Cell by cell" describes the trilinear mode | Use |
| Volume-Preserving Skin | dual-quaternion skinning | Our own description of what the option does; the method's name appears in the docs only | Use |
| Animation layer, Base layer, Add Layer, Layer weight, Mute Layer, Layer Replaces, Flatten Layers | animation layers (additive / override, merge) | Generic terms | Use |
| Motion Paths, Turntable, Smooth Rotation Keys | motion trails, turntable animation, Euler filter | Generic descriptions; "Euler filter" is the mathematician's name and appears only in the parity map and the command id | Use |
| Key Transform, Key Attribute, Auto-Key, Shift Keys, Scale Keys | set-key and auto-keyframe commands | Generic animation terms | Use |
| Stepped, Linear, Curved (interpolation); Automatic, Flat, Free, Split (tangents); Hold, Continue, Repeat, Repeat with offset, Back and forth (outside the keys) | key interpolation, tangent types, pre/post infinity | Plain words; no product's tangent-type names ("spline", "clamped", "plateau") used as labels | Use |
| Keyboard Shortcuts (dialog) | shortcut editors | Generic; the dialog is the suite's shared craft-keymap one | Use |
| Render View, Update while editing | interactive / progressive render windows (the references use product-specific names for theirs) | Generic words; no live software mark found for the phrase in class 9; the references' own names avoided | Use |
| Studio, Daylight, Panorama image (Render View lighting) | dome lights, physical sun-and-sky systems, HDRI lighting | Generic words; the references' product names for their skies and domes avoided | Use |
| Mask, Draw, Clay, Smooth, Inflate, Grab, Pinch, Flatten, Crease | sculpt brushes | Generic sculpting verbs used across tools; no product-specific brush names (with trademarks or prefixes) used | Use |
| Bend, Twist, Taper | deformers | Generic | Use |
| Extrude, Inset, Bevel, Loop Cut, Merge, Weld, Subdivide, Triangulate | polygon tools | Generic modeling terms | Use |
| Scripts (window and menu), Run Script, Run Selection, Output, Quick Scripts | script editors, script consoles and shelves of the references (their product names for them are avoided) | Generic words. A web search (2026-11-17) found no 3D software mark for "Quick Scripts"; the USPTO register itself was not queried, so treat this row as unconfirmed until a maintainer checks it | Use, pending that check |
| Export UV Layout | UV snapshot / UV template export | Generic description; product terms avoided | Use |
| Seams, Mark Seam, Clear Seam, Unwrap UVs, Pin | UV cutting and flattening tools | Generic modeling terms; no product-specific tool names used | Use |
| Pack UV Islands, Project UVs, Planar, Box, Cylindrical, Spherical, Camera (projections) | UV layout and projection tools | Generic geometry terms; no product-specific tool names used | Use |
| UV Editor, UV Vertex / Edge / Face / Island, Weld, Pin, Heat map | UV editor tools | Generic terms; product tool names avoided | Use |
| Model / Sculpt / Paint (workspaces) | workspace layouts | Generic | Use |
| Falloff Field; Remap, Curve, Slow start, Slow finish, Combine with earlier fields | fields and field layers | Generic geometry and animation words; the reference's field and layer names are not used | Use |
| Clones, Clone generator; layouts Linear, Radial, Grid, Scatter, Surface, Vertices | the cloner object and its modes | Plain descriptive words. The reference's object name, its motion-graphics brand and "honeycomb" are not used as labels (the internal command id `add_cloner` is not a label) | Use |
| Hair, Guides, Strands, Density, Clump, Frizz, Comb, Cut, Length, Puff, Smooth (hair tools) | the references' curve-hair and fur systems, their guide and brush names | Plain English words; no reference product, module or brush name (with trademarks or prefixes) is used | Use |
| Clone modifiers (planned) | effectors | "Effector" is the reference's term, so it is not used | Use |

Names deliberately **not** used as labels: the reference products' names and their branded features
(their remesher, sculpt-mesh, layer and brush product names, object/attribute manager names,
"construction history", "channel box", "subdivision surface" object name, "polygroups", "alphas",
"dyntopo", "multires"). The parity document names the products only to describe the comparison.
| Radial Menu | pie / marking menus | Generic ("radial menu" is a common UI pattern name); the reference term avoided | Use |
| Quick Bar | shelves / custom palettes | No live software mark found for this phrase in class 9; plain descriptive words | Use |
| Preferences | settings dialogs | Generic | Use |
