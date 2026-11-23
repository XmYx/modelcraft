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
| Decimate, Project Detail | polygon reduction and detail projection plug-ins and commands | Generic words; the reference products' names avoided | Use |
| Rectangle Mask, Lasso Mask, Rectangle Hide, Lasso Hide (region tools); Clip, Trim by plane; Shell | selection-rectangle and lasso masking and visibility, clip and trim brushes, mesh extraction | Generic tool words; the reference product's tool names avoided | Use |
| Pull, Plateau, Buildup, Surface grab, Gouge, Slide, Bulge (brushes) | the reference sculpting app's hook, layer, clay build-up, topological move, groove, nudge and blob brushes | Plain verbs and nouns; the reference brush names avoided | Use |
| Trim, Fill, Polish (brushes) | planar trim, fill and polish brushes | Generic tool words | Use |
| Rotate, Elastic move, Scrape (brushes); Radial copies | twist, elastic deform and scrape brushes; radial symmetry | Generic words | Use |
| Hardness | brush focal shift | Generic | Use |
| Spaced, Dots, Spray, Trail, Drag out (stroke modes) | freehand, dots, spray, lazy-mouse and drag-rectangle strokes | Generic words; "lazy" naming avoided | Use |
| Shoulder, Needle (falloff curves) | curve presets | Generic | Use |
| Quad sphere | cube-mapped sphere primitive | Generic | Use |
| Disc, Polygon, Tube, Capsule, Pyramid, Tetrahedron, Octahedron, Dodecahedron, Icosahedron, Landscape (primitives); Hole radius, Feature scale | disc, n-gon, tube, capsule, pyramid, platonic solid and landscape primitives | Geometry words and plain nouns | Use |
| Bulge, Squash, Smooth, Delta Smooth, Displace, Shrink Wrap, Formula (deformers); Reach, Fit to parent, Within box; Move along | bulge, squash-and-stretch, smoothing, delta smoothing, displacement, shrink-wrap and formula deformers; limited / unlimited / within-box modes; the formula deformer's direction | Generic geometry words; Reach describes how far the effect goes | Use |
| Radial, Cylinder, Cone, Capsule, Torus, Random, Formula (falloff fields); Falloff layers, Switched on, Opacity | radial, cylinder, cone, capsule, torus, random and formula fields; the field-layer list with its enable switch and opacity | Geometry words and plain descriptions; the formula field uses the expression language of the animation crate | Use |
| Connect, Thicken, Reduce, Boolean (generators); Union, Subtract, Intersect; Join distance, Thickness, Grow, Keep, Operation | joining, solidify, polygon reduction and boolean generators | Plain geometry words; the reference product's generator names are not used as labels beyond these generic verbs | Use |
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
| Place Joints by Clicking, Paint Skin Weights, Weight Brush, Solo Layer, Lock Layer, Key Layer Weight, Move Layer Up / Down, Copy / Paste Keys, Select All Shown Keys, Show Ghosts, Zoom Values In / Out, Fit Values | joint tool, weight painting, layer solo / lock / weight keys, key clipboard, onion-skin ghosts, graph value zoom | Generic terms and plain descriptions | Use |
| Volume-Preserving Skin | dual-quaternion skinning | Our own description of what the option does; the method's name appears in the docs only | Use |
| Animation layer, Base layer, Add Layer, Layer weight, Mute Layer, Layer Replaces, Flatten Layers | animation layers (additive / override, merge) | Generic terms | Use |
| Motion Paths, Turntable, Smooth Rotation Keys | motion trails, turntable animation, Euler filter | Generic descriptions; "Euler filter" is the mathematician's name and appears only in the parity map and the command id | Use |
| Key Transform, Key Attribute, Auto-Key, Shift Keys, Scale Keys | set-key and auto-keyframe commands | Generic animation terms | Use |
| Stepped, Linear, Curved (interpolation); Automatic, Flat, Free, Split (tangents); Hold, Continue, Repeat, Repeat with offset, Back and forth (outside the keys) | key interpolation, tangent types, pre/post infinity | Plain words; no product's tangent-type names ("spline", "clamped", "plateau") used as labels | Use |
| Keyboard Shortcuts (dialog) | shortcut editors | Generic; the dialog is the suite's shared craft-keymap one | Use |
| Render View, Update while editing | interactive / progressive render windows (the references use product-specific names for theirs) | Generic words; no live software mark found for the phrase in class 9; the references' own names avoided | Use |
| Learn where light comes from (Render View) | path guiding, light cache or irradiance-map style options (the references use product-specific names) | A plain description of what it does; no product name used | Use |
| Resample direct light (Render View) | reservoir-based many-light options (the references use product-specific names for theirs) | A plain description of what it does; "ReSTIR" appears only in engineering notes as the name of the published technique, not in the interface | Use |
| Denoise (Render View, `--denoise`) | denoiser options (the references name theirs after vendors) | A plain verb; the filters are named after their published papers in engineering notes only | Use |
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
| Array, Symmetry, Shear | array / symmetry generators and the shear deformer | Generic modeling words | Use |
| Falloff Field; Remap, Curve, Slow start, Slow finish, Combine with earlier fields | fields and field layers | Generic geometry and animation words; the reference's field and layer names are not used | Use |
| Clones, Clone generator; layouts Linear, Radial, Grid, Scatter, Surface, Vertices | the cloner object and its modes | Plain descriptive words. The reference's object name, its motion-graphics brand and "honeycomb" are not used as labels (the internal command id `add_cloner` is not a label) | Use |
| Simulate (menu), Simulation (panel), Bake, Cancel, Play Cache | the simulation menu and panel | Generic words; the references' dynamics and cache menus are not used as labels | Use |
| Particles, Emitter, Rate, Burst, Lifetime, Spread, Attractor, Vortex, Drag | particle systems | Generic words; no reference product's particle module, emitter or force names | Use |
| Cloth, Pin, Stretch, Shear, Bend, Thickness, Self Collision, Wind, Turbulence | cloth simulation settings | Generic physical terms; no reference product's cloth module or setting names | Use |
| Simulation, Cache, Bake, Clear Cache, Start Frame, Substeps, Seed | simulation caches and bake commands | Generic words; the references' dynamics and cache product names are not used as labels | Use |
| Hair, Guides, Strands, Density, Clump, Frizz, Comb, Cut, Length, Puff, Smooth (hair tools) | the references' curve-hair and fur systems, their guide and brush names | Plain English words; no reference product, module or brush name (with trademarks or prefixes) is used | Use |
| Clone Modifier; modes Plain, Random, Noise, Step, Delay, Target, Formula; Move, Turn, Grow, Tint, Strength | effectors and their parameter tabs | "Effector" is the reference's term, so it is not used; "Shader" is called Noise because that is what the mode is, and "Time" is covered by Delay | Use |

Names deliberately **not** used as labels: the reference products' names and their branded features
(their remesher, sculpt-mesh, layer and brush product names, object/attribute manager names,
"construction history", "channel box", "subdivision surface" object name, "polygroups", "alphas",
"dyntopo", "multires"). The parity document names the products only to describe the comparison.
| Radial Menu | pie / marking menus | Generic ("radial menu" is a common UI pattern name); the reference term avoided | Use |
| Quick Bar | shelves / custom palettes | No live software mark found for this phrase in class 9; plain descriptive words | Use |
| Preferences | settings dialogs | Generic | Use |
| Lock, Unlock | protecting objects from edits | Generic | Use |
| Reference Only | template objects | Generic words; the reference term avoided | Use |
| Selection Sets | named selections / quick selection sets | Generic | Use |
| Copy Attributes, Paste Attributes | attribute copy tools | Generic | Use |
| Search objects | outliner filters | Generic | Use |
| Tool Options (window), Slide, Spacing curve, Rows, Variation, Caps, Lift, Segments, Profile (Flat, Round, Custom), Bevel Vertices, Extrude Edges | per-tool settings pages and bevel/extrude/loop-cut parameters in the reference products | Plain modeling words; the reference products' option names avoided | Use |
| Knife, Knife by Numbers, Slice by Plane, Connect Vertices, Polygon Pen, Polygon by Numbers | knife / line cut, plane cut, connect, create-polygon tools | Plain verbs and nouns; the reference products' tool names avoided | Use |
| Point Light, Spot Light, Area Light, Sun Light, Dome Light; Camera; Look Through; Render Through This Camera | light and camera objects, viewing through a camera, the camera a render uses | Generic lighting and photography words; the reference products' object names avoided | Use |
| Render Settings, Picture Viewer, Frame Queue, Passes, Region | render settings and render queue windows, the picture viewer, multi-pass output, render region | Generic words for what they do; the reference products' window names avoided | Use |
| Bake Maps; Cage distance, Ray distance; Cavity | map baking, bake cages, cavity maps | Generic words for what they do; the reference products' bake dialog and map names avoided | Use |
| Spline; Circle, Rectangle, Star, N-Side, Arc, Helix (spline kinds); Intermediate points, Fixed steps, Uniform length, Adaptive, Tolerance, Largest turn (tessellation) | spline objects and their primitives | Plain geometry words; the references' "Points", "Intermediate points" and primitive names are generic nouns, no product term (such as their object type name) is used as a label | Use |
| Pen Tool; Point, Handle in, Handle out, Handles (Free, Aligned, Mirrored, Smooth), Curve type (Polyline, Bezier, B-spline), Closed, Add Point, Insert Point, Delete Points | spline drawing and point editing | Plain drawing words; the references' pen and tangent terms are generic craft terms, no product tool name is used | Use |
| Extrude, Revolve, Loft, Sweep (spline generators); Cap at the start, Cap at the end, Scale at the end, Twist, Roll at the start, Smooth through the curves | surfaces from splines | Plain geometry verbs; the references' own object names for lathing ("Lathe") and their NURBS variants are not used as labels | Use |
| Reverse Spline, Chamfer Points, Fillet Points, Corner size, Convert Spline to Polygon, Convert Spline to Mesh | spline operations | Plain geometry verbs (chamfer and fillet are standard drafting terms) | Use |
| Scene Nodes; Add Node…, Add a node, Search nodes, Reset View; Parent, Children (ports) | node editors and their node browsers | Plain words for a window of nodes and its search; the references' node editor and browser product names are not used as labels | Use |
