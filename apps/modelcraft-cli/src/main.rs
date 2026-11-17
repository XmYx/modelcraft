//! `modelcraft-cli`: ModelCraft without the window. Every subcommand runs engine commands, the
//! same ones the app's menus and shortcuts run.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod bench_cmd;
mod render_cmd;
mod script_cmd;

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use glam::Vec3;
use modelcraft_engine::scene::{ObjectKind, Scene};
use modelcraft_engine::sculpt::{BrushKind, BrushSettings, Dab, SculptMesh};
use modelcraft_engine::{COMMANDS, Command, Document};

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
  export <project> <out.glb|obj|stl>
  convert <in> <out>                any readable file to .glb, .obj, .stl or .modelcraft
  recover <project>                 replay the crash journal next to a project and save
  control [project]                 JSON-lines control channel on stdin/stdout:
                                    one command per line in, {\"ok\":…} per line out
  bake <project> <object id or name> <out.mcpc> [--start F] [--end F] [--step F] [--world]
                                    bake an object's vertices over the frames into a point cache
  cache-info <file.mcpc>            print a point cache's header and bounds
  bench [--polys N] [--dabs N]      sculpt latency and GPU upload/draw times on a dense mesh
  -h, --help | -V, --version
";

fn usage() -> String {
    format!("{USAGE}\nScripts (docs/scripting.md):\n{}\n\nRendering:\n{}\n{}\n", script_cmd::USAGE, render_cmd::USAGE, bench_cmd::USAGE)
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
fn load(path: &str) -> Result<Document, String> {
    let p = Path::new(path);
    if is_project(p) {
        let mut d = Document::open(p).map_err(|e| e.to_string())?;
        if d.pending_recovery > 0 {
            eprintln!("note: {} unsaved commands wait in the crash journal; `modelcraft-cli recover` replays them", d.pending_recovery);
        }
        d.detach_journal();
        Ok(d)
    } else {
        let mut d = Document::new();
        let out = d.execute(Command::Import { path: path.to_string() }).map_err(|e| e.to_string())?;
        for w in out.warnings {
            eprintln!("warning: {w}");
        }
        Ok(d)
    }
}

fn save(d: &mut Document, out: &str) -> Result<(), String> {
    let p = Path::new(out);
    if is_project(p) { d.save_as(p) } else { d.export(p) }.map_err(|e| e.to_string())?;
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
        "export" => save(&mut load(arg(a, 0, "project")?)?, arg(a, 1, "output")?),
        "convert" => save(&mut load(arg(a, 0, "input")?)?, arg(a, 1, "output")?),
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
        "control" => control(a.first().map(String::as_str)),
        "render-bench" => bench_cmd::run_all(&bench_cmd::parse(a)?),
        "run-script" => script_cmd::run(a, load, save),
        "render" => {
            let out = flag(a, "-o").ok_or_else(|| format!("missing -o <out.png>\n\n{}", render_cmd::USAGE))?;
            let report = render_cmd::run(arg(a, 0, "file or builtin:NAME")?, &out, &render_cmd::parse(a)?, load)?;
            println!("{report}");
            Ok(())
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

/// The agent channel: newline-delimited JSON commands on stdin, one reply per line on stdout.
/// Extra verbs: `{"cmd":"save","path":…}`, `{"cmd":"info"}`, `{"cmd":"export","path":…}`.
fn control(project: Option<&str>) -> Result<(), String> {
    let mut d = match project {
        Some(p) if Path::new(p).exists() => load(p)?,
        _ => Document::new(),
    };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        if line.len() > 64 << 20 {
            let _ = writeln!(out, "{{\"ok\":false,\"error\":\"line too long\"}}");
            continue;
        }
        let reply = match serde_json::from_str::<serde_json::Value>(&line) {
            Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            Ok(v) => match v.get("cmd").and_then(|c| c.as_str()) {
                Some("save") | Some("export") => {
                    let path = v.get("path").and_then(|p| p.as_str()).unwrap_or_default().to_string();
                    match save(&mut d, &path) {
                        Ok(()) => serde_json::json!({ "ok": true }),
                        Err(e) => serde_json::json!({ "ok": false, "error": e }),
                    }
                }
                Some("info") => {
                    let objects: Vec<serde_json::Value> = d
                        .scene
                        .tree_order()
                        .into_iter()
                        .filter_map(|(id, depth)| {
                            let o = d.scene.get(id)?.clone();
                            let m = d.eval.evaluate(&d.scene, id).mesh;
                            Some(serde_json::json!({"id": id.0, "name": o.name, "type": o.kind.type_name(), "depth": depth, "faces": m.face_count(), "steps": o.steps().len()}))
                        })
                        .collect();
                    serde_json::json!({ "ok": true, "result": { "objects": objects, "undo": d.undo.labels() } })
                }
                _ => match serde_json::from_value::<Command>(v) {
                    Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
                    Ok(c) => match d.execute(c) {
                        Ok(o) => serde_json::json!({ "ok": true, "result": o }),
                        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
                    },
                },
            },
        };
        writeln!(out, "{reply}").map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn pct(v: &mut [f64], q: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(((v.len() as f64 - 1.0) * q).round().max(0.0) as usize).copied().unwrap_or(0.0)
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Sculpt latency on a dense quad sphere: build times, per-dab time for each brush, and, when a
/// GPU is present, the chunked upload and a full 1920×1080 draw of the mesh.
fn bench(polys: u32, dabs: usize) -> Result<(), String> {
    let seg = ((polys as f64 / 6.0).sqrt().ceil() as u32).clamp(4, 1024);
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
                            roughness: 0.5,
                        }],
                        lines: &[],
                        overlay: &[],
                    };
                    let t = Instant::now();
                    r.render(device, queue, target, &frame);
                    let _ = device.poll(wgpu::PollType::wait_indefinitely());
                    draw_t.push(ms(t));
                }
            }
        }
        let undo = s.end_stroke().map(|(u, _)| u.indices.len()).unwrap_or(0);
        if kind == BrushKind::Mask {
            // Leave the surface free for the brushes that follow.
            s.mask_all(0.0);
            let _ = s.take_dirty_chunks();
        }
        let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
        println!(
            "{:<12} dab mean {:.3} ms p95 {:.3} ms | upload mean {:.3} ms ({} verts/dab) | draw+wait mean {:.2} ms | stroke touched {} verts",
            kind.label(),
            mean(&dab_t),
            pct(&mut dab_t, 0.95),
            mean(&up_t),
            touched / dabs.max(1),
            mean(&draw_t),
            undo
        );
    }
    let t = Instant::now();
    s.refit();
    println!("bvh refit {:.1} ms", ms(t));
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
