//! Prints `docs/scripting-api.md`: `cargo run -q -p modelcraft-script --example api_docs`.
//! `cargo xtask script-docs` writes it (or checks that it is current).

use std::path::PathBuf;

fn first_comment(text: &str) -> String {
    let lines: Vec<&str> = text.lines().take_while(|l| l.trim_start().starts_with("//")).map(|l| l.trim_start().trim_start_matches('/').trim()).collect();
    lines.join(" ")
}

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/scripts");
    let mut examples: Vec<(String, String)> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "rhai"))
                .filter_map(|p| {
                    let name = p.file_name()?.to_string_lossy().into_owned();
                    let text = std::fs::read_to_string(&p).ok()?;
                    Some((name, first_comment(&text)))
                })
                .collect()
        })
        .unwrap_or_default();
    examples.sort();
    print!("{}", modelcraft_script::docs::render_markdown(&examples));
}
