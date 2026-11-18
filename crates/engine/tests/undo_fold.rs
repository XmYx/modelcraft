//! Folding undo entries: what callbacks and scripts do after a user's step must not add steps.

use modelcraft_engine::{Command, Document};
use proptest::prelude::*;

fn group(d: &mut Document, name: &str) -> u64 {
    d.execute(Command::AddGroup { name: Some(name.into()) }).unwrap().created[0].0
}

fn names(d: &Document) -> Vec<String> {
    d.scene.objects().iter().map(|o| o.name.clone()).collect()
}

#[test]
fn entries_since_a_mark_become_one_step_that_undoes_and_redoes_whole() {
    let mut d = Document::new();
    group(&mut d, "keep");
    let mark = d.undo_mark();
    group(&mut d, "a");
    group(&mut d, "b");
    let id = group(&mut d, "c");
    d.execute(Command::Rename { id: modelcraft_engine::scene::ObjectId(id), name: "c2".into() }).unwrap();
    assert_eq!(d.undo.len(), 5);
    assert_eq!(d.fold_undo_since(mark, "Script callbacks", false), 4);
    assert_eq!(d.undo.labels(), vec!["Add group".to_string(), "Script callbacks".to_string()]);
    let after = names(&d);
    d.execute(Command::Undo).unwrap();
    assert_eq!(names(&d), vec!["keep".to_string()], "one undo reverts everything the callbacks did");
    d.execute(Command::Redo).unwrap();
    assert_eq!(names(&d), after);
    assert_eq!(d.undo.redo_labels().len(), 0);
}

#[test]
fn with_into_previous_the_users_step_absorbs_them_and_keeps_its_name() {
    let mut d = Document::new();
    group(&mut d, "base");
    d.execute(Command::AddPrimitive { kind: "cube".into(), name: Some("Mine".into()), parent: None }).unwrap();
    let mark = d.undo_mark();
    group(&mut d, "echo");
    assert_eq!(d.fold_undo_since(mark, "Script callbacks", true), 2);
    assert_eq!(d.undo.labels(), vec!["Add group".to_string(), "Add cube".to_string()]);
    assert_eq!(names(&d), vec!["base".to_string(), "Mine".to_string(), "echo".to_string()]);
    d.execute(Command::Undo).unwrap();
    assert_eq!(names(&d), vec!["base".to_string()], "the user's step and the reaction go together");
    d.execute(Command::Redo).unwrap();
    assert_eq!(names(&d).len(), 3);
}

#[test]
fn a_single_entry_is_only_renamed_and_nothing_made_is_nothing_folded() {
    let mut d = Document::new();
    group(&mut d, "a");
    let mark = d.undo_mark();
    assert_eq!(d.fold_undo_since(mark, "Script callbacks", true), 0, "no entries since the mark: nothing joins the previous one");
    group(&mut d, "b");
    assert_eq!(d.fold_undo_since(mark, "Script callbacks", false), 1);
    assert_eq!(d.undo.labels().last().map(String::as_str), Some("Script callbacks"));
    // A mark from the future or from before the history is harmless.
    assert_eq!(d.fold_undo_since(usize::MAX, "x", true), 0);
    assert_eq!(d.fold_undo_since(0, "x", false), 2);
    assert_eq!(d.undo.labels(), vec!["x".to_string()]);
    // Without a previous entry `into_previous` just folds what is there.
    let mut e = Document::new();
    let mark = e.undo_mark();
    group(&mut e, "only");
    assert_eq!(e.fold_undo_since(mark, "Script callbacks", true), 1);
}

#[test]
fn the_redo_history_can_be_held_aside_while_changes_are_made() {
    let mut d = Document::new();
    group(&mut d, "a");
    group(&mut d, "b");
    d.execute(Command::Undo).unwrap();
    assert!(d.undo.can_redo());
    let redo = d.undo.take_redo();
    assert!(!d.undo.can_redo());
    let mark = d.undo_mark();
    group(&mut d, "reaction");
    d.fold_undo_since(mark, "Script callbacks", false);
    d.undo.restore_redo(redo);
    assert_eq!(d.undo.redo_labels(), vec!["Add group".to_string()], "the user can still redo");
    assert_eq!(d.undo.labels(), vec!["Add group".to_string(), "Script callbacks".to_string()]);
    // Without holding it aside, a change clears the redo history.
    let mut e = Document::new();
    group(&mut e, "a");
    e.execute(Command::Undo).unwrap();
    group(&mut e, "b");
    assert!(!e.undo.can_redo());
}

#[test]
fn a_merged_step_replays_as_one_flat_batch() {
    let mut d = Document::new();
    let mark = d.undo_mark();
    group(&mut d, "a");
    group(&mut d, "b");
    d.fold_undo_since(mark, "Script callbacks", false);
    let mark = d.undo_mark();
    group(&mut d, "c");
    // Merging into the previous (already merged) step stays one flat batch, not a nest.
    d.fold_undo_since(mark, "Script callbacks", true);
    let (_, done, _) = d.undo.persisted(1 << 20).expect("history");
    assert_eq!(done.len(), 1);
    match &done[0] {
        Command::Batch { commands, .. } => {
            assert_eq!(commands.len(), 3);
            assert!(commands.iter().all(|c| !matches!(c, Command::Batch { .. })));
        }
        other => panic!("{other:?}"),
    }
}

proptest! {
    /// Folding changes how many steps there are, never what the scene is: after any sequence of
    /// edits and folds the scene is the same as without folds, one undo per remaining step walks
    /// back to the empty scene, and the same number of redos returns.
    #[test]
    fn folding_never_changes_the_scene(ops in proptest::collection::vec((0u8..4, any::<bool>(), any::<bool>()), 1..30)) {
        let mut plain = Document::new();
        let mut folded = Document::new();
        for (i, (kind, fold, previous)) in ops.iter().enumerate() {
            let mark = folded.undo_mark();
            for d in [&mut plain, &mut folded] {
                match kind {
                    0 => { group(d, &format!("g{i}")); }
                    1 => { d.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap(); }
                    2 => {
                        if let Some(o) = d.scene.objects().last().map(|o| o.id) {
                            d.execute(Command::Rename { id: o, name: format!("r{i}") }).unwrap();
                        }
                    }
                    _ => {
                        if let Some(o) = d.scene.objects().first().map(|o| o.id) {
                            d.execute(Command::Delete { ids: vec![o] }).unwrap();
                        }
                    }
                }
            }
            if *fold {
                folded.fold_undo_since(mark, "Script callbacks", *previous);
            }
        }
        prop_assert_eq!(names(&plain), names(&folded));
        prop_assert!(folded.undo.len() <= plain.undo.len());
        let steps = folded.undo.len();
        let end = names(&folded);
        for _ in 0..steps {
            folded.execute(Command::Undo).unwrap();
        }
        prop_assert!(folded.scene.is_empty());
        for _ in 0..steps {
            folded.execute(Command::Redo).unwrap();
        }
        prop_assert_eq!(names(&folded), end);
    }
}
