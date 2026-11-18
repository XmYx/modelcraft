// Scatter: drops copies of a primitive at seeded random places on a square, all under one group.
// Arguments: count (40), area (10.0), seed (7), kind ("sphere"). The same seed gives the same scatter.
let count = arg("count", 40);
let area = arg("area", 10.0);
let seed = arg("seed", 7);
let kind = arg("kind", "sphere");

let dice = rng(seed);
let root = ();
undo_group("Scatter", || {
    root = add_group("Scatter " + seed);
    for i in 0..count {
        let id = add_primitive(kind, kind + " " + i);
        let size = dice.range(0.05, 0.4);
        scale_to(id, size, size, size);
        move_to(id, dice.range(-area / 2.0, area / 2.0), size, dice.range(-area / 2.0, area / 2.0));
        reparent(id, root);
    }
});
print(`scattered ${count} ${kind} objects under ${name_of(root)}`);
root
