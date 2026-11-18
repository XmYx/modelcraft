//! `modelcraft-cli`: ModelCraft without the window. Every subcommand runs engine commands, the
//! same ones the app's menus and shortcuts run.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod bench_cmd;
mod hair_cmd;
mod macro_cmd;
mod render_cmd;
mod script_cmd;
#[cfg(test)]
mod script_library_tests;
#[cfg(test)]
mod script_run_mcp_tests;
mod sim_cmd;

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use glam::Vec3;
use modelcraft_engine::scene::{ObjectKind, Scene};
use modelcraft_engine::sculpt::{BrushKind, BrushSettings, Dab, SculptMesh};
use modelcraft_engine::{COMMANDS, Command, Document, control as channel};

mod mcp;

const USAGE: &str = "Usage: modelcraft-cli <command> [args]

Commands:
  commands                          list the engine commands (JSON, `cmd` tag)
  new <out.modelcraft>              write an empty project
  info <file>                       print the object tree with polygon counts
  validate <file>                   evaluate every object and check every mesh
  run <project> <script.jsonl> [-o <out>]
                                    apply one JSON command per line, then save
  exec <project> '<json command>' [-o <out>]
                                    apply one command, then save
  export <project> <out.glb|obj|stl|fbx|3mf|usda> [--up y|z] [--scale M]
  export-uv <project> <out.svg|png> [--size N] [--object ID] [--udim]
                                    the UV layout of a mesh object as a picture
  convert <in> <out> [--up auto|y|z] [--scale M]
                                    any readable file (glTF, OBJ, STL, PLY, FBX, 3MF) to
                                    .glb, .obj, .stl, .fbx, .3mf, .usda or .modelcraft; --up and --scale
                                    (metres per file unit) convert axes and units
  recover <project>                 replay the crash journal next to a project and save
  control [project]                 JSON-lines control channel on stdin/stdout:
                                    one command per line in, {\"ok\":…} per line out
  bake <project> <object id or name> <out.mcpc> [--start F] [--end F] [--step F] [--world]
                                    bake an object's vertices over the frames into a point cache
  cache-info <file.mcpc>            print a point cache's header and bounds
  bench-skin [--verts N] [--joints N] [--frames N]
                                    bind and play a skinned tube: weights, evaluation, playback
  serve [project] [--port N]        the same channel on a loopback socket, behind a token
                                    written to a file only you can read
  call <method> ['<json params>'] [--port N] [--token T]
                                    one request to a running app or `serve` (engine.execute,
                                    document.info, document.save, …)
  mcp [project] [--app [--port N]]  MCP server on stdio; --app drives the running window
  bench [--polys N] [--dabs N]      sculpt latency and GPU upload/draw times on a dense mesh
  bench --groups [--polys N]        patch, visibility, region, shell, trim, occlusion, decimate and projection times
  uv-pack-bench [--islands N] [--tiles T]
                                    pack N random island rectangles (into T UDIM tiles): time and utilisation
  uv-transfer-bench [--segments N]  transfer the UVs of a sphere to one 1.5 times finer: time and distance
  uv <file>                         UV islands and distortion of every mesh object (JSON lines)
  uv-bench [--segments N] [--shape S] [--method lscm|abf|both] [--cut auto|meridian] [--relax N]
                                    unwrap a sphere (or tube, cone, torus, roundcube): time and distortion
  -h, --help | -V, --version
";

fn usage() -> String {
    format!(
        "{USAGE}\nScripts (docs/scripting.md):\n{}\n{}\n\nRendering:\n{}\n{}\n\nHair:\n{}\n\nSimulation:\n{}\n",
        script_cmd::USAGE,
        macro_cmd::USAGE,
        render_cmd::USAGE,
        bench_cmd::USAGE,
        hair_cmd::USAGE,
        sim_cmd::USAGE
    )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => {
            print!("{}", usage());
            Ok(())
        }
        Some("-V" | "--version") => {
            println!("{}", modelcraft_engine::formats::GENERATOR);
            Ok(())
        }
        Some(cmd) => run(cmd, args.get(1..).unwrap_or_default()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("modelcraft-cli: {e}");
            ExitCode::FAILURE
        }
    }
}

fn arg<'a>(a: &'a [String], i: usize, what: &str) -> Result<&'a str, String> {
    a.get(i).map(String::as_str).ok_or_else(|| format!("missing {what}\n\n{USAGE}"))
}

fn flag(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

fn is_project(p: &Path) -> bool {
    p.extension().is_some_and(|e| e == modelcraft_engine::formats::project::EXTENSION)
}

/// Open a project, or import any other readable file into a new document.
pub(crate) fn load(path: &str) -> Result<Document, String> {
    let p = Path::new(path);
    if is_project(p) {
        let mut d = Document::open(p).map_err(|e| e.to_string())?;
        if let Some(n) = d.history_note.take() {
            eprintln!("note: {n}");
        }
        if d.pending_recovery > 0 {
            eprintln!("note: {} unsaved commands wait in the crash journal; `modelcraft-cli recover` replays them", d.pending_recovery);
        }
        d.detach_journal();
        Ok(d)
    } else {
        let mut d = Document::new();
        let out = d.execute(Command::Import { path: path.to_string(), up: Default::default(), scale: None }).map_err(|e| e.to_string())?;
        for w in out.warnings {
            eprintln!("warning: {w}");
        }
        Ok(d)
    }
}

/// `--up` and `--scale` (metres per file unit).
fn convert_flags(a: &[String]) -> Result<modelcraft_engine::formats::exchange::Convert, String> {
    use modelcraft_engine::formats::exchange::{Convert, Up};
    let up = match flag(a, "--up").as_deref() {
        None | Some("auto") => Up::Auto,
        Some("y") | Some("Y") => Up::Y,
        Some("z") | Some("Z") => Up::Z,
        Some(other) => return Err(format!("--up takes auto, y or z, not {other:?}")),
    };
    let scale = match flag(a, "--scale") {
        None => None,
        Some(s) => Some(s.parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0).ok_or_else(|| format!("--scale takes a positive number, not {s:?}"))?),
    };
    Ok(Convert { up, scale })
}

fn save(d: &mut Document, out: &str) -> Result<(), String> {
    save_with(d, out, Default::default())
}

fn save_with(d: &mut Document, out: &str, convert: modelcraft_engine::formats::exchange::Convert) -> Result<(), String> {
    let p = Path::new(out);
    if is_project(p) {
        d.save_as(p).map_err(|e| e.to_string())?;
    } else {
        // What the format could not keep (a rigged .glb says so) goes to stderr.
        for w in d.export_report(p, convert).map_err(|e| e.to_string())? {
            eprintln!("warning: {w}");
        }
    }
    // A CLI save is final: no journal left behind.
    d.detach_journal();
    Ok(())
}

fn run(cmd: &str, a: &[String]) -> Result<(), String> {
    match cmd {
        "commands" => {
            for (name, help) in COMMANDS {
                println!("{name}\t{help}");
            }
            for (name, help) in modelcraft_engine::anim::COMMANDS {
                println!("anim op={name}\t{help}");
            }
            for (name, help) in modelcraft_engine::rig::COMMANDS {
                println!("rig op={name}\t{help}");
            }
            for (name, help) in modelcraft_engine::organize::COMMANDS {
                println!("organize op={name}\t{help}");
            }
            for (name, help) in modelcraft_engine::hair::COMMANDS {
                println!("hair op={name}\t{help}");
            }
            for (name, help) in modelcraft_engine::sim::COMMANDS {
                println!("sim op={name}\t{help}");
            }
            Ok(())
        }
        "new" => save(&mut Document::new(), arg(a, 0, "output file")?),
        "info" => {
            let mut d = load(arg(a, 0, "file")?)?;
            info(&mut d);
            Ok(())
        }
        "validate" => validate(&mut load(arg(a, 0, "file")?)?),
        "run" | "exec" => {
            let project = arg(a, 0, "project")?;
            let mut d = if Path::new(project).exists() { load(project)? } else { Document::new() };
            let lines: Vec<String> = if cmd == "run" {
                std::fs::read_to_string(arg(a, 1, "script")?).map_err(|e| e.to_string())?.lines().map(str::to_string).collect()
            } else {
                vec![arg(a, 1, "command")?.to_string()]
            };
            for (n, line) in lines.iter().enumerate().filter(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with('#')) {
                let c: Command = serde_json::from_str(line).map_err(|e| format!("line {}: {e}", n + 1))?;
                let out = d.execute(c).map_err(|e| format!("line {}: {e}", n + 1))?;
                println!("{}", serde_json::to_string(&out).unwrap_or_default());
            }
            save(&mut d, &flag(a, "-o").unwrap_or_else(|| project.to_string()))
        }
        "export" => {
            let c = convert_flags(a)?;
            save_with(&mut load(arg(a, 0, "project")?)?, arg(a, 1, "output")?, c)
        }
        "export-uv" => {
            let mut d = load(arg(a, 0, "project")?)?;
            let size = flag(a, "--size").and_then(|v| v.parse().ok()).unwrap_or(1024);
            let object = flag(a, "--object").and_then(|v| v.parse().ok()).map(modelcraft_engine::scene::ObjectId);
            if a.iter().any(|x| x == "--udim") {
                let files = d.export_uv_layout_udim(Path::new(arg(a, 1, "output")?), object, size).map_err(|e| e.to_string())?;
                for f in files {
                    println!("{}", f.display());
                }
                Ok(())
            } else {
                d.export_uv_layout(Path::new(arg(a, 1, "output")?), object, size).map_err(|e| e.to_string())
            }
        }
        "convert" => {
            let c = convert_flags(a)?;
            let input = arg(a, 0, "input")?;
            let mut d = if is_project(Path::new(input)) {
                load(input)?
            } else {
                let mut d = Document::new();
                let out = d.execute(Command::Import { path: input.to_string(), up: c.up, scale: c.scale }).map_err(|e| e.to_string())?;
                for w in out.warnings {
                    eprintln!("warning: {w}");
                }
                d
            };
            // The conversion applies on the way in; the output is written as the scene is.
            let out = if is_project(Path::new(input)) { c } else { Default::default() };
            save_with(&mut d, arg(a, 1, "output")?, out)
        }
        "recover" => {
            let p = PathBuf::from(arg(a, 0, "project")?);
            let mut d = Document::open(&p).map_err(|e| e.to_string())?;
            let n = d.recover().map_err(|e| e.to_string())?;
            d.save_as(&p).map_err(|e| e.to_string())?;
            println!("replayed {n} commands into {}", p.display());
            Ok(())
        }
        "bake" => bake(a),
        "cache-info" => {
            let p = arg(a, 0, "point cache")?;
            let bytes = std::fs::read(p).map_err(|e| format!("{p}: {e}"))?;
            let c = modelcraft_engine::formats::pointcache::PointCache::from_bytes(&bytes).map_err(|e| e.to_string())?;
            let (lo, hi) = c.positions.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
            println!("{} frames of {} vertices, from frame {} every {} frames at {} fps", c.frames(), c.vertices, c.start, c.step, c.fps);
            if !c.positions.is_empty() {
                println!("bounds {lo} .. {hi}");
            }
            Ok(())
        }
        "bench-skin" => {
            let n = |name: &str, default: usize| flag(a, name).and_then(|v| v.parse().ok()).unwrap_or(default);
            bench_skin(n("--verts", 100_000), n("--joints", 60), n("--frames", 120))
        }
        "control" => control(a.first().map(String::as_str)),
        "render-bench" => bench_cmd::run_all(&bench_cmd::parse(a)?),
        "run-script" => script_cmd::run(a, load, save),
        "macro-script" => macro_cmd::run(a),
        "batch-script" => script_cmd::batch(a, load, save),
        "render" => {
            let out = flag(a, "-o").ok_or_else(|| format!("missing -o <out.png>\n\n{}", render_cmd::USAGE))?;
            let report = render_cmd::run(arg(a, 0, "file or builtin:NAME")?, &out, &render_cmd::parse(a)?, load)?;
            println!("{report}");
            Ok(())
        }
        "serve" => serve(positional(a), flag(a, "--port").map(|p| p.parse::<u16>().map_err(|e| format!("--port: {e}"))).transpose()?),
        "call" => {
            let method = arg(a, 0, "method")?;
            let params = match a.get(1).filter(|p| !p.starts_with("--")) {
                Some(p) => serde_json::from_str(p).map_err(|e| format!("params: {e}"))?,
                None => serde_json::json!({}),
            };
            let port = flag(a, "--port").map(|p| p.parse::<u16>().map_err(|e| format!("--port: {e}"))).transpose()?;
            let mut c = mcp::app_connection(port, flag(a, "--token").as_deref())?;
            let v = c.call(craft_control::client::Request::new(method, params)).map_err(|e| e.message().to_string())?;
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            Ok(())
        }
        "mcp" => {
            let backend = if a.iter().any(|x| x == "--app") {
                let port = flag(a, "--port").map(|p| p.parse::<u16>().map_err(|e| format!("--port: {e}"))).transpose()?;
                mcp::Backend::App(Box::new(mcp::app_connection(port, None)?))
            } else {
                mcp::local(positional(a))?
            };
            mcp::serve(backend)
        }
        "bench" if a.iter().any(|x| x == "--groups") => bench_groups(flag(a, "--polys").and_then(|v| v.parse().ok()).unwrap_or(1_000_000)),
        "uv" => uv_report(&mut load(arg(a, 0, "file")?)?),
        "hair-bench" => hair_cmd::bench(a),
        "hair-export" => hair_cmd::export(a, load),
        "sim-bench" => sim_cmd::bench(a),
        "uv-bench" => uv_bench(
            flag(a, "--segments").and_then(|v| v.parse().ok()).unwrap_or(64),
            &flag(a, "--method").unwrap_or_else(|| "lscm".to_string()),
            &flag(a, "--shape").unwrap_or_else(|| "sphere".to_string()),
            &flag(a, "--cut").unwrap_or_else(|| "auto".to_string()),
            flag(a, "--relax").and_then(|v| v.parse().ok()).unwrap_or(0),
        ),
        "uv-transfer-bench" => uv_transfer_bench(flag(a, "--segments").and_then(|v| v.parse().ok()).unwrap_or(64)),
        "uv-pack-bench" => {
            uv_pack_bench(flag(a, "--islands").and_then(|v| v.parse().ok()).unwrap_or(200), flag(a, "--tiles").and_then(|v| v.parse().ok()).unwrap_or(1))
        }
        "bench" => bench(flag(a, "--polys").and_then(|v| v.parse().ok()).unwrap_or(1_000_000), flag(a, "--dabs").and_then(|v| v.parse().ok()).unwrap_or(400)),
        other => Err(format!("unknown command {other:?}\n\n{USAGE}")),
    }
}

/// `bake`: write the vertices of one object at every sampled frame to a `.mcpc` point cache.
fn bake(a: &[String]) -> Result<(), String> {
    let mut d = load(arg(a, 0, "project")?)?;
    let which = arg(a, 1, "object id or name")?;
    let out = arg(a, 2, "output .mcpc")?;
    let id = d
        .scene
        .objects()
        .iter()
        .find(|o| which.parse::<u64>().is_ok_and(|n| n == o.id.0) || o.name == which)
        .map(|o| o.id)
        .ok_or_else(|| format!("no object {which:?}"))?;
    let num = |name: &str, default: f64| -> Result<f64, String> {
        match flag(a, name) {
            Some(v) => v.parse::<f64>().map_err(|_| format!("{name} needs a number, got {v:?}")),
            None => Ok(default),
        }
    };
    let time = d.scene.anim.time;
    let (start, end, step) = (num("--start", time.start)?, num("--end", time.end)?, num("--step", 1.0)?);
    let world = a.iter().any(|x| x == "--world");
    let cache = d.bake_points(id, start, end, step, world).map_err(|e| e.to_string())?;
    let bytes = cache.to_bytes().map_err(|e| e.to_string())?;
    craft_atomic::atomic_write(Path::new(out), &bytes).map_err(|e| format!("{out}: {e}"))?;
    println!("{} frames of {} vertices written to {out}", cache.frames(), cache.vertices);
    Ok(())
}

/// A procedurally generated character stand-in: a tube of about `verts` points along Y with a
/// chain of `joints` joints up its middle, bound with heat weights, every joint keyed to sway.
/// Prints the bind time, the per-frame cost of posing and evaluating with both skinning methods,
/// and the kernels alone.
fn bench_skin(verts: usize, joints: usize, frames: usize) -> Result<(), String> {
    use modelcraft_engine::animation::skin;
    use modelcraft_engine::{BindMethod, RigCommand};
    let joints = joints.clamp(2, 1000);
    let frames = frames.clamp(2, 100_000);
    let mut d = Document::new();
    let run = |d: &mut Document, c: Command| d.execute(c).map_err(|e| e.to_string());
    let t = Instant::now();
    let (mesh, skin_id, _) = modelcraft_engine::rig::bench_character(&mut d, verts, joints, frames, BindMethod::Heat).map_err(|e| e.to_string())?;
    let count = d.eval.evaluate(&d.scene, mesh).mesh.vertex_count();
    println!("mesh: {count} vertices, {joints} joints; built and bound with heat weights (4 influences) in {:.0} ms", ms(t));
    println!("animation: {} curves on {joints} joints, {frames} frames at 24 fps; {} threads", d.scene.anim.tracks().len(), rayon_threads());
    for dq in [false, true] {
        run(&mut d, Command::Rig(RigCommand::SetSkin { mesh, dual_quaternion: dq }))?;
        let (mut pose, mut eval) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
        let mut checksum = 0.0f64;
        for f in 0..frames {
            let t = Instant::now();
            d.set_frame(f as f64);
            pose.push(ms(t));
            let t = Instant::now();
            let m = d.eval.evaluate(&d.scene, mesh).mesh;
            eval.push(ms(t));
            checksum += m.positions().first().map_or(0.0, |p| p.x as f64);
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
        let total = mean(&pose) + mean(&eval);
        println!(
            "{}: pose (curves → joints) {:.3} ms, evaluate (skin {count} points) mean {:.2} ms p95 {:.2} ms → {:.0} evaluated frames per second (checksum {checksum:.3})",
            if dq { "dual quaternion" } else { "linear blend   " },
            mean(&pose),
            mean(&eval),
            pct(&mut eval, 0.95),
            1000.0 / total.max(1e-9)
        );
    }
    // The kernels alone, on the same data.
    let ObjectKind::Skin { skin: bound } = d.scene.get(skin_id).map(|o| o.kind.clone()).ok_or("no skin")? else { return Err("no skin".into()) };
    let scene = d.scene.clone();
    let mut hidden = scene.clone();
    if let Ok(o) = hidden.edit(skin_id) {
        o.visible = false;
    }
    let rest = d.eval.evaluate(&hidden, mesh).mesh.positions().to_vec();
    let world: Vec<glam::Mat4> = bound.joints.iter().map(|j| scene.world_matrix(*j)).collect();
    let mats = skin::skin_matrices(&world, &bound.inverse_bind);
    let mut out = Vec::new();
    type Kernel = fn(&[Vec3], &skin::SkinWeights, &[glam::Mat4], &mut Vec<Vec3>, bool);
    for (name, f) in [("linear blend", skin::linear_blend_with as Kernel), ("dual quaternion", skin::dual_quaternion_with)] {
        let mut per = [0.0; 2];
        for (slot, parallel) in per.iter_mut().zip([false, true]) {
            // The best of five batches: the machine is rarely idle.
            *slot = (0..5)
                .map(|_| {
                    let t = Instant::now();
                    for _ in 0..10 {
                        f(&rest, &bound.weights, &mats, &mut out, parallel);
                    }
                    ms(t) / 10.0
                })
                .fold(f64::MAX, f64::min);
        }
        println!("kernel {name}: {:.2} ms on one core, {:.2} ms on all, per {count} points", per[0], per[1]);
    }
    Ok(())
}

fn info(d: &mut Document) {
    let anim = &d.scene.anim;
    if !anim.is_empty() {
        let keys: usize = anim.tracks().iter().map(|t| t.curve.len()).sum();
        println!(
            "animation: {} curves, {keys} keys, frames {}..{} at {} fps, current frame {}",
            anim.tracks().len(),
            anim.time.start,
            anim.time.end,
            anim.time.fps,
            anim.frame
        );
        for t in anim.tracks() {
            println!("  curve id={} {} keys={}", t.target.0, t.path, t.curve.len());
        }
    }
    let order = d.scene.tree_order();
    for (id, depth) in order {
        let Some(o) = d.scene.get(id).cloned() else { continue };
        let e = d.eval.evaluate(&d.scene, id);
        let m = &e.mesh;
        println!(
            "{}{} [{}] id={} steps={} verts={} faces={} tris={}{}",
            "  ".repeat(depth),
            o.name,
            o.kind.type_name(),
            id.0,
            o.steps().len(),
            m.vertex_count(),
            m.face_count(),
            m.triangle_count(),
            if e.errors.is_empty() { String::new() } else { format!(" errors={}", e.errors.len()) }
        );
    }
}

fn validate(d: &mut Document) -> Result<(), String> {
    let ids: Vec<_> = d.scene.objects().iter().map(|o| o.id).collect();
    let mut bad = 0;
    for id in ids {
        let e = d.eval.evaluate(&d.scene, id);
        if let Err(err) = e.mesh.validate() {
            println!("object {}: invalid mesh: {err}", id.0);
            bad += 1;
        }
        for s in e.errors {
            println!("object {}: step {}: {}", id.0, s.step, s.message);
        }
    }
    println!("{} objects checked, {bad} invalid", d.scene.len());
    if bad == 0 { Ok(()) } else { Err("invalid meshes found".into()) }
}

/// The first argument that is not a flag (or a flag's value).
fn positional(a: &[String]) -> Option<&str> {
    let mut skip = false;
    for x in a {
        if skip {
            skip = false;
            continue;
        }
        if matches!(x.as_str(), "--port" | "--token") {
            skip = true;
            continue;
        }
        if !x.starts_with("--") {
            return Some(x);
        }
    }
    None
}

/// The agent channel: newline-delimited JSON on stdin, one reply per line on stdout. A line is an
/// engine command (`{"cmd": …}`), one of the extra verbs `{"cmd":"save","path":…}`,
/// `{"cmd":"info"}`, `{"cmd":"export","path":…}`, or a control request `{"method": …, "params": …}`
/// as the socket takes it.
fn control(project: Option<&str>) -> Result<(), String> {
    let mut d = match project {
        Some(p) if Path::new(p).exists() => load(p)?,
        _ => Document::new(),
    };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    let mut script = modelcraft_script::channel::ScriptChannel::new();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        if line.len() > channel::MAX_LINE {
            let _ = writeln!(out, "{{\"ok\":false,\"error\":\"line too long\"}}");
            continue;
        }
        let reply = match serde_json::from_str::<serde_json::Value>(&line) {
            Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            Ok(v) => {
                let (method, params) = match (v.get("method").and_then(|m| m.as_str()), v.get("cmd").and_then(|c| c.as_str())) {
                    (Some(m), _) => (m.to_string(), v.get("params").cloned().unwrap_or(serde_json::json!({}))),
                    (None, Some("save")) => ("document.save".into(), v.clone()),
                    (None, Some("export")) => ("document.export".into(), v.clone()),
                    (None, Some("info")) => ("document.info".into(), serde_json::json!({})),
                    _ => ("engine.execute".into(), v.clone()),
                };
                let r = channel::handle_with(&mut d, &method, &params, &mut |doc, m, p| script.handle(doc, &mut Vec::new(), m, p));
                // A CLI save is final: no journal left behind.
                if method == "document.save" {
                    d.detach_journal();
                }
                r
            }
        };
        writeln!(out, "{reply}").map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The control channel on a loopback socket, answering until the process is stopped.
fn serve(project: Option<&str>, port: Option<u16>) -> Result<(), String> {
    let mut d = match project {
        Some(p) if Path::new(p).exists() => load(p)?,
        Some(p) => {
            let mut d = Document::new();
            d.path = Some(PathBuf::from(p));
            d
        }
        None => Document::new(),
    };
    let l = channel::listen(port, None, || {}).map_err(|e| format!("cannot listen: {e}"))?;
    match &l.token_file {
        Some(f) => println!("listening on 127.0.0.1:{} (token in {})", l.port, f.display()),
        None => println!("listening on 127.0.0.1:{} (token from MODELCRAFT_CONTROL_TOKEN)", l.port),
    }
    let _ = std::io::stdout().flush();
    let mut script = modelcraft_script::channel::ScriptChannel::new();
    while let Ok(req) = l.requests.recv() {
        let save = req.method == "document.save";
        channel::answer_with(&mut d, req, &mut |doc, m, p| script.handle(doc, &mut Vec::new(), m, p));
        if save {
            d.detach_journal();
        }
    }
    Ok(())
}

fn distortion_json(d: &modelcraft_engine::uv::Distortion) -> serde_json::Value {
    serde_json::json!({
        "triangles": d.triangles, "flipped": d.flipped,
        "mean_angle_error_deg": d.mean_angle_error_deg, "max_angle_error_deg": d.max_angle_error_deg,
        "mean_stretch": d.mean_stretch, "max_stretch": d.max_stretch,
        "area_rms_log": d.area_rms_log, "min_area_ratio": d.min_area_ratio, "max_area_ratio": d.max_area_ratio,
    })
}

/// One JSON line per mesh object: its UV islands (cut along the UV discontinuities) and the
/// distortion of its UVs. Objects without UVs report `"uvs":false`.
fn uv_report(d: &mut Document) -> Result<(), String> {
    use modelcraft_engine::uv::{Charts, SeamSet, measure};
    let ids: Vec<_> = d.scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Mesh { .. })).map(|o| o.id).collect();
    for id in ids {
        let m = d.eval.evaluate(&d.scene, id).mesh;
        let name = d.scene.get(id).map(|o| o.name.clone()).unwrap_or_default();
        if !m.has_uvs() {
            println!("{}", serde_json::json!({ "id": id.0, "name": name, "uvs": false }));
            continue;
        }
        let charts = Charts::build(&m, &SeamSet::from_uv_discontinuities(&m)).map_err(|e| e.to_string())?;
        let (dist, _) = measure(&m);
        println!("{}", serde_json::json!({ "id": id.0, "name": name, "uvs": true, "islands": charts.islands.len(), "distortion": distortion_json(&dist) }));
    }
    Ok(())
}

/// Unwrap a welded shape (no seams unless the shape is developable: the automatic cut gives a
/// closed one a path) with each solver asked for and report time and distortion. Reproduces the
/// numbers in `docs/performance.md`.
fn uv_bench(segments: u32, method: &str, shape: &str, cut: &str, relax: u32) -> Result<(), String> {
    use modelcraft_engine::ops::{edit, primitives::Primitive};
    use modelcraft_engine::uv::{RelaxMode, RelaxOptions, SeamSet, UnwrapMethod, UnwrapOptions, measure, relax_corners, unwrap_mesh};
    let segments = segments.clamp(4, 512);
    let rings = (segments / 2).max(2);
    let prim = match shape {
        "sphere" => Primitive::Sphere { radius: 1.0, segments, rings },
        "tube" => Primitive::Cylinder { radius: 1.0, height: 2.0, sides: segments, rings, caps: false },
        "cone" => Primitive::Cone { radius: 1.0, height: 1.5, sides: segments, caps: false },
        "torus" => Primitive::Torus { radius: 1.0, tube: 0.35, segments, sides: (segments / 2).max(3) },
        "roundcube" => Primitive::RoundCube { radius: 1.0, segments: (segments / 4).max(2) },
        other => return Err(format!("unknown shape `{other}` (sphere, tube, cone, torus, roundcube)")),
    };
    let methods: Vec<(&str, UnwrapMethod)> = match method {
        "lscm" => vec![("LSCM", UnwrapMethod::Lscm)],
        "abf" => vec![("ABF", UnwrapMethod::Abf)],
        "both" => vec![("LSCM", UnwrapMethod::Lscm), ("ABF", UnwrapMethod::Abf)],
        other => return Err(format!("unknown method `{other}` (lscm, abf, both)")),
    };
    let t = Instant::now();
    let raw = prim.build().map_err(|e| e.to_string())?;
    let mesh = edit::weld(&raw, 1e-5).map_err(|e| e.to_string())?;
    println!("{shape} {segments}: {} faces, {} vertices, built in {:.0} ms", mesh.face_count(), mesh.vertex_count(), ms(t));
    let mut seams = SeamSet::new();
    match cut {
        "auto" => {}
        // One seam from the top to the bottom of the shape (pole to pole on a sphere, a line down
        // a tube, apex to rim on a cone): a single interior everywhere else.
        "meridian" => {
            let key = |v: u32| f64::from(mesh.position(v).y) * 1000.0 - f64::from(mesh.position(v).z);
            let ids = 0..mesh.vertex_count() as u32;
            let top = ids.clone().max_by(|a, b| key(*a).total_cmp(&key(*b)));
            let bottom = ids.min_by(|a, b| key(*a).total_cmp(&key(*b)));
            if let (Some(top), Some(bottom)) = (top, bottom) {
                seams.mark_path(&mesh, top, bottom).ok_or("no seam path between the ends of the shape")?;
            }
        }
        other => return Err(format!("unknown cut `{other}` (auto, meridian)")),
    }
    for (label, m) in methods {
        let t = Instant::now();
        let (out, res) = unwrap_mesh(&mesh, &seams, &UnwrapOptions { method: m, ..UnwrapOptions::default() }).map_err(|e| e.to_string())?;
        let solve = ms(t);
        let isl = res.islands.first();
        match isl.and_then(|i| i.abf) {
            Some(a) => println!(
                "unwrap {label} (auto cut of {} edges): {solve:.0} ms, {} Newton iterations, constraint residual {:.1e}, converged {}, fell back {}",
                res.cut.edges_added,
                a.iterations,
                a.residual,
                a.converged,
                isl.is_some_and(|i| i.abf_fell_back())
            ),
            None => println!(
                "unwrap {label} (auto cut of {} edges): {solve:.0} ms, {} CG iterations, residual {:.1e}, converged {}",
                res.cut.edges_added,
                isl.map_or(0, |i| i.iterations),
                isl.map_or(0.0, |i| i.residual),
                isl.is_some_and(|i| i.converged)
            ),
        }
        let (dist, _) = measure(&out);
        println!("{}", distortion_json(&dist));
        if relax > 0 {
            for (label, mode) in [("rigid", RelaxMode::Rigid), ("conformal", RelaxMode::Conformal)] {
                let t = Instant::now();
                let (relaxed, rep) = relax_corners(&out, &[], &RelaxOptions { iterations: relax, mode }).map_err(|e| e.to_string())?;
                println!("relax {label} x{relax}: {:.0} ms, energy {:.4e} -> {:.4e}, {} islands", ms(t), rep.energy_before, rep.energy_after, rep.islands);
                println!("{}", distortion_json(&measure(&relaxed).0));
            }
        }
    }
    Ok(())
}

/// Unwrap a sphere of N×N/2 quads, then give a sphere of 1.5 N × 0.75 N quads its UVs by
/// closest-point lookup, per island and plain.
fn uv_transfer_bench(segments: u32) -> Result<(), String> {
    use modelcraft_engine::ops::{edit, primitives::Primitive};
    use modelcraft_engine::uv::{SeamSet, TransferOptions, UnwrapOptions, transfer_uvs, unwrap_mesh};
    let segments = segments.clamp(4, 340);
    let make = |n: u32| -> Result<_, String> {
        let raw = Primitive::Sphere { radius: 1.0, segments: n, rings: (n / 2).max(2) }.build().map_err(|e| e.to_string())?;
        edit::weld(&raw, 1e-5).map_err(|e| e.to_string())
    };
    let source = make(segments)?;
    let (source, _) = unwrap_mesh(&source, &SeamSet::new(), &UnwrapOptions::default()).map_err(|e| e.to_string())?;
    let target = make(segments * 3 / 2)?;
    println!("source {} faces, target {} faces ({} corners)", source.face_count(), target.face_count(), target.corner_count());
    for (label, per_island) in [("per island", true), ("plain", false)] {
        let t = Instant::now();
        let (_, rep) = transfer_uvs(&source, &target, &TransferOptions { max_distance: None, per_island }).map_err(|e| e.to_string())?;
        println!("transfer {label}: {:.0} ms, {} corners mapped, mean distance {:.2e}, max {:.2e}", ms(t), rep.mapped, rep.mean_distance, rep.max_distance);
    }
    Ok(())
}

/// Pack N random rectangles (fixed seed) with the default options for each rotation mode.
fn uv_pack_bench(n: usize, tiles: u32) -> Result<(), String> {
    use modelcraft_engine::uvpack::{Density, PackIsland, PackOptions, Rotation, pack};
    let n = n.clamp(1, 100_000);
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut rnd = move || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    let islands: Vec<PackIsland> = (0..n)
        .map(|_| {
            let (w, h) = (0.05 + 1.5 * rnd() * rnd(), 0.05 + 1.5 * rnd() * rnd());
            PackIsland {
                triangles: vec![
                    [glam::Vec2::ZERO, glam::Vec2::new(w, 0.0), glam::Vec2::new(w, h)],
                    [glam::Vec2::ZERO, glam::Vec2::new(w, h), glam::Vec2::new(0.0, h)],
                ],
                area_3d: w * h,
            }
        })
        .collect();
    // The margin is per island on every side, so many islands need a thinner one to fit at all.
    let margin = if n <= 1000 { 0.004 } else { 0.0005 };
    if tiles > 1 {
        use modelcraft_engine::uvpack::{UdimOptions, UdimTiles, pack_udim};
        for rotation in [Rotation::None, Rotation::Fit] {
            let t = Instant::now();
            let r =
                pack_udim(&islands, &UdimOptions { margin, rotation, density: Density::Uniform, tiles: UdimTiles::Count(tiles) }).map_err(|e| e.to_string())?;
            let (lo, hi) = r.utilisation.iter().fold((f32::MAX, f32::MIN), |(l, h), u| (l.min(*u), h.max(*u)));
            println!(
                "{n} islands over {tiles} tiles, rotation {rotation:?}: {:.0} ms, scale {:.4}, utilisation {:.1} to {:.1} % per tile, {} tiles used",
                ms(t),
                r.scale,
                lo * 100.0,
                hi * 100.0,
                r.tiles_used.len()
            );
        }
        return Ok(());
    }
    for rotation in [Rotation::None, Rotation::Quarter, Rotation::Fit] {
        let t = Instant::now();
        let r = pack(&islands, &PackOptions { margin, rotation, density: Density::Uniform }).map_err(|e| e.to_string())?;
        println!(
            "{n} islands, rotation {rotation:?}: {:.0} ms, utilisation {:.1} %, scale {:.4}, work {}{}",
            ms(t),
            r.utilisation * 100.0,
            r.scale,
            r.work,
            if r.shelf_fallback { " (shelf packer)" } else { "" }
        );
    }
    Ok(())
}

fn pct(v: &mut [f64], q: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(((v.len() as f64 - 1.0) * q).round().max(0.0) as usize).copied().unwrap_or(0.0)
}

/// How many threads the parallel kernels use.
fn rayon_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Sculpt latency on a dense quad sphere: build times, per-dab time for each brush, and, when a
/// GPU is present, the chunked upload and a full 1920×1080 draw of the mesh.
/// Time the patch, visibility, region, occlusion, shell and trim operations on a quad sphere.
fn bench_groups(polys: u32) -> Result<(), String> {
    use modelcraft_engine::sculpt::groups;
    let seg = ((polys as f64 / 6.0).sqrt().ceil() as u32).clamp(4, 1632);
    let mesh = modelcraft_engine::ops::primitives::Primitive::RoundCube { radius: 1.0, segments: seg }.build().map_err(|e| e.to_string())?;
    let mut s = SculptMesh::new(&mesh, None, None);
    println!("mesh: {} quads, {} triangles, {} vertices", mesh.face_count(), s.triangle_count(), s.vertex_count());
    let cam = modelcraft_render::Camera { distance: 3.2, ..Default::default() };
    let region = modelcraft_engine::sculpt::ScreenRegion {
        view_proj: cam.view_proj(1.0),
        points: (0..64).map(|k| glam::Vec2::from_angle(k as f32 / 64.0 * std::f32::consts::TAU) * 0.3).collect(),
        front_only: true,
        eye: cam.eye(),
    };
    if std::env::var("MODELCRAFT_BENCH_ONLY").as_deref() == Ok("decimate") {
        return bench_decimate(&mesh);
    }
    let t = Instant::now();
    let (by_angle, count) = groups::patches_by_normal(&mesh, 30.0);
    println!("patches by angle (30 degrees): {count} patches, {:.0} ms", ms(t));
    let t = Instant::now();
    let islands = groups::patches_by_uv_island(&mesh).map(|p| p.1);
    println!("patches by UV island: {islands:?}, {:.0} ms", ms(t));
    let t = Instant::now();
    s.set_face_patches(&by_angle);
    println!("patches into the session: {:.0} ms", ms(t));
    let t = Instant::now();
    let inside = groups::faces_in_region(&mesh, &region);
    println!("faces inside a 64-corner lasso: {} faces, {:.0} ms", inside.iter().filter(|x| **x).count(), ms(t));
    let t = Instant::now();
    s.set_face_hidden(&inside);
    println!("hide them in the session: {:.0} ms", ms(t));
    s.set_face_hidden(&[]);
    let t = Instant::now();
    let masked = s.mask_region(&region, 1.0).indices.len();
    println!("mask the same lasso: {masked} vertices, {:.0} ms", ms(t));
    let t = Instant::now();
    let shell = groups::extract_shell(&mesh, &s.mask, 0.02).map_err(|e| e.to_string())?;
    println!("shell from that mask: {} faces, {:.0} ms", shell.face_count(), ms(t));
    let t = Instant::now();
    let moved = s.clip_plane(Vec3::new(0.0, 0.5, 0.0), Vec3::Y).indices.len();
    println!("clip to a plane: {moved} vertices, {:.0} ms", ms(t));
    let t = Instant::now();
    let trimmed = modelcraft_engine::ops::trim::trim_plane(&mesh, Vec3::new(0.0, 0.3, 0.0), Vec3::new(0.2, 1.0, 0.1), true).map_err(|e| e.to_string())?;
    println!("trim by a plane with a cap: {} faces left, {:.0} ms", trimmed.face_count(), ms(t));
    let t = Instant::now();
    let changed = s.mask_by_occlusion().indices.len();
    let rays = (groups::OCCLUSION_RAY_BUDGET / s.vertex_count().max(1)).clamp(4, 24);
    println!("occlusion mask ({rays} rays per vertex): {changed} vertices changed, {:.0} ms", ms(t));
    bench_decimate(&mesh)
}

/// Decimate to a tenth, then project a plain ball of the same count onto a bumpy one.
fn bench_decimate(mesh: &modelcraft_engine::mesh::PolyMesh) -> Result<(), String> {
    let t = Instant::now();
    let low = modelcraft_engine::ops::decimate::decimate(mesh, 0.1).map_err(|e| e.to_string())?;
    let far = low.positions().iter().map(|p| (p.length() - 1.0).abs()).fold(0.0f32, f32::max);
    let chord = (0..low.face_count() as u32).map(|f| 1.0 - low.face_center(f).length()).fold(0.0f32, f32::max);
    println!("decimate to 10 %: {} triangles, {:.0} ms; vertices within {far:.1e} of the sphere, faces within {chord:.1e}", low.face_count(), ms(t));
    let mut bumpy = mesh.clone();
    bumpy.map_positions(|_, p| p * (1.0 + 0.03 * (p.x * 40.0).sin() * (p.y * 40.0).cos()));
    let source = SculptMesh::new(&bumpy, None, None);
    let mut target = SculptMesh::new(mesh, None, None);
    let t = Instant::now();
    let moved = target.project_onto(&source, 0.2).indices.len();
    let worst = target.positions.iter().map(|p| source.closest_point(*p, 0.05).map(|q| q.distance(*p)).unwrap_or(0.05)).fold(0.0f32, f32::max);
    println!("project onto a detailed mesh of the same size: {moved} vertices, {:.0} ms; worst distance left {worst:.1e}", ms(t));
    Ok(())
}

fn bench(polys: u32, dabs: usize) -> Result<(), String> {
    let seg = ((polys as f64 / 6.0).sqrt().ceil() as u32).clamp(4, 1632);
    let t = Instant::now();
    let mesh = modelcraft_engine::ops::primitives::Primitive::RoundCube { radius: 1.0, segments: seg }.build().map_err(|e| e.to_string())?;
    let build_ms = ms(t);
    let t = Instant::now();
    let mut s = SculptMesh::new(&mesh, None, None);
    let session_ms = ms(t);
    println!(
        "mesh: {} quads, {} triangles, {} vertices; generate {:.0} ms, sculpt session (normals, adjacency, BVH) {:.0} ms",
        mesh.face_count(),
        s.triangle_count(),
        s.vertex_count(),
        build_ms,
        session_ms
    );
    let gpu = modelcraft_render::gpu::headless();
    let mut gpu_state = gpu.as_ref().map(|(device, _, name)| {
        let data = modelcraft_render::buffers::shared_data(&s.positions, &s.normals, Some(&s.mask), s.triangles());
        let g = modelcraft_render::gpu::GpuMesh::new(device, &data);
        let target = modelcraft_render::gpu::Target::new(device, [1920, 1080]);
        let r = modelcraft_render::gpu::Renderer::new(device);
        println!("gpu: {name}");
        (g, target, r)
    });
    let _ = s.take_dirty_chunks();
    let radius = 0.08f32;
    for kind in modelcraft_engine::sculpt::brush::KINDS.iter().copied() {
        let mut b = BrushSettings::new(kind, radius);
        b.symmetry.x = true;
        s.begin_stroke(b);
        let (mut dab_t, mut up_t, mut draw_t, mut touched) = (Vec::new(), Vec::new(), Vec::new(), 0usize);
        for i in 0..dabs {
            let a = i as f32 * 0.01;
            let c = Vec3::new(a.cos() * 0.6, 0.55, a.sin() * 0.6).normalize();
            let t = Instant::now();
            s.dab(Dab { center: c, view_dir: -c, pressure: 1.0, grab_delta: Vec3::new(0.0, 0.002, 0.0), scale: 1.0 });
            dab_t.push(ms(t));
            if let (Some((g, target, r)), Some((device, queue, _))) = (gpu_state.as_mut(), gpu.as_ref()) {
                let t = Instant::now();
                for (start, len) in s.take_dirty_chunks() {
                    touched += len;
                    let v: Vec<_> = (start..start + len).map(|k| modelcraft_render::buffers::vertex_at(&s.positions, &s.normals, Some(&s.mask), k)).collect();
                    g.update(queue, start, &v);
                }
                up_t.push(ms(t));
                if i % 10 == 0 {
                    let cam = modelcraft_render::Camera { distance: 3.2, ..Default::default() };
                    let frame = modelcraft_render::gpu::Frame {
                        view_proj: cam.view_proj(1920.0 / 1080.0),
                        eye: cam.eye(),
                        clear: [0.1, 0.1, 0.1, 1.0],
                        draws: vec![modelcraft_render::gpu::DrawItem {
                            mesh: g,
                            model: glam::Mat4::IDENTITY,
                            color: [0.7; 4],
                            selected: false,
                            show_mask: true,
                            patch_tint: false,
                            uv_checker: false,
                            roughness: 0.5,
                        }],
                        lines: &[],
                        overlay: &[],
                        hair: Vec::new(),
                    };
                    let t = Instant::now();
                    r.render(device, queue, target, &frame);
                    let _ = device.poll(wgpu::PollType::wait_indefinitely());
                    draw_t.push(ms(t));
                }
            }
        }
        let t = Instant::now();
        let undo = s.end_stroke().map(|(u, _)| u.indices.len()).unwrap_or(0);
        let end_ms = ms(t);
        if kind == BrushKind::Mask {
            // Leave the surface free for the brushes that follow.
            s.mask_all(0.0);
            let _ = s.take_dirty_chunks();
        }
        let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
        println!(
            "{:<12} dab mean {:.3} ms p95 {:.3} ms | upload mean {:.3} ms ({} verts/dab) | draw+wait mean {:.2} ms | stroke touched {} verts, stroke end {:.1} ms",
            kind.label(),
            mean(&dab_t),
            pct(&mut dab_t, 0.95),
            mean(&up_t),
            touched / dabs.max(1),
            mean(&draw_t),
            undo,
            end_ms
        );
    }
    let t = Instant::now();
    s.refit();
    println!("full bvh refit {:.1} ms (a stroke end refits only what moved)", ms(t));
    if polys > 2_000_000 {
        println!("(history, detail levels, live detail and even remesh are measured at --polys 1000000; skipped at this size)");
        return Ok(());
    }
    // A full scene-evaluation round trip on the same density through the engine.
    let mut d = Document::new();
    let id = d
        .execute(Command::AddPrimitive { kind: "round_cube".into(), name: None, parent: None })
        .map_err(|e| e.to_string())?
        .created
        .first()
        .copied()
        .ok_or("no object")?;
    let t = Instant::now();
    d.execute(Command::SetPrimitive { id, primitive: modelcraft_engine::ops::primitives::Primitive::RoundCube { radius: 1.0, segments: seg / 2 } })
        .map_err(|e| e.to_string())?;
    d.execute(Command::AddStep { id, op: modelcraft_engine::scene::StepOp::Subdivide { levels: 1 } }).map_err(|e| e.to_string())?;
    let faces = d.eval.evaluate(&d.scene, id).mesh.face_count();
    println!("history: {}-quad base + 1 subdivision level = {} quads in {:.0} ms", 6 * (seg / 2) * (seg / 2), faces, ms(t));
    let _ = (Scene::new(), ObjectKind::Group);
    if polys > 2_000_000 {
        println!("(detail levels, live detail and even remesh are measured at --polys 1000000; skipped at this size)");
        return Ok(());
    }
    detail_bench(&mut d, id)
}

/// Detail levels and shape layers on the bench object: sculpt, step up to the full density,
/// sculpt there, step down and up again, and report the times and the round-trip error.
fn detail_bench(d: &mut Document, id: modelcraft_engine::scene::ObjectId) -> Result<(), String> {
    use modelcraft_engine::{DetailOp, ShapeOp};
    let run = |d: &mut Document, c: Command| d.execute(c).map(|_| ()).map_err(|e| e.to_string());
    run(d, Command::RemoveStep { id, index: 0 })?;
    run(d, Command::BeginSculpt { id })?;
    let stroke = |kind: BrushKind| {
        let mut b = BrushSettings::new(kind, 0.3);
        b.symmetry.x = true;
        let dabs = (0..40).map(|i| {
            let a = i as f32 * 0.05;
            let c = Vec3::new(a.cos() * 0.6, 0.55, a.sin() * 0.6).normalize();
            Dab { center: c, view_dir: -c, pressure: 1.0, grab_delta: Vec3::ZERO, scale: 1.0 }
        });
        modelcraft_engine::sculpt::Stroke { brush: b, dabs: dabs.collect() }
    };
    let count = |d: &Document| d.sculpt.as_ref().map(|s| s.mesh.vertex_count()).unwrap_or(0);
    let coarse = count(d);
    let t = Instant::now();
    run(d, Command::SculptDetail { id, op: DetailOp::Up })?;
    let up_ms = ms(t);
    run(d, Command::SculptStroke { id, stroke: stroke(BrushKind::Draw) })?;
    let fine: Vec<Vec3> = d.sculpt.as_ref().map(|s| s.mesh.positions.clone()).unwrap_or_default();
    let t = Instant::now();
    run(d, Command::SculptDetail { id, op: DetailOp::Down })?;
    let down_ms = ms(t);
    run(d, Command::SculptStroke { id, stroke: stroke(BrushKind::Inflate) })?;
    run(d, Command::Undo)?;
    let t = Instant::now();
    run(d, Command::SculptDetail { id, op: DetailOp::Up })?;
    let again_ms = ms(t);
    let err = d.sculpt.as_ref().map(|s| s.mesh.positions.iter().zip(&fine).map(|(a, b)| a.distance(*b)).fold(0.0f32, f32::max)).unwrap_or(f32::NAN);
    println!(
        "detail levels: {coarse} -> {} vertices: up {up_ms:.0} ms, down {down_ms:.0} ms, up with stored detail {again_ms:.0} ms; round-trip error {err:.2e} (unit sphere)",
        count(d)
    );
    let t = Instant::now();
    run(d, Command::SculptShape { id, op: ShapeOp::Add { name: None } })?;
    let add_ms = ms(t);
    run(d, Command::SculptStroke { id, stroke: stroke(BrushKind::Inflate) })?;
    let t = Instant::now();
    run(d, Command::SculptShape { id, op: ShapeOp::Strength { index: 0, strength: 0.5 } })?;
    println!("shape layers at {} vertices: add {add_ms:.0} ms, change strength {:.0} ms", count(d), ms(t));
    live_bench()
}

/// Live detail (a stroke that splits triangles as it goes) and even remesh, on quad spheres.
fn live_bench() -> Result<(), String> {
    use modelcraft_engine::ops::remesh;
    let ball = |seg: u32| modelcraft_engine::ops::primitives::Primitive::RoundCube { radius: 1.0, segments: seg }.build().map_err(|e| e.to_string());
    for (seg, radius, edge) in [(64u32, 0.2f32, 0.01f32), (288, 0.1, 0.004)] {
        let mesh = ball(seg)?;
        let mut s = SculptMesh::new(&mesh, None, None);
        let before = s.triangle_count();
        let mut b = BrushSettings::new(BrushKind::Draw, radius);
        b.live_detail = Some(edge);
        s.begin_stroke(b);
        let mut times = Vec::new();
        for i in 0..200 {
            let a = i as f32 * 0.01;
            let c = Vec3::new(a.cos() * 0.6, 0.55, a.sin() * 0.6).normalize();
            let t = Instant::now();
            s.dab(Dab { center: c, view_dir: -c, pressure: 1.0, grab_delta: Vec3::ZERO, scale: 1.0 });
            times.push(ms(t));
        }
        let t = Instant::now();
        let undo = s.end_stroke().map(|(u, _)| u).unwrap_or_default();
        let end_ms = ms(t);
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        let after = s.triangle_count();
        let t = Instant::now();
        s.apply_undo(&undo, false);
        println!(
            "live detail: {before} -> {after} triangles over 200 dabs (radius {radius}, edge {edge}): dab mean {mean:.2} ms p95 {:.2} ms, stroke end {end_ms:.0} ms, undo {:.0} ms",
            pct(&mut times, 0.95),
            ms(t)
        );
    }
    for (seg, cell) in [(64u32, 0.02f32), (410, 0.01), (410, 0.006)] {
        let mesh = ball(seg)?;
        let t = Instant::now();
        let out = remesh::even_remesh(&mesh, cell).map_err(|e| e.to_string())?;
        println!(
            "even remesh: {} -> {} quads at cell {cell}: {:.0} ms, volume kept to {:.3} %",
            mesh.face_count(),
            out.face_count(),
            ms(t),
            (remesh::volume(&out) / remesh::volume(&mesh) - 1.0).abs() * 100.0
        );
    }
    Ok(())
}
