// Distribute: spaces the selected objects evenly along an axis, keeping the first and last where they are.
// Arguments: axis ("x", "y" or "z"; default "x").
let axis = arg("axis", "x");
let which = if axis == "x" { 0 } else if axis == "y" { 1 } else if axis == "z" { 2 } else { throw "axis must be x, y or z" };

let ids = selection();
if ids.len() < 3 { throw "select at least three objects"; }

// Order by their position along the axis, then place the ones in between at equal steps.
ids.sort(|a, b| {
    let pa = get_translation(a)[which];
    let pb = get_translation(b)[which];
    if pa < pb { -1 } else if pa > pb { 1 } else { 0 }
});
let first = get_translation(ids[0])[which];
let last = get_translation(ids[ids.len() - 1])[which];
let gap = (last - first) / (ids.len() - 1);
undo_group("Distribute", || {
    for i in 1..(ids.len() - 1) {
        let t = get_translation(ids[i]);
        t[which] = first + gap * i;
        move_to(ids[i], t[0], t[1], t[2]);
    }
});
print(`distributed ${ids.len()} objects along ${axis}, ${gap} apart`);
ids.len()
