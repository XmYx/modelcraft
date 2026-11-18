//! Project tasks: `cargo xtask <command>`.
//!
//! - `ci`: everything a pull request must pass: `fmt --check`, clippy with `-D warnings`, the
//!   tests, the layering check and the asset check.
//! - `layers`: the core crates (mesh, ops, scene, sculpt, formats, engine, render) must not depend
//!   on any UI crate (egui, eframe, winit, rfd).
//! - `assets`: every file under `assets/` and `docs/images/` is listed in `ATTRIBUTION.md`.
//! - `licenses`: every dependency's licence expression is satisfiable from the house-rules
//!   allowlist (`cargo metadata`).
//! - `progress [--check]`: the RFC-0017 progress log, `ROADMAP.md`'s table joined with the
//!   fragments in `docs/progress.d/`; `--check` validates the fragments (part of `ci`).
//! - `parity [--write]`: scores `docs/target-app-parity.md` (row parity and weighted parity, see
//!   `parity.rs`); `--write` updates the block between the `parity:` markers, and without it the
//!   command fails when the block is out of date. `ci` validates the rows strictly but only warns
//!   about a stale block (feature PRs flip rows; a maintainer refreshes the block separately).
//! - `version [set <x.y.z>]`: print or set `[workspace.package] version`.
//! - `icons`: render `assets/app-icon/modelcraft.svg` to the hicolor PNGs, the 1024 px PNG, the
//!   Windows `.ico` and the macOS `.icns` (all committed, so packaging needs no tools).
//! - `strings [<craft-i18n checkout>]`: write every interface source text (`tl!`/`tlf!`) to
//!   `docs/i18n/modelcraft-strings.tsv`, marking the ones the shared catalog already has.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod parity;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const UI_CRATES: &[&str] = &["egui", "eframe", "egui-winit", "winit", "rfd", "egui-wgpu", "egui_glow"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

fn cargo(args: &[&str]) -> Result<(), String> {
    println!("$ cargo {}", args.join(" "));
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())).args(args).current_dir(root()).status().map_err(|e| e.to_string())?;
    if status.success() { Ok(()) } else { Err(format!("cargo {} failed", args.join(" "))) }
}

const CORE: &[&str] = &[
    "modelcraft-mesh",
    "modelcraft-ops",
    "modelcraft-uv",
    "modelcraft-hair",
    "modelcraft-clone",
    "modelcraft-sim",
    "modelcraft-scene",
    "modelcraft-sculpt",
    "modelcraft-formats",
    "modelcraft-engine",
    "modelcraft-render",
    "modelcraft-script",
    "craft-render-bvh",
    "craft-render-core",
    "craft-render-gpu",
];

fn layers() -> Result<(), String> {
    for krate in CORE {
        let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args(["tree", "-p", krate, "-e", "normal", "--prefix", "none", "--format", "{p}"])
            .current_dir(root())
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let bad: Vec<&str> = text.lines().filter(|l| UI_CRATES.iter().any(|c| l.split_whitespace().next() == Some(c))).collect();
        if !bad.is_empty() {
            return Err(format!("{krate} depends on UI crates: {}", bad.join(", ")));
        }
    }
    println!("layers: {} core crates, none depends on a UI crate", CORE.len());
    Ok(())
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn assets() -> Result<(), String> {
    let root = root();
    let index = std::fs::read_to_string(root.join("ATTRIBUTION.md")).map_err(|e| format!("ATTRIBUTION.md: {e}"))?;
    let mut files = Vec::new();
    for d in ["assets", "docs/images", "docs/brand"] {
        walk(&root.join(d), &mut files);
    }
    let missing: Vec<String> = files
        .iter()
        .filter_map(|f| f.strip_prefix(&root).ok())
        .map(|f| f.to_string_lossy().replace('\\', "/"))
        .filter(|f| !f.ends_with("README.md") && !f.ends_with("LICENSE.txt"))
        .filter(|f| !index.contains(&format!("`{f}`")))
        .collect();
    if missing.is_empty() {
        println!("assets: {} files, all attributed", files.len());
        Ok(())
    } else {
        Err(format!("not listed in ATTRIBUTION.md:\n  {}", missing.join("\n  ")))
    }
}

/// SPDX ids allowed by craft-contrib `standards/house-rules-rust.md` (Licence allowlist).
const ALLOWED: &[&str] = &[
    "ISC",
    "MIT",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "Apache-2.0",
    "Unicode-3.0",
    "OFL-1.1",
    "Ubuntu-font-1.0",
    "Bitstream-Vera",
    "CC0-1.0",
    "CC-BY-4.0",
    "CC-BY-SA-4.0",
    "Zlib",
    "BSL-1.0",
    "0BSD",
    "Unlicense",
    "IJG",
    "NCSA",
    "CDLA-Permissive-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "LicenseRef-Public-Domain",
    // Third-party dependencies used unmodified only (house rules).
    "MPL-2.0",
];

/// Evaluate an SPDX expression (`A OR B`, `A AND (B OR C)`, `A/B`, `X WITH Y`) against the allowlist.
fn license_ok(expr: &str) -> bool {
    fn tokens(e: &str) -> Vec<String> {
        e.replace('(', " ( ").replace(')', " ) ").replace('/', " OR ").split_whitespace().map(str::to_string).collect()
    }
    fn or(t: &[String], i: &mut usize) -> bool {
        let mut v = and(t, i);
        while t.get(*i).map(String::as_str) == Some("OR") {
            *i += 1;
            v |= and(t, i);
        }
        v
    }
    fn and(t: &[String], i: &mut usize) -> bool {
        let mut v = atom(t, i);
        while t.get(*i).map(String::as_str) == Some("AND") {
            *i += 1;
            v &= atom(t, i);
        }
        v
    }
    fn atom(t: &[String], i: &mut usize) -> bool {
        match t.get(*i).map(String::as_str) {
            Some("(") => {
                *i += 1;
                let v = or(t, i);
                *i += 1; // ")"
                v
            }
            Some(id) => {
                let mut id = id.trim_end_matches('+').to_string();
                *i += 1;
                if t.get(*i).map(String::as_str) == Some("WITH") {
                    id = format!("{id} WITH {}", t.get(*i + 1).cloned().unwrap_or_default());
                    *i += 2;
                }
                ALLOWED.contains(&id.as_str())
            }
            None => false,
        }
    }
    let t = tokens(expr);
    let mut i = 0;
    !t.is_empty() && or(&t, &mut i) && i >= t.len()
}

fn licenses() -> Result<(), String> {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["metadata", "--format-version", "1", "--locked"])
        .current_dir(root())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let mut bad = Vec::new();
    let mut n = 0;
    for p in meta["packages"].as_array().into_iter().flatten() {
        n += 1;
        let name = p["name"].as_str().unwrap_or("?");
        // Workspace members inherit MIT OR Apache-2.0.
        match p["license"].as_str() {
            Some(l) if license_ok(l) => {}
            Some(l) => bad.push(format!("{name} {}: {l}", p["version"].as_str().unwrap_or(""))),
            None => bad.push(format!("{name}: no licence field")),
        }
    }
    if bad.is_empty() {
        println!("licenses: {n} packages, all on the allowlist");
        Ok(())
    } else {
        Err(format!("licences outside the allowlist:\n  {}", bad.join("\n  ")))
    }
}

fn fragments() -> Result<Vec<(PathBuf, String)>, String> {
    let dir = root().join("docs/progress.d");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else { return Ok(out) };
    for e in entries.flatten() {
        let p = e.path();
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name == "README.md" {
            continue;
        }
        let (issue, slug) = name.strip_suffix(".md").and_then(|n| n.split_once('-')).ok_or(format!("{name}: expected <issue>-<branch-slug>.md"))?;
        if issue.is_empty()
            || !issue.chars().all(|c| c.is_ascii_digit())
            || slug.is_empty()
            || !slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(format!("{name}: expected <issue number>-<lowercase-branch-slug>.md"));
        }
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{name}: {e}"))?;
        let rows: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        if rows.len() != 1 || !rows[0].starts_with("| ") || !rows[0].ends_with(" |") || rows[0].matches(" | ").count() != 1 {
            return Err(format!("{name}: a fragment is one progress-log row: `| YYYY-MM-DD | what landed |`"));
        }
        let date = rows[0].trim_start_matches("| ").split(' ').next().unwrap_or("");
        if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
            return Err(format!("{name}: the row must start with a YYYY-MM-DD date, found {date:?}"));
        }
        out.push((p, rows[0].to_string()));
    }
    out.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(out)
}

fn progress(check_only: bool) -> Result<(), String> {
    let frags = fragments()?;
    if check_only {
        println!("progress: {} fragments join", frags.len());
        return Ok(());
    }
    let roadmap = std::fs::read_to_string(root().join("ROADMAP.md")).map_err(|e| e.to_string())?;
    let mut in_log = false;
    let mut header_rows = 0;
    for line in roadmap.lines() {
        if line.starts_with("## ") {
            in_log = line == "## Progress log";
        }
        println!("{line}");
        if in_log && line.starts_with('|') {
            header_rows += 1;
            if header_rows == 2 {
                for (_, row) in &frags {
                    println!("{row}");
                }
            }
        }
    }
    Ok(())
}

fn version(args: &[String]) -> Result<(), String> {
    let path = root().join("Cargo.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut in_pkg = false;
    let mut out = String::new();
    let mut current = None;
    let new = match args.first().map(String::as_str) {
        Some("set") => Some(args.get(1).ok_or("usage: cargo xtask version set <x.y.z>")?.clone()),
        Some(other) => return Err(format!("unknown argument {other}")),
        None => None,
    };
    if let Some(v) = &new
        && !v.split(['.', '-', '+']).take(3).all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()))
    {
        return Err(format!("not a version: {v}"));
    }
    for line in text.lines() {
        if line.starts_with('[') {
            in_pkg = line == "[workspace.package]";
        }
        if in_pkg && line.starts_with("version = ") && current.is_none() {
            current = Some(line.trim_start_matches("version = ").trim_matches('"').to_string());
            if let Some(v) = &new {
                out.push_str(&format!("version = \"{v}\"\n"));
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    let current = current.ok_or("no [workspace.package] version")?;
    match new {
        Some(v) => {
            std::fs::write(&path, out).map_err(|e| e.to_string())?;
            println!("{current} -> {v}");
        }
        None => println!("{current}"),
    }
    Ok(())
}

/// Source texts of `tl!("…")` and `tlf!("…"` in the app.
fn source_strings() -> Vec<String> {
    let mut files = Vec::new();
    walk(&root().join("apps/modelcraft/src"), &mut files);
    let mut out = std::collections::BTreeSet::new();
    for f in files.iter().filter(|f| f.extension().is_some_and(|e| e == "rs") && !f.ends_with("ui_tests.rs")) {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        for pat in ["tl!(\"", "tlf!(\""] {
            let mut rest = text.as_str();
            while let Some(i) = rest.find(pat) {
                rest = &rest[i + pat.len()..];
                if let Some(end) = rest.find('"') {
                    out.insert(rest[..end].to_string());
                    rest = &rest[end..];
                }
            }
        }
    }
    out.into_iter().filter(|s| !s.is_empty()).collect()
}

fn strings(args: &[String]) -> Result<(), String> {
    let shared: std::collections::HashSet<String> = match args.first() {
        Some(dir) => {
            let common = Path::new(dir).join("locales/fr/common.tsv");
            let text = std::fs::read_to_string(&common).map_err(|e| format!("{}: {e}", common.display()))?;
            text.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split('\t').nth(1)).map(str::to_string).collect()
        }
        None => Default::default(),
    };
    let list = source_strings();
    let mut out = String::from(
        "# ModelCraft interface source texts (English is the key), generated by `cargo xtask strings`.\n# Columns: in-shared-catalog <TAB> source text. `shared` = already in craft-i18n common.tsv.\n",
    );
    for s in &list {
        out.push_str(if shared.contains(s) { "shared\t" } else { "app\t" });
        out.push_str(&s.replace('\t', "\\t"));
        out.push('\n');
    }
    let dir = root().join("docs/i18n");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("modelcraft-strings.tsv"), out).map_err(|e| e.to_string())?;
    println!("strings: {} source texts ({} already shared)", list.len(), list.iter().filter(|s| shared.contains(*s)).count());
    Ok(())
}

const ICON_ID: &str = "ai.storyteller.modelcraft";
const HICOLOR: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];

/// Render the icon SVG at `size` px, as straight-alpha RGBA.
fn render_icon(svg: &[u8], size: u32) -> Result<Vec<u8>, String> {
    use craft_svg::tree::{self, usvg};
    let limits = tree::Limits::default();
    let t = tree::parse(svg, &limits, &usvg::Options::default()).map_err(|e| format!("icon SVG: {e:?}"))?;
    let k = size as f32 / t.size().width().max(1.0);
    let pix = tree::draw::render(&t, usvg::Transform::from_scale(k, k), size, size, 0, 1 << 30).map_err(|e| format!("render: {e:?}"))?;
    let mut rgba = pix.data().to_vec();
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Ok(rgba)
}

fn png(rgba: Vec<u8>, size: u32) -> Result<Vec<u8>, String> {
    use craft_image_codecs::{ChannelLayout, EncodeOptions, Format, Image, SampleType};
    let img = Image::from_raw(size, size, ChannelLayout::Rgba, SampleType::U8, rgba).map_err(|e| e.to_string())?;
    craft_image_codecs::encode(&img, Format::Png, &EncodeOptions::default()).map_err(|e| e.to_string())
}

/// ICO with PNG entries (Windows Vista and later read PNG-compressed icon images).
fn ico(entries: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![0, 0, 1, 0];
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * entries.len() as u32;
    for (size, data) in entries {
        let b = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[b, b, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in entries {
        out.extend_from_slice(data);
    }
    out
}

/// ICNS with PNG entries (macOS 10.7 and later).
fn icns(entries: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let total: usize = 8 + entries.iter().map(|(_, d)| 8 + d.len()).sum::<usize>();
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&(total as u32).to_be_bytes());
    for (kind, data) in entries {
        out.extend_from_slice(*kind);
        out.extend_from_slice(&((8 + data.len()) as u32).to_be_bytes());
        out.extend_from_slice(data);
    }
    out
}

fn icons() -> Result<(), String> {
    let dir = root().join("assets/app-icon");
    let svg = std::fs::read(dir.join("modelcraft.svg")).map_err(|e| e.to_string())?;
    let mut by_size = std::collections::BTreeMap::new();
    for size in HICOLOR.into_iter().chain([1024]) {
        by_size.insert(size, png(render_icon(&svg, size)?, size)?);
    }
    let get = |s: u32| by_size.get(&s).cloned().ok_or_else(|| format!("no {s} px render"));
    for size in HICOLOR {
        let d = dir.join(format!("hicolor/{size}x{size}/apps"));
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        std::fs::write(d.join(format!("{ICON_ID}.png")), get(size)?).map_err(|e| e.to_string())?;
    }
    std::fs::write(dir.join("modelcraft-1024.png"), get(1024)?).map_err(|e| e.to_string())?;
    let ico_entries: Vec<(u32, Vec<u8>)> = [16, 24, 32, 48, 64, 128, 256].into_iter().map(|s| get(s).map(|d| (s, d))).collect::<Result<_, _>>()?;
    std::fs::write(dir.join("modelcraft.ico"), ico(&ico_entries)).map_err(|e| e.to_string())?;
    let icns_entries = vec![
        (b"icp4", get(16)?),
        (b"icp5", get(32)?),
        (b"icp6", get(64)?),
        (b"ic07", get(128)?),
        (b"ic08", get(256)?),
        (b"ic09", get(512)?),
        (b"ic10", get(1024)?),
    ];
    std::fs::write(dir.join("modelcraft.icns"), icns(&icns_entries)).map_err(|e| e.to_string())?;
    println!("icons: {} hicolor sizes, 1024 px, .ico, .icns written to {}", HICOLOR.len(), dir.display());
    Ok(())
}

const PARITY_START: &str = "<!-- parity:start -->\n";
const PARITY_END: &str = "<!-- parity:end -->";

/// The byte range of the generated block inside the map, between the `parity:` markers.
fn parity_block_range(text: &str) -> Result<(usize, usize), String> {
    let (Some(a), Some(b)) = (text.find(PARITY_START), text.find(PARITY_END)) else { return Err("parity markers missing".into()) };
    if a + PARITY_START.len() > b {
        return Err("parity markers out of order".into());
    }
    Ok((a, b))
}

fn parity(write: bool) -> Result<(), String> {
    let path = root().join("docs/target-app-parity.md");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let block = parity::score(&text)?.block();
    print!("{block}");
    let (a, b) = parity_block_range(&text)?;
    if text[a + PARITY_START.len()..b] == block {
        return Ok(());
    }
    if !write {
        return Err("the parity headline is out of date: run `cargo xtask parity --write`".into());
    }
    let new = format!("{}{PARITY_START}{block}{}", &text[..a], &text[b..]);
    std::fs::write(&path, new).map_err(|e| e.to_string())?;
    println!("wrote {}", path.display());
    Ok(())
}

/// The parity check `ci` runs. The rows are validated strictly (a malformed row, a bad status or
/// size, a table that does not parse, or missing markers is an error), but a stale generated block
/// only warns: rows are flipped in many concurrent PRs, so committing the block there would make
/// them all conflict. A maintainer refreshes it separately with `cargo xtask parity --write`.
fn parity_ci_check(text: &str) -> Result<Option<String>, String> {
    let block = parity::score(text)?.block();
    let (a, b) = parity_block_range(text)?;
    if text[a + PARITY_START.len()..b] == block {
        return Ok(None);
    }
    Ok(Some(
        "the generated parity headline is out of date (not a failure; a maintainer refreshes it with `cargo xtask parity --write` in a separate PR)".into(),
    ))
}

fn parity_ci() -> Result<(), String> {
    let path = root().join("docs/target-app-parity.md");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(warning) = parity_ci_check(&text)? {
        eprintln!("xtask: warning: {warning}");
    }
    Ok(())
}

/// The generated script API page, as `cargo run -p modelcraft-script --example api_docs` prints it.
fn script_docs_fresh() -> Result<String, String> {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["run", "-q", "-p", "modelcraft-script", "--example", "api_docs"])
        .current_dir(root())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("could not generate the script API reference:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

/// What a stale `docs/scripting-api.md` is called, or nothing when it is current.
fn script_docs_staleness(have: &str, fresh: &str) -> Option<String> {
    (have != fresh).then(|| "docs/scripting-api.md is out of date: run `cargo xtask script-docs --write`".to_string())
}

/// `docs/scripting-api.md`, generated from the script function table and the command registry.
/// With `--write` it is rewritten; without it the command fails when the page is out of date.
fn script_docs(write: bool) -> Result<(), String> {
    let fresh = script_docs_fresh()?;
    let path = root().join("docs/scripting-api.md");
    if write {
        std::fs::write(&path, &fresh).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("script-docs: wrote docs/scripting-api.md ({} lines)", fresh.lines().count());
        return Ok(());
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    match script_docs_staleness(&have, &fresh) {
        None => {
            println!("script-docs: docs/scripting-api.md is current");
            Ok(())
        }
        Some(e) => Err(e),
    }
}

/// The check `ci` runs: a stale page only warns, like the parity block (commands change in many
/// concurrent PRs, so committing the generated page there would make them all conflict).
fn script_docs_ci() -> Result<(), String> {
    let fresh = script_docs_fresh()?;
    let have = std::fs::read_to_string(root().join("docs/scripting-api.md")).unwrap_or_default();
    if let Some(w) = script_docs_staleness(&have, &fresh) {
        eprintln!("xtask: warning: {w} (not a failure; refresh it in a separate change)");
    }
    Ok(())
}

fn ci() -> Result<(), String> {
    cargo(&["fmt", "--all", "--check"])?;
    cargo(&["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"])?;
    cargo(&["test", "--workspace"])?;
    layers()?;
    assets()?;
    licenses()?;
    parity_ci()?;
    script_docs_ci()?;
    progress(true)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    let result = match args.first().map(String::as_str).unwrap_or("") {
        "ci" => ci(),
        "layers" => layers(),
        "assets" => assets(),
        "licenses" => licenses(),
        "progress" => progress(rest.first().is_some_and(|a| a == "--check")),
        "parity" => parity(rest.first().is_some_and(|a| a == "--write")),
        "script-docs" => script_docs(rest.first().is_some_and(|a| a == "--write")),
        "version" => version(rest),
        "strings" => strings(rest),
        "icons" => icons(),
        _ => Err("usage: cargo xtask <ci|layers|assets|licenses|progress [--check]|parity [--write]|script-docs [--write]|version [set X]|strings [craft-i18n dir]|icons>".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{license_ok, parity_ci_check};

    const ROWS: &str = "## One\nWeight: 2\n\n| Feature | Refs | Status | Size | Notes |\n|---|---|---|---|---|\n| a | C | D | S | |\n| b | C | P | L | |\n| c | C | M | M | |\n";

    fn doc(block: &str, rows: &str) -> String {
        format!("# Map\n\n<!-- parity:start -->\n{block}<!-- parity:end -->\n\n{rows}")
    }

    #[test]
    fn fresh_block_is_silent() {
        let block = crate::parity::score(ROWS).map(|r| r.block()).unwrap_or_default();
        assert!(!block.is_empty());
        assert_eq!(parity_ci_check(&doc(&block, ROWS)), Ok(None));
    }

    #[test]
    fn stale_block_warns_but_does_not_fail() {
        let checked = parity_ci_check(&doc("| | Value | Kind |\n|---|---|---|\n| In-scope feature rows | 1 | counted |\n", ROWS));
        assert!(matches!(&checked, Ok(Some(w)) if w.contains("out of date")), "{checked:?}");
    }

    #[test]
    fn malformed_rows_and_markers_still_fail() {
        let block = "x\n";
        let bad_size = ROWS.replace("| P | L |", "| P | huge |");
        assert!(parity_ci_check(&doc(block, &bad_size)).is_err(), "bad size");
        let no_weight = ROWS.replace("Weight: 2\n", "");
        assert!(parity_ci_check(&doc(block, &no_weight)).is_err(), "area without a weight");
        assert!(parity_ci_check(&doc(block, "no table at all\n")).is_err(), "no rows");
        assert!(parity_ci_check(ROWS).is_err(), "markers missing");
    }

    #[test]
    fn icon_containers() {
        let i = super::ico(&[(16, vec![1, 2]), (256, vec![3])]);
        assert_eq!(&i[..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!(i[6 + 16], 0, "256 px is written as 0");
        assert_eq!(i.len(), 6 + 32 + 3);
        let n = super::icns(&[(b"ic07", vec![9; 4])]);
        assert_eq!(&n[..4], b"icns");
        assert_eq!(u32::from_be_bytes([n[4], n[5], n[6], n[7]]) as usize, n.len());
    }

    #[test]
    fn a_stale_script_reference_warns_but_does_not_fail_ci() {
        assert_eq!(super::script_docs_staleness("same", "same"), None);
        let w = super::script_docs_staleness("old", "new").unwrap_or_default();
        assert!(w.contains("out of date") && w.contains("script-docs --write"), "{w}");
        // `ci` treats it as a warning: the check returns Ok even when the page is stale.
        // (script_docs_ci compares with the file on disk and only prints.)
        assert!(super::script_docs_staleness("old", "new").is_some());
    }

    #[test]
    fn spdx_expressions() {
        assert!(license_ok("MIT OR Apache-2.0"));
        assert!(license_ok("MIT/Apache-2.0"));
        assert!(license_ok("(MIT OR Apache-2.0) AND Unicode-3.0"));
        assert!(license_ok("Apache-2.0 WITH LLVM-exception OR MIT"));
        assert!(!license_ok("GPL-3.0"));
        assert!(!license_ok("MIT AND GPL-2.0"));
        assert!(license_ok("GPL-2.0 OR MIT"));
        assert!(!license_ok(""));
    }
}
