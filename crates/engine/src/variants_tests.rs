//! Scene variants: overrides applied and taken back exactly, edits captured, undo across
//! switches, saving, JSON, limits and hostile files.

use modelcraft_scene::attr;
use modelcraft_scene::variants::{MAX_OVERRIDES, MAX_VARIANTS};
use modelcraft_scene::{ObjectId, Scene};

use crate::variants::VariantCommand as V;
use crate::{AnimCommand, Command, Document};

fn cube(d: &mut Document) -> ObjectId {
    d.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0]
}

fn add(d: &mut Document, name: &str) -> u32 {
    d.execute(Command::Variant(V::Add { name: Some(name.into()) })).unwrap();
    d.scene.variants.list.last().unwrap().id
}

fn switch(d: &mut Document, id: Option<u32>) {
    d.execute(Command::Variant(V::Switch { id })).unwrap();
}

fn over(d: &mut Document, object: ObjectId, paths: &[&str]) {
    d.execute(Command::Variant(V::Override { object, paths: paths.iter().map(|p| p.to_string()).collect() })).unwrap();
}

fn set(d: &mut Document, id: ObjectId, path: &str, v: f64) {
    d.execute(Command::Anim(AnimCommand::SetAttr { id, path: path.into(), value: v })).unwrap();
}

fn value(d: &Document, id: ObjectId, path: &str) -> f64 {
    d.scene.attr(id, path).unwrap()
}

/// Every numeric attribute of every object: what "the main data" is.
fn numbers(s: &Scene) -> Vec<(ObjectId, Vec<(String, f64)>)> {
    s.objects().iter().map(|o| (o.id, attr::list(o))).collect()
}

#[test]
fn a_variant_overrides_chosen_attributes_and_the_main_scene_comes_back_exactly() {
    let mut d = Document::new();
    let a = cube(&mut d);
    let b = cube(&mut d);
    let main = numbers(&d.scene);
    let red = add(&mut d, "Red and moved");
    switch(&mut d, Some(red));
    over(&mut d, a, &["translate.x"]);
    over(&mut d, b, &["material.metallic"]);
    set(&mut d, a, "translate.x", 5.0);
    set(&mut d, b, "material.metallic", 0.5);
    assert_eq!(value(&d, a, "translate.x"), 5.0);
    switch(&mut d, None);
    assert_eq!(numbers(&d.scene), main, "switching to main gives the main data back, every number");
    switch(&mut d, Some(red));
    assert_eq!((value(&d, a, "translate.x"), value(&d, b, "material.metallic")), (5.0, 0.5));
    assert_eq!(d.scene.variants.stash.len(), 2);
    switch(&mut d, None);
    assert_eq!(numbers(&d.scene), main);
    assert!(d.scene.variants.stash.is_empty() && d.scene.variants.active.is_none());
}

#[test]
fn two_variants_do_not_leak_into_each_other() {
    let mut d = Document::new();
    let a = cube(&mut d);
    let one = add(&mut d, "One");
    let two = add(&mut d, "Two");
    switch(&mut d, Some(one));
    over(&mut d, a, &["translate.y", "visible"]);
    set(&mut d, a, "translate.y", 1.0);
    set(&mut d, a, "visible", 0.0);
    switch(&mut d, Some(two));
    assert_eq!((value(&d, a, "translate.y"), value(&d, a, "visible")), (0.0, 1.0), "variant two starts from the main values");
    over(&mut d, a, &["translate.y"]);
    set(&mut d, a, "translate.y", 3.0);
    switch(&mut d, Some(one));
    assert_eq!((value(&d, a, "translate.y"), value(&d, a, "visible")), (1.0, 0.0));
    switch(&mut d, Some(two));
    assert_eq!((value(&d, a, "translate.y"), value(&d, a, "visible")), (3.0, 1.0));
    switch(&mut d, None);
    assert_eq!((value(&d, a, "translate.y"), value(&d, a, "visible")), (0.0, 1.0));
}

#[test]
fn edits_of_overridden_attributes_belong_to_the_variant_and_other_edits_stay_in_main() {
    let mut d = Document::new();
    let a = cube(&mut d);
    let v = add(&mut d, "V");
    switch(&mut d, Some(v));
    over(&mut d, a, &["translate.x"]);
    set(&mut d, a, "translate.x", 2.0);
    set(&mut d, a, "translate.z", 4.0);
    switch(&mut d, None);
    assert_eq!(value(&d, a, "translate.x"), 0.0, "the overridden number went into the variant");
    assert_eq!(value(&d, a, "translate.z"), 4.0, "an ordinary edit is an edit of the main scene");
    switch(&mut d, Some(v));
    assert_eq!(value(&d, a, "translate.x"), 2.0);
    assert_eq!(value(&d, a, "translate.z"), 4.0);
    // Clearing an override puts the main value back.
    d.execute(Command::Variant(V::ClearOverride { object: a, paths: vec!["translate.x".into()] })).unwrap();
    assert_eq!(value(&d, a, "translate.x"), 0.0);
    assert!(d.scene.variants.stash.is_empty());
}

#[test]
fn switching_is_one_undo_step_and_undo_and_redo_cross_switches() {
    let mut d = Document::new();
    let a = cube(&mut d);
    let v = add(&mut d, "V");
    switch(&mut d, Some(v));
    over(&mut d, a, &["scale.x"]);
    set(&mut d, a, "scale.x", 3.0);
    let n = d.undo.len();
    switch(&mut d, None);
    assert_eq!(d.undo.len(), n + 1);
    assert_eq!(value(&d, a, "scale.x"), 1.0);
    d.execute(Command::Undo).unwrap();
    assert_eq!((d.scene.variants.active, value(&d, a, "scale.x")), (Some(v), 3.0), "undo of the switch brings the variant back");
    d.execute(Command::Redo).unwrap();
    assert_eq!((d.scene.variants.active, value(&d, a, "scale.x")), (None, 1.0));
    // Undo further back: the switch, the edit, the override and the first switch.
    for _ in 0..4 {
        d.execute(Command::Undo).unwrap();
    }
    assert_eq!(d.scene.variants.active, None);
    assert_eq!(value(&d, a, "scale.x"), 1.0);
}

#[test]
fn variants_are_saved_with_the_project_and_survive_opening() {
    let dir = std::env::temp_dir().join(format!("mc-variants-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("v.modelcraft");
    let mut d = Document::new();
    let a = cube(&mut d);
    let v = add(&mut d, "Night");
    switch(&mut d, Some(v));
    over(&mut d, a, &["translate.x"]);
    set(&mut d, a, "translate.x", 7.0);
    d.save_as(&path).unwrap();
    let mut e = Document::open(&path).unwrap();
    assert_eq!(e.scene.variants, d.scene.variants);
    assert_eq!(value(&e, a, "translate.x"), 7.0, "saved with the variant applied");
    switch(&mut e, None);
    assert_eq!(value(&e, a, "translate.x"), 0.0, "and the main value is still in the file");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn commands_are_json_and_a_scene_without_variants_saves_nothing_extra() {
    let c = Command::Variant(V::Override { object: ObjectId(3), paths: vec!["visible".into()] });
    let text = serde_json::to_string(&c).unwrap();
    assert_eq!(text, "{\"cmd\":\"variant\",\"op\":\"override\",\"object\":3,\"paths\":[\"visible\"]}");
    assert_eq!(serde_json::from_str::<Command>(&text).unwrap(), c);
    let s: Scene = serde_json::from_str("{\"objects\":[]}").unwrap();
    assert!(s.variants.is_empty());
    assert!(!serde_json::to_string(&s).unwrap().contains("variants"));
    assert!(crate::variants::COMMANDS.iter().all(|(n, _)| !n.is_empty()));
}

#[test]
fn bad_requests_are_refused_without_changing_anything() {
    let mut d = Document::new();
    let a = cube(&mut d);
    let v = add(&mut d, "V");
    let before = numbers(&d.scene);
    // The main scene has no overrides to add.
    assert!(d.execute(Command::Variant(V::Override { object: a, paths: vec!["translate.x".into()] })).is_err());
    assert!(d.execute(Command::Variant(V::Switch { id: Some(99) })).is_err());
    switch(&mut d, Some(v));
    assert!(d.execute(Command::Variant(V::Override { object: a, paths: vec!["nope.x".into()] })).is_err());
    assert!(d.execute(Command::Variant(V::Override { object: ObjectId(99), paths: vec!["visible".into()] })).is_err());
    assert!(d.execute(Command::Variant(V::ClearOverride { object: a, paths: vec!["visible".into()] })).is_err());
    assert!(d.execute(Command::Variant(V::Rename { id: 99, name: "x".into() })).is_err());
    // An animated attribute cannot be overridden.
    d.execute(Command::Anim(AnimCommand::SetKey { id: a, path: "translate.y".into(), frame: Some(0.0), value: Some(1.0) })).unwrap();
    assert!(d.execute(Command::Variant(V::Override { object: a, paths: vec!["translate.y".into()] })).is_err());
    switch(&mut d, None);
    assert_eq!(numbers(&d.scene).len(), before.len());
    // Names are cleaned.
    d.execute(Command::Variant(V::Rename { id: v, name: "  Day\u{7}  ".into() })).unwrap();
    assert_eq!(d.scene.variants.get(v).unwrap().name, "Day");
    d.execute(Command::Variant(V::Rename { id: v, name: "   ".into() })).unwrap();
    assert_eq!(d.scene.variants.get(v).unwrap().name, "Day", "an empty name keeps the old one");
}

#[test]
fn removing_the_active_variant_goes_back_to_main_and_deleting_an_object_is_harmless() {
    let mut d = Document::new();
    let a = cube(&mut d);
    let b = cube(&mut d);
    let v = add(&mut d, "V");
    switch(&mut d, Some(v));
    over(&mut d, a, &["translate.x"]);
    over(&mut d, b, &["translate.x"]);
    set(&mut d, a, "translate.x", 2.0);
    set(&mut d, b, "translate.x", 3.0);
    d.execute(Command::Delete { ids: vec![b] }).unwrap();
    switch(&mut d, None);
    assert_eq!(value(&d, a, "translate.x"), 0.0);
    switch(&mut d, Some(v));
    assert_eq!(value(&d, a, "translate.x"), 2.0);
    d.execute(Command::Variant(V::Remove { id: v })).unwrap();
    assert_eq!((d.scene.variants.active, value(&d, a, "translate.x")), (None, 0.0));
    assert!(d.scene.variants.list.is_empty());
}

#[test]
fn the_counts_are_bounded_and_a_hostile_file_is_repaired() {
    let mut d = Document::new();
    for i in 0..MAX_VARIANTS {
        d.scene.add_variant(&format!("v{i}")).unwrap();
    }
    assert!(d.scene.add_variant("one too many").is_err());
    assert!(MAX_OVERRIDES >= 1000);
    // Hostile JSON: a stash with no active variant, an unknown active id, overrides of objects
    // that do not exist, non-finite and over-long names.
    let json = format!(
        "{{\"objects\":[],\"variants\":{{\"list\":[{{\"id\":1,\"name\":\"{}\",\"overrides\":[{{\"object\":42,\"path\":\"visible\",\"value\":0}}]}},{{\"id\":1,\"name\":\"dup\",\"overrides\":[]}}],\"active\":9,\"stash\":[{{\"object\":42,\"path\":\"visible\",\"value\":1}}]}}}}",
        "x".repeat(10_000)
    );
    let s: Scene = serde_json::from_str(&json).unwrap();
    let s = s.repaired().unwrap();
    assert_eq!(s.variants.list.len(), 1, "a duplicate id is dropped");
    assert_eq!(s.variants.list[0].name.chars().count(), modelcraft_scene::variants::MAX_NAME);
    assert!(s.variants.list[0].overrides.is_empty(), "an override of a missing object is dropped");
    assert_eq!(s.variants.active, None);
    assert!(s.variants.stash.is_empty());
}
