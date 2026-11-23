//! `modelcraft-cli bake-maps`: bake normal, displacement, occlusion, curvature, cavity and colour
//! maps from dense meshes onto a target's UV layout ([`modelcraft_bake`]).

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use modelcraft_bake::{MapKind, Options, Prepared, Progress};
use modelcraft_engine::Document;
use modelcraft_engine::scene::ObjectId;

pub const USAGE: &str = "  bake-maps <file> --target NAME|ID -o PREFIX [options]
                                    bake maps onto the target's UV layout; writes PREFIX_normal.png …
      --source NAME|ID[,…]          the dense meshes (default: every other visible surface)
      --maps LIST                   normal, displacement, occlusion (ao), curvature, cavity, colour
                                    (default normal)
      --size N | --width N --height N   map size in texels (default 1024)
      --cage D --distance D         rays start D above the target and reach D further down (world
                                    units; defaults 0.05 and 0.1), and occlusion rays are D long
      --rays N --seed N             occlusion rays per texel (default 64)
      --padding N                   texels spread past the layout (default 4)
      --flip-green                  the other normal-map convention
      --exr                         write 32-bit float EXR files instead of PNG";

fn value<'a>(a: &'a [String], name: &str) -> Option<&'a str> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).map(String::as_str)
}

fn number<T: std::str::FromStr>(a: &[String], name: &str) -> Result<Option<T>, String> {
    value(a, name).map(|v| v.parse::<T>().map_err(|_| format!("{name}: {v:?} is not a number"))).transpose()
}

/// An object by name or id.
fn find(d: &Document, want: &str) -> Result<ObjectId, String> {
    d.scene.objects().iter().find(|o| o.name == want || o.id.0.to_string() == want).map(|o| o.id).ok_or_else(|| format!("no object named or numbered {want:?}"))
}

pub fn parse_maps(a: &[String]) -> Result<Vec<MapKind>, String> {
    match value(a, "--maps") {
        None => Ok(vec![MapKind::Normal]),
        Some(v) => v
            .split(',')
            .map(|m| MapKind::from_id(m.trim()).ok_or_else(|| format!("--maps: no map {m:?} (normal, displacement, occlusion, curvature, cavity, colour)")))
            .collect(),
    }
}

pub fn parse_options(a: &[String]) -> Result<Options, String> {
    let d = Options::default();
    let side = number::<u32>(a, "--size")?;
    Ok(Options {
        width: number(a, "--width")?.or(side).unwrap_or(d.width),
        height: number(a, "--height")?.or(side).unwrap_or(d.height),
        maps: parse_maps(a)?,
        cage: number(a, "--cage")?.unwrap_or(d.cage),
        distance: number(a, "--distance")?.unwrap_or(d.distance),
        rays: number(a, "--rays")?.unwrap_or(d.rays),
        seed: number(a, "--seed")?.unwrap_or(d.seed),
        padding: number(a, "--padding")?.unwrap_or(d.padding),
        flip_green: a.iter().any(|x| x == "--flip-green"),
        curvature_radius: number(a, "--curvature-radius")?,
    })
}

pub fn run(a: &[String], load: impl FnOnce(&str) -> Result<Document, String>) -> Result<(), String> {
    let file = a.first().filter(|f| !f.starts_with("--")).ok_or_else(|| format!("missing file\n\n{USAGE}"))?;
    let prefix = value(a, "-o").ok_or_else(|| format!("missing -o PREFIX\n\n{USAGE}"))?;
    let want = value(a, "--target").ok_or_else(|| format!("missing --target\n\n{USAGE}"))?;
    let options = parse_options(a)?;
    let mut d = load(file)?;
    let target = find(&d, want)?;
    let sources = match value(a, "--source") {
        Some(list) => list.split(',').map(|s| find(&d, s.trim())).collect::<Result<Vec<_>, _>>()?,
        None => Vec::new(),
    };
    let prepared = Prepared::from_scene(&d.scene, &mut d.eval, target, &sources).map_err(|e| e.to_string())?;
    let t = std::time::Instant::now();
    let out = prepared.bake(&options, &AtomicBool::new(false), &Progress::default()).map_err(|e| e.to_string())?;
    let exr = a.iter().any(|x| x == "--exr");
    let mut files = Vec::new();
    for map in &out.maps {
        let path = PathBuf::from(format!("{prefix}_{}.{}", map.kind.id(), if exr { "exr" } else { "png" }));
        if exr { map.write_exr(&path) } else { map.write_png(&path) }.map_err(|e| e.to_string())?;
        files.push(path.display().to_string());
    }
    println!(
        "{}",
        serde_json::json!({
            "files": files,
            "width": options.width,
            "height": options.height,
            "sources": prepared.source_count(),
            "covered_texels": out.covered,
            "missed_texels": out.missed,
            "seconds": (t.elapsed().as_secs_f64() * 1000.0).round() / 1000.0,
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use modelcraft_engine::Command;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_maps_and_options() {
        assert_eq!(parse_maps(&args(&["--maps", "ao,normal,color"])), Ok(vec![MapKind::Occlusion, MapKind::Normal, MapKind::Colour]));
        assert!(parse_maps(&args(&["--maps", "shine"])).is_err());
        let o = parse_options(&args(&["--size", "256", "--height", "128", "--cage", "0.2", "--rays", "8", "--flip-green"])).unwrap();
        assert_eq!((o.width, o.height, o.cage, o.rays, o.flip_green), (256, 128, 0.2, 8, true));
        assert!(parse_options(&args(&["--cage", "far"])).is_err());
    }

    #[test]
    fn bakes_a_sphere_onto_a_coarser_one_and_writes_the_files() {
        let dir = std::env::temp_dir().join(format!("modelcraft-bake-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let load = |_: &str| {
            let mut d = Document::new();
            let dense =
                d.execute(Command::AddPrimitive { kind: "sphere".into(), name: Some("Dense".into()), parent: None }).map_err(|e| e.to_string())?.created[0];
            let low = d.execute(Command::AddPrimitive { kind: "sphere".into(), name: Some("Low".into()), parent: None }).map_err(|e| e.to_string())?.created[0];
            let coarse = modelcraft_engine::scene::Primitive::Sphere { radius: 1.0, segments: 8, rings: 4 };
            d.execute(Command::SetPrimitive { id: low, primitive: coarse }).map_err(|e| e.to_string())?;
            let _ = dense;
            Ok(d)
        };
        let prefix = dir.join("sphere");
        let a = args(&[
            "scene",
            "--target",
            "Low",
            "--source",
            "Dense",
            "--maps",
            "normal,occlusion",
            "--size",
            "32",
            "--cage",
            "0.2",
            "--distance",
            "0.2",
            "--rays",
            "4",
            "-o",
            prefix.to_str().unwrap(),
        ]);
        run(&a, load).unwrap();
        for name in ["sphere_normal.png", "sphere_occlusion.png"] {
            let image = craft_image_codecs::decode(&std::fs::read(dir.join(name)).unwrap()).unwrap();
            assert_eq!((image.width(), image.height()), (32, 32), "{name}");
        }
        assert!(run(&args(&["scene", "--target", "Nobody", "-o", prefix.to_str().unwrap()]), load).is_err());
        assert!(run(&args(&["scene", "-o", prefix.to_str().unwrap()]), load).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
