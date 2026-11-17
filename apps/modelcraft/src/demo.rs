//! Built-in demo scenes (written by hand for this project; no reference-app content).

use glam::{Quat, Vec3};
use modelcraft_engine::scene::{ObjectId, StepOp, Transform};
use modelcraft_engine::sculpt::{BrushKind, BrushSettings, Dab, Stroke};
use modelcraft_engine::{Command, Document};

fn add(doc: &mut Document, cmd: Command) -> Option<ObjectId> {
    doc.execute(cmd).ok().and_then(|o| o.created.first().copied())
}

fn place(doc: &mut Document, id: ObjectId, t: Vec3, r: Quat) {
    let transform = Transform { translation: t, rotation: r, scale: Vec3::ONE };
    let _ = doc.execute(Command::SetTransform { id, transform });
}

/// A small modeling scene: a box built up through history steps inside a smooth cage, a twisted
/// torus and a cylinder. Returns the object to select.
pub fn model(doc: &mut Document) -> Option<ObjectId> {
    let cube = add(doc, Command::AddPrimitive { kind: "cube".into(), name: Some("Body".into()), parent: None })?;
    // Top face (index 2) up, inset, then pushed back in; a bevel along the bottom ring.
    for op in [
        StepOp::Extrude { faces: vec![2], distance: 0.8 },
        StepOp::Inset { faces: vec![2], amount: 0.35 },
        StepOp::Extrude { faces: vec![2], distance: -0.5 },
        StepOp::LoopCut { edge: 0, cuts: 1 },
        StepOp::Bevel { edges: vec![1, 5], width: 0.12 },
    ] {
        let _ = doc.execute(Command::AddStep { id: cube, op });
    }
    let cage = add(doc, Command::AddSmoothCage { input: Some(cube), levels: 2 });
    if let Some(c) = cage {
        let _ = doc.execute(Command::Rename { id: c, name: "Smooth cage".into() });
        place(doc, c, Vec3::new(0.0, 1.0, 0.0), Quat::IDENTITY);
    }
    if let Some(t) = add(doc, Command::AddPrimitive { kind: "torus".into(), name: None, parent: None }) {
        place(doc, t, Vec3::new(3.2, 0.5, -0.5), Quat::from_rotation_x(0.5));
        let _ = doc.execute(Command::AddDeformer { kind: "twist".into(), target: t });
    }
    if let Some(c) = add(doc, Command::AddPrimitive { kind: "cylinder".into(), name: None, parent: None }) {
        place(doc, c, Vec3::new(-3.0, 1.0, 0.5), Quat::IDENTITY);
        let _ = doc.execute(Command::AddStep { id: c, op: StepOp::Inset { faces: vec![25], amount: 0.3 } });
        let _ = doc.execute(Command::AddStep { id: c, op: StepOp::Extrude { faces: vec![25], distance: 0.6 } });
    }
    Some(cube)
}

/// A dense quad sphere with a few symmetric strokes. `segments` sets the density
/// (6 × segments² quads: 410 gives about one million).
pub fn sculpt(doc: &mut Document, segments: u32) -> Option<ObjectId> {
    let id = add(doc, Command::AddPrimitive { kind: "round_cube".into(), name: Some("Head".into()), parent: None })?;
    let primitive = modelcraft_engine::ops::primitives::Primitive::RoundCube { radius: 1.2, segments: segments.clamp(4, 1024) };
    let _ = doc.execute(Command::SetPrimitive { id, primitive });
    let _ = doc.execute(Command::BeginSculpt { id });
    let stroke = |kind: BrushKind, radius: f32, strength: f32, sym: bool, pts: &[Vec3]| {
        let mut b = BrushSettings::new(kind, radius);
        b.strength = strength;
        b.symmetry.x = sym;
        let mut dabs = Vec::new();
        for w in pts.windows(2) {
            if let [a, c] = w {
                for i in 0..8 {
                    let p = a.lerp(*c, i as f32 / 8.0).normalize() * 1.2;
                    dabs.push(Dab { center: p, view_dir: -p.normalize(), pressure: 1.0, grab_delta: Vec3::ZERO });
                }
            }
        }
        Stroke { brush: b, dabs }
    };
    let v = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let strokes = [
        // Brow ridge, eye sockets, nose, cheeks, a crease for the mouth.
        stroke(BrushKind::Clay, 0.28, 0.8, true, &[v(0.05, 0.35, 1.0), v(0.35, 0.38, 0.9), v(0.6, 0.3, 0.7)]),
        stroke(BrushKind::Draw, 0.22, 0.7, true, &[v(0.35, 0.12, 1.0), v(0.36, 0.1, 1.0)]),
        stroke(BrushKind::Inflate, 0.2, 0.6, false, &[v(0.0, 0.05, 1.0), v(0.0, -0.25, 1.0)]),
        stroke(BrushKind::Clay, 0.35, 0.5, true, &[v(0.45, -0.25, 0.9), v(0.6, -0.35, 0.7)]),
        stroke(BrushKind::Crease, 0.12, 0.8, true, &[v(0.0, -0.55, 1.0), v(0.3, -0.5, 0.95)]),
        stroke(BrushKind::Pinch, 0.15, 0.5, true, &[v(0.0, -0.55, 1.0), v(0.28, -0.5, 0.95)]),
        stroke(BrushKind::Smooth, 0.3, 0.4, true, &[v(0.3, 0.0, 1.0), v(0.5, -0.2, 0.9)]),
        stroke(BrushKind::Draw, 0.5, 0.4, true, &[v(0.9, 0.2, 0.0), v(0.95, 0.0, -0.2)]),
    ];
    // Stand the head on the ground grid.
    place(doc, id, Vec3::new(0.0, 1.25, 0.0), Quat::IDENTITY);
    for mut s in strokes {
        // Invert the eye-socket pass.
        if s.brush.kind == BrushKind::Draw && s.brush.radius < 0.25 {
            s.brush.invert = true;
        }
        let _ = doc.execute(Command::SculptStroke { id, stroke: s });
    }
    Some(id)
}
