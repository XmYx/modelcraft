// Staircase: one mesh object for a straight staircase made of solid steps.
// Arguments: steps (12), rise (0.18), run (0.28), width (1.2).
let steps = arg("steps", 12);
let rise = arg("rise", 0.18);
let run = arg("run", 0.28);
let width = arg("width", 1.2);

if steps < 1 || steps > 200 { throw "steps must be between 1 and 200"; }

let b = mesh_builder();
for i in 0..steps {
    // Step i is a block from the floor up to its tread, one run deeper than the step before it.
    b.add_box([0.0, 0.0, run * i], [width, rise * (i + 1), run * (i + 1)]);
}
let id = b.build("Staircase");
print(`staircase: ${steps} steps, ${steps * rise} high, ${steps * run} long`);
id
