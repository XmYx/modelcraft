//! Plugins: manifests, installing, grants and the sandbox a plugin runs in.

use std::path::{Path, PathBuf};

use modelcraft_engine::{Command, Document};
use modelcraft_script::plugin::{self, API_VERSION, Grants, Installation, Manifest, Permission, Status, Version};
use modelcraft_script::{Event, RunContext};
use proptest::prelude::*;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc-plugin-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn manifest_json(id: &str, version: &str, perms: &[&str]) -> String {
    serde_json::json!({"id": id, "name": "Row Tools", "version": version, "api": API_VERSION, "entry": "main.rhai", "description": "Makes rows", "author": "Someone", "permissions": perms}).to_string()
}

/// A package folder with a manifest and an entry script.
fn package(dir: &Path, id: &str, version: &str, perms: &[&str], entry: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("plugin.json"), manifest_json(id, version, perms)).unwrap();
    std::fs::write(dir.join("main.rhai"), entry).unwrap();
}

#[test]
fn versions_parse_strictly_and_order_numerically() {
    assert_eq!(Version::parse("1.2.3"), Ok(Version(1, 2, 3)));
    assert!(Version::parse("1.10.0").unwrap() > Version::parse("1.9.9").unwrap());
    for bad in ["", "1", "1.2", "1.2.3.4", "1.2.x", "01.2.3", "1.2.-3", "1.2.3-beta", " 1.2.3", "1..3", "9999999.0.0"] {
        assert!(Version::parse(bad).is_err(), "{bad:?}");
    }
    assert_eq!(Version(3, 0, 12).to_string(), "3.0.12");
}

#[test]
fn a_manifest_is_checked_field_by_field() {
    let ok = Manifest::parse(&manifest_json("row.tools", "1.0.0", &["edit_scene", "register_tools"])).unwrap();
    assert_eq!(ok.requested(), vec![Permission::EditScene, Permission::RegisterTools]);
    let base = serde_json::json!({"id": "row.tools", "name": "N", "version": "1.0.0", "api": 1, "entry": "main.rhai"});
    let with = |k: &str, v: serde_json::Value| {
        let mut m = base.clone();
        m[k] = v;
        Manifest::parse(&m.to_string())
    };
    assert!(Manifest::parse(&base.to_string()).is_ok(), "permissions are optional");
    for (k, v, needle) in [
        ("id", serde_json::json!("AB"), "id"),
        ("id", serde_json::json!("Has Space"), "id"),
        ("id", serde_json::json!("a..b"), "id"),
        ("id", serde_json::json!("-lead"), "id"),
        ("id", serde_json::json!("x".repeat(65)), "id"),
        ("name", serde_json::json!("  "), "name"),
        ("name", serde_json::json!("a\nb"), "name"),
        ("version", serde_json::json!("1.0"), "version"),
        ("api", serde_json::json!(0), "no longer supported"),
        ("api", serde_json::json!(API_VERSION + 1), "update ModelCraft"),
        ("entry", serde_json::json!("main.py"), "entry"),
        ("entry", serde_json::json!("../x.rhai"), "entry"),
        ("entry", serde_json::json!("/etc/x.rhai"), "entry"),
        ("permissions", serde_json::json!(["root"]), "unknown variant"),
        ("permissions", serde_json::json!(["edit_scene", "edit_scene"]), "twice"),
        ("author", serde_json::json!("x".repeat(81)), "too long"),
    ] {
        let e = with(k, v.clone()).unwrap_err();
        assert!(e.contains(needle), "{k}={v}: {e}");
    }
    // Unknown fields are refused: a typo in `permissions` must not silently mean "none".
    assert!(with("permisions", serde_json::json!(["edit_scene"])).unwrap_err().contains("unknown field"));
    assert!(Manifest::parse("not json").is_err());
    assert!(Manifest::parse("{}").is_err());
}

proptest! {
    /// Whatever bytes arrive as a manifest, parsing answers and never panics; what it accepts is valid.
    #[test]
    fn arbitrary_manifests_never_panic(text in "\\PC{0,300}") {
        if let Ok(m) = Manifest::parse(&text) {
            prop_assert!(m.validate().is_ok());
        }
    }

    #[test]
    fn arbitrary_json_values_never_panic(id in "\\PC{0,80}", version in "\\PC{0,20}", entry in "\\PC{0,40}", api in any::<u32>()) {
        let v = serde_json::json!({"id": id, "name": "n", "version": version, "api": api, "entry": entry});
        let _ = Manifest::parse(&v.to_string());
    }

    #[test]
    fn version_order_matches_the_tuple(a in (0u32..1000, 0u32..1000, 0u32..1000), b in (0u32..1000, 0u32..1000, 0u32..1000)) {
        let (x, y) = (Version::parse(&format!("{}.{}.{}", a.0, a.1, a.2)).unwrap(), Version::parse(&format!("{}.{}.{}", b.0, b.1, b.2)).unwrap());
        prop_assert_eq!(x.cmp(&y), a.cmp(&b));
    }
}

#[test]
fn installing_copies_a_checked_package_and_scanning_finds_it() {
    let (src, plugins) = (tmp("i-src"), tmp("i-plugins"));
    package(&src, "row.tools", "1.0.0", &["register_tools"], "// entry");
    std::fs::create_dir_all(src.join("lib")).unwrap();
    std::fs::write(src.join("lib/shapes.rhai"), "fn f() { 1 }").unwrap();
    std::fs::write(src.join("README.md"), "# Row tools").unwrap();
    let Installation::New(i) = plugin::install(&src, &plugins).unwrap() else { panic!("a new plugin") };
    assert_eq!(i.dir, plugins.join("row.tools"));
    assert!(i.dir.join("lib/shapes.rhai").is_file() && i.dir.join("README.md").is_file());
    let (found, bad) = plugin::scan(&plugins);
    assert!(bad.is_empty(), "{bad:?}");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].manifest.name, "Row Tools");
    // The same or an older version is not installed over it; a newer one updates it.
    assert!(plugin::install(&src, &plugins).unwrap_err().contains("already installed"));
    package(&src, "row.tools", "0.9.0", &[], "// older");
    assert!(plugin::install(&src, &plugins).is_err());
    // The new version no longer has the library or the readme.
    std::fs::remove_dir_all(src.join("lib")).unwrap();
    std::fs::remove_file(src.join("README.md")).unwrap();
    package(&src, "row.tools", "1.1.0", &["register_tools", "edit_scene"], "// newer");
    match plugin::install(&src, &plugins).unwrap() {
        Installation::Updated(i, old) => {
            assert_eq!(old, Version(1, 0, 0));
            assert_eq!(i.manifest.version, "1.1.0");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read_to_string(plugins.join("row.tools/main.rhai")).unwrap(), "// newer");
    assert!(!plugins.join("row.tools/lib").exists(), "the old version's files are gone");
    let leftovers: Vec<_> = std::fs::read_dir(&plugins).unwrap().filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(leftovers, vec!["row.tools".to_string()], "no staging folders are left behind");
}

#[test]
fn a_package_that_breaks_the_rules_is_refused_before_anything_is_copied() {
    let plugins = tmp("r-plugins");
    let make = |name: &str| -> PathBuf {
        let src = tmp(&format!("r-{name}"));
        package(&src, "bad.package", "1.0.0", &[], "// entry");
        src
    };
    // Disallowed file types.
    let src = make("exe");
    std::fs::write(src.join("tool.so"), "x").unwrap();
    assert!(plugin::install(&src, &plugins).unwrap_err().contains("only rhai, json, md, txt"));
    let src = make("dll");
    std::fs::write(src.join("native.dll"), "x").unwrap();
    assert!(plugin::install(&src, &plugins).is_err());
    // Hidden files, odd names, big files, too many files, deep folders.
    let src = make("hidden");
    std::fs::write(src.join(".secret"), "x").unwrap();
    assert!(plugin::install(&src, &plugins).unwrap_err().contains("hidden"));
    let src = make("name");
    std::fs::write(src.join("a b|c.rhai"), "x").unwrap();
    assert!(plugin::install(&src, &plugins).is_err());
    let src = make("big");
    std::fs::write(src.join("big.txt"), vec![b'x'; plugin::MAX_FILE_BYTES as usize + 1]).unwrap();
    assert!(plugin::install(&src, &plugins).unwrap_err().contains("larger than"));
    let src = make("many");
    for i in 0..plugin::MAX_FILES {
        std::fs::write(src.join(format!("f{i}.txt")), "x").unwrap();
    }
    assert!(plugin::install(&src, &plugins).unwrap_err().contains("more than"));
    let src = make("deep");
    let mut d = src.clone();
    for i in 0..=plugin::MAX_DEPTH + 1 {
        d = d.join(format!("d{i}"));
    }
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("x.txt"), "x").unwrap();
    assert!(plugin::install(&src, &plugins).unwrap_err().contains("nest"));
    // No manifest, no entry, an entry that is not in the package, a folder that is no package.
    let empty = tmp("r-empty");
    assert!(plugin::install(&empty, &plugins).unwrap_err().contains("plugin.json"));
    let src = make("noentry");
    std::fs::remove_file(src.join("main.rhai")).unwrap();
    assert!(plugin::install(&src, &plugins).unwrap_err().contains("entry"));
    assert!(plugin::install(&tmp("r-nothing").join("missing"), &plugins).is_err());
    assert!(plugin::scan(&plugins).0.is_empty(), "nothing was installed");
    assert!(!plugins.join("bad.package").exists());
}

#[cfg(unix)]
#[test]
fn links_in_a_package_are_refused_so_it_cannot_copy_files_from_elsewhere() {
    let plugins = tmp("l-plugins");
    let src = tmp("l-src");
    package(&src, "linky", "1.0.0", &[], "// entry");
    let outside = tmp("l-outside");
    std::fs::write(outside.join("secret.txt"), "secret").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.txt"), src.join("notes.txt")).unwrap();
    assert!(plugin::install(&src, &plugins).unwrap_err().contains("links are not allowed"));
    std::fs::remove_file(src.join("notes.txt")).unwrap();
    std::os::unix::fs::symlink(&outside, src.join("dir")).unwrap();
    assert!(plugin::install(&src, &plugins).unwrap_err().contains("links are not allowed"));
    // Even the manifest and the entry cannot be links.
    std::fs::remove_file(src.join("dir")).unwrap();
    std::fs::remove_file(src.join("main.rhai")).unwrap();
    std::fs::write(outside.join("real.rhai"), "// x").unwrap();
    std::os::unix::fs::symlink(outside.join("real.rhai"), src.join("main.rhai")).unwrap();
    assert!(plugin::install(&src, &plugins).is_err());
}

#[test]
fn scanning_reports_broken_plugins_and_remove_only_touches_plugins() {
    let plugins = tmp("s-plugins");
    package(&plugins.join("good.one"), "good.one", "1.0.0", &[], "// x");
    package(&plugins.join("wrong.folder"), "other.id", "1.0.0", &[], "// x");
    std::fs::create_dir_all(plugins.join("no.manifest")).unwrap();
    std::fs::create_dir_all(plugins.join(".hidden")).unwrap();
    std::fs::write(plugins.join("loose-file.txt"), "x").unwrap();
    let (ok, bad) = plugin::scan(&plugins);
    assert_eq!(ok.len(), 1);
    assert_eq!(bad.len(), 2, "{bad:?}");
    assert!(bad.iter().any(|(_, e)| e.contains("id is")) && bad.iter().any(|(_, e)| e.contains("no plugin.json")));
    // Remove deletes the plugin's folder and refuses ids that are not plugins.
    assert!(plugin::remove(&plugins, "no.manifest").is_err());
    assert!(plugin::remove(&plugins, "../good.one").is_err());
    assert!(plugin::remove(&plugins, "loose-file.txt").is_err());
    assert!(plugin::remove(&plugins, "good.one").is_ok());
    assert!(!plugins.join("good.one").exists());
    assert!(plugins.join("no.manifest").exists() && plugins.join("loose-file.txt").exists());
}

#[test]
fn grants_decide_whether_to_ask_and_only_ask_about_what_is_new() {
    let dir = tmp("g");
    let mut g = Grants::load(&dir);
    let m = |perms: &[&str]| Manifest::parse(&manifest_json("row.tools", "1.0.0", perms)).unwrap();
    // A new plugin: ask about everything it wants. One that wants nothing is still asked once.
    assert_eq!(g.status(&m(&["edit_scene", "register_tools"])), Status::NeedsApproval(vec![Permission::EditScene, Permission::RegisterTools]));
    assert_eq!(g.status(&m(&[])), Status::NeedsApproval(vec![]));
    g.approve("row.tools", &[Permission::EditScene, Permission::RegisterTools]);
    assert_eq!(g.status(&m(&["register_tools", "edit_scene"])), Status::Ready(vec![Permission::EditScene, Permission::RegisterTools]));
    // Asking for less than was granted runs without a prompt; asking for more asks about the extra only.
    assert_eq!(g.status(&m(&["register_tools"])), Status::Ready(vec![Permission::RegisterTools]));
    assert_eq!(g.status(&m(&["edit_scene", "write_project_files"])), Status::NeedsApproval(vec![Permission::WriteProjectFiles]));
    // The grant survives a restart, in a file; garbage in the file means "nothing granted".
    g.save(&dir).unwrap();
    assert_eq!(Grants::load(&dir), g);
    // Denying switches the plugin off whatever it asks; enabling again keeps the old grant.
    g.deny("row.tools");
    assert_eq!(g.status(&m(&["edit_scene"])), Status::Disabled);
    g.set_enabled("row.tools", true);
    assert_eq!(g.status(&m(&["edit_scene"])), Status::Ready(vec![Permission::EditScene]));
    g.forget("row.tools");
    assert!(g.get("row.tools").is_none());
    for junk in [
        &b""[..],
        b"{",
        b"[1]",
        b"{\"plugins\": 5}",
        b"{\"plugins\": {\"../x\": {\"permissions\": [\"edit_scene\"]}}}",
        b"{\"plugins\": {\"ok.id\": {\"permissions\": [\"root\"]}}}",
    ] {
        std::fs::write(dir.join("plugin-grants.json"), junk).unwrap();
        let loaded = Grants::load(&dir);
        assert!(loaded.get("../x").is_none(), "{:?}", String::from_utf8_lossy(junk));
        assert!(loaded.get("ok.id").is_none());
    }
}

fn start3(entry: &str, perms: &[Permission], project: Option<&Path>) -> (plugin::LoadedPlugin, modelcraft_script::RunReport, Document) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let plugins = tmp(&format!("start-{}", N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let ids: Vec<&str> = perms.iter().map(|p| p.name()).collect();
    package(&plugins.join("test.plugin"), "test.plugin", "1.0.0", &ids, entry);
    let installed = plugin::scan(&plugins).0.remove(0);
    let mut d = Document::new();
    let (p, r) = plugin::start(&installed, perms, project, &mut d, &mut RunContext::default());
    (p, r, d)
}

fn start(entry: &str, perms: &[Permission]) -> (plugin::LoadedPlugin, modelcraft_script::RunReport, Document) {
    start3(entry, perms, None)
}

#[test]
fn a_plugin_without_edit_scene_cannot_change_the_scene_and_with_it_can() {
    let (_, r, d) = start3("add_group(\"nope\");", &[], None);
    assert!(r.error.as_ref().is_some_and(|e| e.message.contains("edit_scene")), "{:?}", r.error);
    assert!(d.scene.is_empty());
    let (_, r, d) = start3("add_group(\"yes\");", &[Permission::EditScene], None);
    assert!(r.ok(), "{:?}", r.error);
    assert_eq!(d.scene.objects()[0].name, "yes");
    // Batches, undo and every other command go through the same gate, not only the helpers.
    let (_, r, d) = start("cmd::batch(#{ commands: [#{ cmd: \"add_group\" }] });", &[]);
    assert!(r.error.is_some(), "batch is refused too");
    assert!(d.scene.is_empty());
    let (_, r, _) = start("run(#{ cmd: \"undo\" });", &[]);
    assert!(r.error.is_some());
    // Reading the scene needs no permission.
    let (_, r, _) = start("print(object_count());", &[]);
    assert!(r.ok(), "{:?}", r.error);
}

#[test]
fn tools_and_callbacks_need_their_permissions() {
    let (p, r, _) = start("register_tool(\"T\", || { });", &[]);
    assert!(r.error.as_ref().is_some_and(|e| e.message.contains("register_tools")), "{:?}", r.error);
    assert!(p.session.as_ref().unwrap().tools().is_empty());
    let (p, r, _) = start("register_tool(\"T\", || { });", &[Permission::RegisterTools]);
    assert!(r.ok(), "{:?}", r.error);
    assert_eq!(p.session.as_ref().unwrap().tools().len(), 1);
    let (p, r, _) = start("on_scene_changed(|e| { });", &[Permission::RegisterTools]);
    assert!(r.error.as_ref().is_some_and(|e| e.message.contains("listen_events")), "{:?}", r.error);
    assert!(!p.session.as_ref().unwrap().has_callbacks());
    let (mut p, r, mut d) = start("on_selection_changed(|e| { print(\"heard\"); });", &[Permission::ListenEvents]);
    assert!(r.ok(), "{:?}", r.error);
    let reports = p.session.as_mut().unwrap().fire(&mut d, &mut RunContext::default(), &Event::SelectionChanged);
    assert_eq!(reports[0].lines[0].text, "heard");
}

#[test]
fn a_plugin_reads_its_own_folder_and_the_project_only_when_granted() {
    let project = tmp("p-project");
    std::fs::write(project.join("data.txt"), "project data").unwrap();
    let read = format!("print(read_text({:?}));", project.join("data.txt").display().to_string());
    let (_, r, _) = start3(&read, &[], Some(&project));
    assert!(r.error.is_some(), "no permission, no project files");
    let (_, r, _) = start3(&read, &[Permission::ReadProjectFiles], Some(&project));
    assert!(r.ok(), "{:?}", r.error);
    assert_eq!(r.lines[0].text, "project data");
    // Without a project there is nothing to grant.
    let (_, r, _) = start3(&read, &[Permission::ReadProjectFiles], None);
    assert!(r.error.is_some());
    // Its own files are always readable (relative to its folder), nothing outside is.
    let (_, r, _) = start3("print(file_exists(\"plugin.json\"));", &[], None);
    assert!(r.ok() && r.lines[0].text == "true", "{:?}", r.error);
    let (_, r, _) = start3("read_text(\"/etc/hostname\")", &[Permission::ReadProjectFiles], Some(&project));
    assert!(r.error.is_some());
}

#[test]
fn writing_is_confined_to_plugin_out_in_the_project_folder() {
    let project = tmp("w-project");
    let target = project.join("plugin-out").join("test.plugin").join("out.txt");
    let ok = format!("write_text({:?}, \"hello\");", target.display().to_string());
    let (_, r, _) = start3(&ok, &[], Some(&project));
    assert!(r.error.is_some(), "no permission");
    assert!(!target.exists());
    let (_, r, _) = start3(&ok, &[Permission::WriteProjectFiles], Some(&project));
    assert!(r.ok(), "{:?}", r.error);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    // Anywhere else in the project, or one level up, is refused even with the permission.
    for elsewhere in
        [project.join("elsewhere.txt"), project.join("plugin-out").join("other.plugin").join("x.txt"), project.parent().unwrap().join("escaped.txt")]
    {
        let src = format!("write_text({:?}, \"x\");", elsewhere.display().to_string());
        let (_, r, _) = start3(&src, &[Permission::WriteProjectFiles], Some(&project));
        assert!(r.error.is_some(), "{}", elsewhere.display());
        assert!(!elsewhere.exists(), "{}", elsewhere.display());
    }
    // Reading the project is a separate permission.
    std::fs::write(project.join("readme.txt"), "x").unwrap();
    let src = format!("read_text({:?})", project.join("readme.txt").display().to_string());
    let (_, r, _) = start3(&src, &[Permission::WriteProjectFiles], Some(&project));
    assert!(r.error.is_some());
}

#[test]
fn a_plugin_imports_only_its_own_libraries_and_runs_under_the_usual_limits() {
    let plugins = tmp("lib-plugins");
    let dir = plugins.join("lib.plugin");
    package(&dir, "lib.plugin", "1.0.0", &["edit_scene"], "import \"lib/names\" as n; add_group(n::tag(\"x\"));");
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(dir.join("lib/names.rhai"), "fn tag(s) { \"plug-\" + s }").unwrap();
    let installed = plugin::scan(&plugins).0.remove(0);
    let mut d = Document::new();
    let (_, r) = plugin::start(&installed, &[Permission::EditScene], None, &mut d, &mut RunContext::default());
    assert!(r.ok(), "{:?}", r.error);
    assert_eq!(d.scene.objects()[0].name, "plug-x");
    // Another plugin's files, or the user's scripts, are not importable.
    let (_, r, _) = start3("import \"../lib.plugin/lib/names\" as n;", &[], None);
    assert!(r.error.is_some());
    // An endless entry script is stopped; the plugin is still returned so its error can be shown.
    let (p, r, _) = start3("loop { }", &[], None);
    assert!(r.error.is_some() && p.session.is_some());
    // A missing entry file at start is an error report, not a panic.
    let plugins = tmp("noentry-plugins");
    package(&plugins.join("gone.plugin"), "gone.plugin", "1.0.0", &[], "// x");
    let installed = plugin::scan(&plugins).0.remove(0);
    std::fs::remove_file(installed.dir.join("main.rhai")).unwrap();
    let (_, r) = plugin::start(&installed, &[], None, &mut Document::new(), &mut RunContext::default());
    assert!(r.error.is_some());
    let _ = Command::Undo;
}
