//! `run-script` and libraries: `import` reads from the script's folder or `--library`.

use modelcraft_engine::Document;

use crate::script_cmd::{parse, run_script};

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mc-cli-lib-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("lib")).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_script_file_imports_from_its_own_folder_and_library_picks_another() {
    let dir = tmp("own");
    std::fs::write(dir.join("lib/shapes.rhai"), "fn name() { \"from-lib\" }").unwrap();
    let script = dir.join("main.rhai");
    std::fs::write(&script, "import \"lib/shapes\" as s; add_group(s::name());").unwrap();
    let o = parse(&strings(&["new", script.to_str().unwrap()])).unwrap();
    let mut d = Document::new();
    let r = run_script(&mut d, &o, Some("new"), true).unwrap();
    assert!(r.ok(), "{:?}", r.error);
    assert_eq!(d.scene.objects()[0].name, "from-lib");
    // An inline script has no folder: imports are refused until --library names one.
    let o = parse(&strings(&["new", "-e", "import \"lib/shapes\" as s; add_group(s::name());"])).unwrap();
    assert!(!run_script(&mut Document::new(), &o, Some("new"), true).unwrap().ok());
    let o = parse(&strings(&["new", "-e", "import \"lib/shapes\" as s; add_group(s::name());", "--library", dir.to_str().unwrap()])).unwrap();
    let mut d = Document::new();
    assert!(run_script(&mut d, &o, Some("new"), true).unwrap().ok());
    assert_eq!(d.scene.objects()[0].name, "from-lib");
}
