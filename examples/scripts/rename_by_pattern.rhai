// Rename by pattern: gives every object whose name matches a pattern a numbered new name.
// Arguments: pattern ("*", where * is any run of characters), name ("Object_#", # becomes 01, 02, ...).
let pattern = arg("pattern", "*");
let template = arg("name", "Object_#");

let hits = find_all(pattern);
let width = if hits.len() >= 100 { 3 } else { 2 };
undo_group("Rename by pattern", || {
    let n = 1;
    for id in hits {
        let number = "" + n;
        while number.len() < width { number = "0" + number; }
        let new_name = template;
        new_name.replace("#", number);
        rename(id, new_name);
        n += 1;
    }
});
print(`renamed ${hits.len()} objects`);
hits.len()
