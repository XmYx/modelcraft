//! The editor's draft file: round trips, torn and hostile files, limits.

use modelcraft_script::draft::{self, Draft, MAX_DRAFT_BYTES};
use proptest::prelude::*;

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mc-draft-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn a_draft_round_trips_and_clear_removes_it() {
    let dir = tmp("roundtrip");
    assert_eq!(draft::load(&dir), None, "no folder, no draft");
    let d = Draft { text: "let x = \"quote\\n\";\n// ünï 日本\n".into(), path: Some("/tmp/a b/c.rhai".into()) };
    draft::save(&dir, &d).unwrap();
    assert_eq!(draft::load(&dir), Some(d.clone()));
    // A newer draft replaces the older one whole.
    let d2 = Draft { text: "print(1);".into(), path: None };
    draft::save(&dir, &d2).unwrap();
    assert_eq!(draft::load(&dir), Some(d2));
    draft::clear(&dir);
    assert_eq!(draft::load(&dir), None);
    draft::clear(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_oversized_text_is_refused_and_leaves_the_old_draft() {
    let dir = tmp("big");
    draft::save(&dir, &Draft { text: "old".into(), path: None }).unwrap();
    let big = Draft { text: "x".repeat(MAX_DRAFT_BYTES + 1), path: None };
    assert!(draft::save(&dir, &big).unwrap_err().contains("larger than"));
    assert_eq!(draft::load(&dir).map(|d| d.text), Some("old".into()));
    let exact = Draft { text: "y".repeat(MAX_DRAFT_BYTES), path: None };
    draft::save(&dir, &exact).unwrap();
    assert_eq!(draft::load(&dir).map(|d| d.text.len()), Some(MAX_DRAFT_BYTES));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn damaged_or_foreign_files_are_ignored_not_trusted() {
    let dir = tmp("hostile");
    std::fs::create_dir_all(&dir).unwrap();
    let f = draft::path_in(&dir);
    for bytes in [
        &b""[..],
        b"not json",
        b"{\"version\":1,\"text\":",
        b"{\"version\":2,\"text\":\"future\"}",
        b"{\"version\":1,\"text\":\"\"}",
        b"{\"version\":1,\"text\":5}",
        b"[1,2,3]",
        b"\xff\xfe\x00garbage",
    ] {
        std::fs::write(&f, bytes).unwrap();
        assert_eq!(draft::load(&dir), None, "{:?}", String::from_utf8_lossy(bytes));
    }
    // A missing `path` is fine.
    std::fs::write(&f, b"{\"version\":1,\"text\":\"keep me\"}").unwrap();
    assert_eq!(draft::load(&dir), Some(Draft { text: "keep me".into(), path: None }));
    // A huge file is not even read.
    std::fs::write(&f, vec![b' '; MAX_DRAFT_BYTES * 2 + 10]).unwrap();
    assert_eq!(draft::load(&dir), None);
    // A directory where the file should be is not a draft.
    std::fs::remove_file(&f).unwrap();
    std::fs::create_dir_all(&f).unwrap();
    assert_eq!(draft::load(&dir), None);
    assert!(draft::save(&dir, &Draft { text: "x".into(), path: None }).is_err(), "cannot write over a directory, and says so");
    let _ = std::fs::remove_dir_all(&dir);
}

proptest! {
    #[test]
    fn any_text_survives(text in "\\PC{1,400}", path in proptest::option::of("\\PC{0,60}")) {
        let dir = tmp(&format!("prop-{}", text.len()));
        let d = Draft { text, path };
        draft::save(&dir, &d).unwrap();
        prop_assert_eq!(draft::load(&dir), Some(d));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
