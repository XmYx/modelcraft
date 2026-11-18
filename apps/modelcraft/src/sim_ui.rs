//! Simulation in the app: the Simulation menu, the Simulation panel and the bake worker.
//!
//! The panel shows the simulation of the selected object (or the selected simulation) with its
//! timing, its kind's settings, what its cache holds, and Bake, Cancel, Clear Cache and Play Cache.
//! A bake runs on its own thread from a copy of the scene's data ([`modelcraft_engine::BakeJob`]),
//! reports its progress through atomics the panel reads every frame, can be cancelled from the
//! button, the palette or Escape, and is stored as one undo step when it finishes.
//!
//! Everything the panel does is a `sim` command, so it is undoable, journaled and scriptable;
//! every control has an accessible name and the actions are in the command palette.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;

use egui::RichText;
use modelcraft_engine::scene::{ObjectId, ObjectKind};
use modelcraft_engine::simulation::cloth::ClothSettings;
use modelcraft_engine::simulation::particles::{Direction, EmitterKind, ParticleSettings};
use modelcraft_engine::simulation::rigid::{BodyRole, RigidSettings, ShapeKind};
use modelcraft_engine::simulation::{CancelFlag, Observer, Progress, SimError, SimKind, SimObject};
use modelcraft_engine::{BakeJob, BakeResult, Command, SimCommand};

use crate::app::{ModelApp, SelectMode};
use crate::panels::{commit_edit, drag, set_name};
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimAction {
    AddRigidBody,
    AddCollider,
    AddCloth,
    AddParticles,
    Bake,
    CancelBake,
    ClearCache,
    PlayCache,
    PinVertices,
    ReleaseVertices,
    TogglePanel,
}

impl SimAction {
    pub const ALL: [SimAction; 11] = [
        SimAction::AddRigidBody,
        SimAction::AddCollider,
        SimAction::AddCloth,
        SimAction::AddParticles,
        SimAction::Bake,
        SimAction::CancelBake,
        SimAction::ClearCache,
        SimAction::PlayCache,
        SimAction::PinVertices,
        SimAction::ReleaseVertices,
        SimAction::TogglePanel,
    ];

    pub fn label(self) -> String {
        match self {
            SimAction::AddRigidBody => tl!("Add Rigid Body"),
            SimAction::AddCollider => tl!("Add Collider"),
            SimAction::AddCloth => tl!("Add Cloth"),
            SimAction::AddParticles => tl!("Add Particles"),
            SimAction::Bake => tl!("Bake Simulation"),
            SimAction::CancelBake => tl!("Cancel Simulation"),
            SimAction::ClearCache => tl!("Clear Simulation Cache"),
            SimAction::PlayCache => tl!("Play Simulation Cache"),
            SimAction::PinVertices => tl!("Pin Selected Cloth Vertices"),
            SimAction::ReleaseVertices => tl!("Release Selected Cloth Vertices"),
            SimAction::TogglePanel => tl!("Simulation Panel"),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            SimAction::AddRigidBody => "sim.add_rigid_body",
            SimAction::AddCollider => "sim.add_collider",
            SimAction::AddCloth => "sim.add_cloth",
            SimAction::AddParticles => "sim.add_particles",
            SimAction::Bake => "sim.bake",
            SimAction::CancelBake => "sim.cancel",
            SimAction::ClearCache => "sim.clear_cache",
            SimAction::PlayCache => "sim.play_cache",
            SimAction::PinVertices => "sim.pin_vertices",
            SimAction::ReleaseVertices => "sim.release_vertices",
            SimAction::TogglePanel => "sim.toggle_panel",
        }
    }

    pub fn default_binding(self) -> Option<&'static str> {
        match self {
            SimAction::Bake => Some("Cmd+Alt+B"),
            _ => None,
        }
    }
}

/// A bake in progress on its own thread.
struct Running {
    name: String,
    done: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    cancel: CancelFlag,
    handle: JoinHandle<Result<BakeResult, SimError>>,
    since: Instant,
}

/// What the bake thread tells the app.
struct Watch {
    done: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    cancel: CancelFlag,
}

impl Observer for Watch {
    fn progress(&mut self, p: Progress) {
        self.total.store(p.total, Ordering::Relaxed);
        self.done.store(p.done, Ordering::Relaxed);
    }
    fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// The app's simulation state: the bake in progress, if any.
#[derive(Default)]
pub struct SimUi {
    running: Option<Running>,
}

impl SimUi {
    /// Whether a bake is running.
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// Progress of the running bake: frames done, frames in all.
    pub fn progress(&self) -> Option<(usize, usize)> {
        self.running.as_ref().map(|r| (r.done.load(Ordering::Relaxed), r.total.load(Ordering::Relaxed)))
    }
}

/// The simulation object the panel works on: the selected one, or the one under the selected object.
pub fn current(app: &ModelApp) -> Option<ObjectId> {
    let id = app.active()?;
    if matches!(app.doc.scene.get(id).map(|o| &o.kind), Some(ObjectKind::Sim { .. })) {
        return Some(id);
    }
    app.doc.scene.sim_children(id).first().map(|(s, _)| *s)
}

fn sim_object(app: &ModelApp, id: ObjectId) -> Option<Arc<SimObject>> {
    match app.doc.scene.get(id).map(|o| &o.kind) {
        Some(ObjectKind::Sim { sim }) => Some(sim.clone()),
        _ => None,
    }
}

fn add(app: &mut ModelApp, kind: &str) {
    let Some(mesh) = app.active() else {
        app.status = tl!("Select the object that gets the simulation first");
        return;
    };
    // A simulation sits under the object it drives; selecting the simulation selects its mesh.
    let mesh = match app.doc.scene.get(mesh).map(|o| (&o.kind, o.parent)) {
        Some((ObjectKind::Sim { .. }, Some(p))) => p,
        _ => mesh,
    };
    if app.run(Command::Sim(SimCommand::Add { mesh, kind: kind.into() })) {
        app.status = tl!("Simulation added; set its frames, then bake it");
    }
}

/// Start baking the simulation `id` on a thread.
fn start_bake(app: &mut ModelApp, id: ObjectId) {
    if app.sim.running.is_some() {
        app.status = tl!("A simulation is already baking");
        return;
    }
    let job: BakeJob = match app.doc.prepare_bake(id) {
        Ok(j) => j,
        Err(e) => {
            app.status = crate::tlf!("Bake failed: {error}", error = e);
            return;
        }
    };
    let (done, total, cancel) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), CancelFlag::new());
    let mut watch = Watch { done: done.clone(), total: total.clone(), cancel: cancel.clone() };
    let spawned = std::thread::Builder::new().name("simulation".into()).spawn(move || job.run(&mut watch));
    match spawned {
        Ok(handle) => {
            let name = app.doc.scene.get(id).map(|o| o.name.clone()).unwrap_or_default();
            app.status = crate::tlf!("Baking {name}…", name = name);
            app.sim.running = Some(Running { name, done, total, cancel, handle, since: Instant::now() });
        }
        Err(e) => app.status = crate::tlf!("Bake failed: {error}", error = e),
    }
}

/// Ask the running bake to stop at the end of its frame.
pub fn cancel(app: &mut ModelApp) {
    match &app.sim.running {
        Some(r) => {
            r.cancel.cancel();
            app.status = tl!("Stopping the simulation…");
        }
        None => app.status = tl!("No simulation is baking"),
    }
}

/// Collect a finished bake and store it. Called every frame; cheap when nothing runs. Escape
/// cancels a running bake.
pub fn tick(app: &mut ModelApp, ctx: &egui::Context) {
    let Some(r) = &app.sim.running else { return };
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        r.cancel.cancel();
    }
    if !r.handle.is_finished() {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        return;
    }
    let Some(r) = app.sim.running.take() else { return };
    let seconds = r.since.elapsed().as_secs_f32();
    match r.handle.join() {
        Ok(Ok(result)) => match app.doc.store_bake(result) {
            Ok(out) => {
                app.status = crate::tlf!(
                    "Baked {name} in {seconds} s: {note}",
                    name = r.name,
                    seconds = format!("{seconds:.1}"),
                    note = out.warnings.first().cloned().unwrap_or_default()
                );
            }
            Err(e) => app.status = crate::tlf!("Bake failed: {error}", error = e),
        },
        Ok(Err(SimError::Cancelled { done })) => app.status = crate::tlf!("Simulation cancelled after {frames} frames; nothing was stored", frames = done),
        Ok(Err(e)) => app.status = crate::tlf!("Bake failed: {error}", error = e),
        Err(_) => app.status = tl!("The simulation stopped unexpectedly; nothing was stored"),
    }
    app.prune_selection();
}

pub fn act(app: &mut ModelApp, a: SimAction) {
    match a {
        SimAction::AddRigidBody => add(app, "rigid_active"),
        SimAction::AddCollider => add(app, "rigid_passive"),
        SimAction::AddCloth => add(app, "cloth"),
        SimAction::AddParticles => add(app, "particles"),
        SimAction::Bake => match current(app) {
            Some(id) => start_bake(app, id),
            None => app.status = tl!("Select an object with a simulation first"),
        },
        SimAction::CancelBake => cancel(app),
        SimAction::ClearCache => match current(app) {
            Some(id) => {
                if app.run(Command::Sim(SimCommand::ClearCache { id })) {
                    app.status = tl!("Simulation cache cleared");
                }
            }
            None => app.status = tl!("Select an object with a simulation first"),
        },
        SimAction::PlayCache => {
            let Some(sim) = current(app).and_then(|id| sim_object(app, id)) else {
                app.status = tl!("Select an object with a simulation first");
                return;
            };
            let Some(cache) = &sim.cache else {
                app.status = tl!("Bake the simulation first");
                return;
            };
            let start = cache.start;
            app.doc.set_frame(start);
            if !app.anim.playback.playing {
                crate::anim_ui::act(app, crate::anim_ui::AnimAction::PlayPause);
            }
            app.status = crate::tlf!("Playing the cache from frame {frame}", frame = start);
        }
        SimAction::PinVertices | SimAction::ReleaseVertices => {
            let Some(id) = current(app) else {
                app.status = tl!("Select a cloth first");
                return;
            };
            if app.mode != SelectMode::Vertex || app.components.is_empty() {
                app.status = tl!("Select cloth vertices in vertex mode first");
                return;
            }
            let vertices: Vec<u32> = app.components.iter().copied().collect();
            app.run(Command::Sim(SimCommand::PinVertices { id, vertices, pin: a == SimAction::PinVertices }));
        }
        SimAction::TogglePanel => {
            let shown = app.layouts.shown(app.workspace, crate::app::PanelId::Simulation);
            let m = if shown { crate::dock::Move::Hide } else { crate::dock::Move::Show };
            crate::dock::move_panel(app, crate::app::PanelId::Simulation, m);
        }
    }
}

pub fn menu(app: &mut ModelApp, ui: &mut egui::Ui) {
    ui.menu_button(tl!("Simulate"), |ui| {
        for a in SimAction::ALL {
            if matches!(a, SimAction::Bake | SimAction::PinVertices) {
                ui.separator();
            }
            let action = crate::actions::Action::Sim(a);
            let b = egui::Button::new(action.label()).shortcut_text(action.shortcut_text(app));
            if ui.add(b).clicked() {
                act(app, a);
                ui.close();
            }
        }
    });
}

// ---------------------------------------------------------------- the panel

fn kind_label(kind: &SimKind) -> String {
    match kind {
        SimKind::Cache => tl!("Cache from a file"),
        SimKind::Rigid(r) if r.role == BodyRole::Passive => tl!("Collider"),
        SimKind::Rigid(_) => tl!("Rigid body"),
        SimKind::Cloth(_) => tl!("Cloth"),
        SimKind::Particles(_) => tl!("Particles"),
    }
}

/// A combo box over `options`; returns whether the choice changed.
fn combo<T: Copy + PartialEq>(ui: &mut egui::Ui, id: (&'static str, u64), name: String, value: &mut T, options: &[(T, String)]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(name.clone());
        let shown = options.iter().find(|(v, _)| v == value).map(|(_, l)| l.clone()).unwrap_or_default();
        let r = egui::ComboBox::from_id_salt(id).selected_text(shown).show_ui(ui, |ui| {
            for (v, l) in options {
                if ui.selectable_label(value == v, l).clicked() && value != v {
                    *value = *v;
                    changed = true;
                }
            }
        });
        set_name(ui, &r.response, name);
        if changed {
            r.response.request_focus();
        }
    });
    changed
}

fn check(ui: &mut egui::Ui, value: &mut bool, name: String) -> bool {
    let r = ui.checkbox(value, name.clone());
    set_name(ui, &r, name);
    r.changed()
}

/// An integer field with an accessible name.
fn count(ui: &mut egui::Ui, v: &mut u32, range: std::ops::RangeInclusive<u32>, name: String) -> egui::Response {
    ui.label(name.clone());
    let r = ui.add(egui::DragValue::new(v).speed(0.2).range(range));
    set_name(ui, &r, name);
    r
}

fn rigid_fields(ui: &mut egui::Ui, r: &mut RigidSettings, id: ObjectId, drags: &mut Vec<egui::Response>) -> bool {
    let mut discrete = false;
    discrete |= combo(ui, ("sim-role", id.0), tl!("Role"), &mut r.role, &[(BodyRole::Active, tl!("Active")), (BodyRole::Passive, tl!("Passive (collider)"))]);
    discrete |= combo(
        ui,
        ("sim-shape", id.0),
        tl!("Collision shape"),
        &mut r.shape,
        &[
            (ShapeKind::Auto, tl!("Automatic")),
            (ShapeKind::Box, tl!("Box")),
            (ShapeKind::Sphere, tl!("Sphere")),
            (ShapeKind::Hull, tl!("Convex hull")),
            (ShapeKind::Mesh, tl!("Mesh (colliders only)")),
        ],
    );
    if r.role == BodyRole::Active {
        ui.horizontal(|ui| drags.push(drag(ui, &mut r.mass, 0.05, 0.001..=1_000_000.0, tl!("Mass (kg)"))));
        ui.horizontal(|ui| {
            drags.push(drag(ui, &mut r.linear_damping, 0.01, 0.0..=100.0, tl!("Linear damping")));
            drags.push(drag(ui, &mut r.angular_damping, 0.01, 0.0..=100.0, tl!("Angular damping")));
        });
        discrete |= check(ui, &mut r.can_sleep, tl!("Sleep when settled"));
    }
    ui.horizontal(|ui| {
        drags.push(drag(ui, &mut r.friction, 0.01, 0.0..=10.0, tl!("Friction")));
        drags.push(drag(ui, &mut r.bounciness, 0.01, 0.0..=1.0, tl!("Bounciness")));
    });
    ui.label(RichText::new(tl!("World: set by the body you bake")).color(theme::TEXT_DIM).small());
    ui.horizontal(|ui| drags.push(drag(ui, &mut r.gravity[1], 0.05, -1000.0..=1000.0, tl!("Gravity Y"))));
    let mut ground = r.ground.is_some();
    if check(ui, &mut ground, tl!("Ground plane")) {
        r.ground = ground.then_some(0.0);
        discrete = true;
    }
    if let Some(g) = &mut r.ground {
        ui.horizontal(|ui| drags.push(drag(ui, g, 0.05, -100_000.0..=100_000.0, tl!("Ground height"))));
    }
    ui.label(RichText::new(crate::tlf!("{count} joints; add them with the sim command", count = r.joints.len())).color(theme::TEXT_DIM).small());
    discrete
}

fn cloth_fields(ui: &mut egui::Ui, c: &mut ClothSettings, id: ObjectId, drags: &mut Vec<egui::Response>) -> bool {
    let mut discrete = false;
    ui.horizontal(|ui| drags.push(drag(ui, &mut c.mass, 0.01, 0.001..=100_000.0, tl!("Mass (kg)"))));
    ui.horizontal(|ui| {
        drags.push(drag(ui, &mut c.stretch, 0.005, 0.0..=1.0, tl!("Stretch stiffness")));
        drags.push(drag(ui, &mut c.shear, 0.005, 0.0..=1.0, tl!("Shear stiffness")));
        drags.push(drag(ui, &mut c.bend, 0.005, 0.0..=1.0, tl!("Bend stiffness")));
    });
    ui.horizontal(|ui| {
        drags.push(drag(ui, &mut c.damping, 0.01, 0.0..=100.0, tl!("Damping")));
        drags.push(drag(ui, &mut c.thickness, 0.001, 0.0001..=10.0, tl!("Thickness")));
        drags.push(drag(ui, &mut c.friction, 0.01, 0.0..=2.0, tl!("Friction")));
    });
    ui.horizontal(|ui| drags.push(count(ui, &mut c.iterations, 1..=16, tl!("Iterations per substep"))));
    discrete |= check(ui, &mut c.collide, tl!("Collide with other objects"));
    discrete |= check(ui, &mut c.self_collision, tl!("Self collision"));
    discrete |= check(ui, &mut c.pin_mask, tl!("Pin vertices with a sculpt mask"));
    ui.horizontal(|ui| {
        drags.push(drag(ui, &mut c.wind.strength, 0.05, 0.0..=200.0, tl!("Wind speed")));
        drags.push(drag(ui, &mut c.wind.turbulence, 0.01, 0.0..=1.0, tl!("Turbulence")));
    });
    ui.horizontal(|ui| drags.push(drag(ui, &mut c.gravity[1], 0.05, -1000.0..=1000.0, tl!("Gravity Y"))));
    ui.label(RichText::new(crate::tlf!("{count} pinned vertices", count = c.pinned.len())).color(theme::TEXT_DIM).small());
    let _ = id;
    discrete
}

fn particle_fields(ui: &mut egui::Ui, p: &mut ParticleSettings, id: ObjectId, drags: &mut Vec<egui::Response>) -> bool {
    let mut discrete = false;
    discrete |= combo(
        ui,
        ("sim-emitter", id.0),
        tl!("Emitter"),
        &mut p.emitter,
        &[(EmitterKind::Point, tl!("Point")), (EmitterKind::Surface, tl!("Surface")), (EmitterKind::Volume, tl!("Volume"))],
    );
    discrete |= combo(
        ui,
        ("sim-direction", id.0),
        tl!("Direction"),
        &mut p.direction,
        &[(Direction::Normal, tl!("Along the surface normal")), (Direction::Fixed, tl!("Fixed direction")), (Direction::Random, tl!("Any way"))],
    );
    ui.horizontal(|ui| {
        drags.push(drag(ui, &mut p.rate, 1.0, 0.0..=1_000_000.0, tl!("Rate per second")));
        drags.push(count(ui, &mut p.burst, 0..=1_000_000, tl!("Burst at start")));
    });
    ui.horizontal(|ui| {
        drags.push(drag(ui, &mut p.lifetime, 0.05, 0.01..=100_000.0, tl!("Lifetime (s)")));
        drags.push(drag(ui, &mut p.lifetime_variation, 0.01, 0.0..=1.0, tl!("Lifetime variation")));
    });
    ui.horizontal(|ui| {
        drags.push(drag(ui, &mut p.speed, 0.05, 0.0..=1000.0, tl!("Speed")));
        drags.push(drag(ui, &mut p.speed_variation, 0.01, 0.0..=1.0, tl!("Speed variation")));
        drags.push(drag(ui, &mut p.spread, 0.5, 0.0..=180.0, tl!("Spread (degrees)")));
    });
    ui.horizontal(|ui| {
        drags.push(drag(ui, &mut p.gravity[1], 0.05, -1000.0..=1000.0, tl!("Gravity Y")));
        drags.push(drag(ui, &mut p.drag, 0.01, 0.0..=1000.0, tl!("Drag")));
    });
    ui.horizontal(|ui| {
        drags.push(drag(ui, &mut p.wind.strength, 0.05, 0.0..=200.0, tl!("Wind speed")));
        drags.push(drag(ui, &mut p.wind.turbulence, 0.01, 0.0..=1.0, tl!("Turbulence")));
        drags.push(drag(ui, &mut p.noise_strength, 0.05, 0.0..=1000.0, tl!("Noise")));
    });
    discrete |= check(ui, &mut p.collide, tl!("Collide with other objects"));
    ui.horizontal(|ui| {
        drags.push(drag(ui, &mut p.bounciness, 0.01, 0.0..=1.0, tl!("Bounciness")));
        drags.push(drag(ui, &mut p.friction, 0.01, 0.0..=1.0, tl!("Friction")));
    });
    ui.horizontal(|ui| {
        drags.push(count(ui, &mut p.capacity, 1..=1_000_000, tl!("Pool size")));
        drags.push(drag(ui, &mut p.size, 0.001, 0.0001..=1000.0, tl!("Particle size")));
    });
    ui.label(RichText::new(crate::tlf!("{count} attractors", count = p.attractors.len())).color(theme::TEXT_DIM).small());
    discrete
}

/// The settings of the simulation's kind; commits like the other property sections.
fn kind_fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, sim: &SimObject) {
    let mut kind = sim.kind.clone();
    let mut drags: Vec<egui::Response> = Vec::new();
    let discrete = match &mut kind {
        SimKind::Cache => {
            ui.label(RichText::new(tl!("This cache came from a file; delete the simulation to remove it.")).color(theme::TEXT_DIM).small());
            false
        }
        SimKind::Rigid(r) => rigid_fields(ui, r, id, &mut drags),
        SimKind::Cloth(c) => cloth_fields(ui, c, id, &mut drags),
        SimKind::Particles(p) => particle_fields(ui, p, id, &mut drags),
    };
    if kind == sim.kind {
        return;
    }
    if discrete {
        app.run(Command::Sim(SimCommand::SetKind { id, kind }));
    } else if let Some(r) = drags.iter().find(|r| r.changed() || r.drag_stopped()) {
        let preview = kind.clone();
        commit_edit(
            app,
            r,
            move |o| {
                if let ObjectKind::Sim { sim } = &mut o.kind {
                    Arc::make_mut(sim).kind = preview;
                }
            },
            || Command::Sim(SimCommand::SetKind { id, kind }),
            id,
        );
    }
}

/// Start, end, frame rate, substeps, seed and cache budget.
fn timing_fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, sim: &SimObject) {
    let mut t = sim.timing;
    let (mut seed, mut mib) = (sim.seed, sim.max_cache_mib);
    let mut drags: Vec<egui::Response> = Vec::new();
    ui.horizontal(|ui| {
        let (mut start, mut end) = (t.start_frame as f32, t.end_frame as f32);
        drags.push(drag(ui, &mut start, 0.5, -1_000_000.0..=1_000_000.0, tl!("Start frame")));
        drags.push(drag(ui, &mut end, 0.5, -1_000_000.0..=1_000_000.0, tl!("End frame")));
        t.start_frame = f64::from(start.round());
        t.end_frame = f64::from(end.round());
    });
    ui.horizontal(|ui| {
        let mut fps = t.fps as f32;
        drags.push(drag(ui, &mut fps, 0.1, 1.0..=1000.0, tl!("Frames per second")));
        t.fps = f64::from(fps);
        drags.push(count(ui, &mut t.substeps, 1..=64, tl!("Substeps per frame")));
    });
    ui.horizontal(|ui| {
        ui.label(tl!("Seed"));
        let r = ui.add(egui::DragValue::new(&mut seed).speed(0.2));
        set_name(ui, &r, tl!("Seed"));
        drags.push(r);
        drags.push(count(ui, &mut mib, 1..=2048, tl!("Cache limit (MiB)")));
    });
    if t == sim.timing && seed == sim.seed && mib == sim.max_cache_mib {
        return;
    }
    let cmd = || {
        Command::Sim(SimCommand::Set {
            id,
            start_frame: Some(t.start_frame),
            end_frame: Some(t.end_frame),
            fps: Some(t.fps),
            substeps: Some(t.substeps),
            seed: Some(seed),
            max_cache_mib: Some(mib),
        })
    };
    if let Some(r) = drags.iter().find(|r| r.changed() || r.drag_stopped()) {
        commit_edit(
            app,
            r,
            |o| {
                if let ObjectKind::Sim { sim } = &mut o.kind {
                    let s = Arc::make_mut(sim);
                    s.timing = t.sanitized();
                    s.seed = seed;
                    s.max_cache_mib = mib;
                }
            },
            cmd,
            id,
        );
    }
}

fn cache_line(app: &ModelApp, id: ObjectId, sim: &SimObject) -> String {
    match &sim.cache {
        None => tl!("No cache: bake the simulation to see it"),
        Some(c) => {
            let note = if app.doc.sim_stale(id) { tl!(" (out of date: bake again)") } else { String::new() };
            crate::tlf!(
                "Cache: {frames} frames of {points} points, {mib} MiB, from frame {start}{note}",
                frames = c.frames(),
                points = c.points(),
                mib = format!("{:.1}", c.bytes() as f64 / (1 << 20) as f64),
                start = c.start,
                note = note
            )
        }
    }
}

pub fn panel(app: &mut ModelApp, ui: &mut egui::Ui) {
    // What can be added: the selected object gets a simulation.
    let selected = app.active();
    let owner = selected.and_then(|id| match app.doc.scene.get(id).map(|o| (&o.kind, o.parent)) {
        Some((ObjectKind::Sim { .. }, p)) => p,
        Some(_) => Some(id),
        None => None,
    });
    let Some(sim_id) = current(app) else {
        ui.label(RichText::new(tl!("No simulation on the selected object.")).color(theme::TEXT_DIM));
        ui.add_enabled_ui(owner.is_some(), |ui| {
            ui.horizontal_wrapped(|ui| {
                for a in [SimAction::AddRigidBody, SimAction::AddCollider, SimAction::AddCloth, SimAction::AddParticles] {
                    let r = ui.button(a.label());
                    if r.clicked() {
                        act(app, a);
                    }
                }
            });
        });
        if owner.is_none() {
            ui.label(
                RichText::new(tl!("Select a mesh first: rigid bodies and cloth drive a mesh; particles can come from any object."))
                    .color(theme::TEXT_DIM)
                    .small(),
            );
        }
        return;
    };
    let Some(sim) = sim_object(app, sim_id) else { return };
    let name = app.doc.scene.get(sim_id).map(|o| o.name.clone()).unwrap_or_default();
    ui.label(RichText::new(crate::tlf!("{name}: {kind}", name = name, kind = kind_label(&sim.kind))).strong());
    let busy = app.sim.busy();
    // Bake, cancel and the cache.
    ui.horizontal_wrapped(|ui| {
        if sim.kind.is_solver() {
            let r = ui.add_enabled(!busy, egui::Button::new(SimAction::Bake.label()));
            if r.clicked() {
                act(app, SimAction::Bake);
            }
        }
        let r = ui.add_enabled(busy, egui::Button::new(tl!("Cancel")));
        if r.clicked() {
            act(app, SimAction::CancelBake);
        }
        let has_cache = sim.cache.is_some();
        for a in [SimAction::PlayCache, SimAction::ClearCache] {
            let enabled = has_cache && !busy && (a == SimAction::PlayCache || sim.kind.is_solver());
            if ui.add_enabled(enabled, egui::Button::new(a.label())).clicked() {
                act(app, a);
            }
        }
    });
    if let Some((done, total)) = app.sim.progress() {
        let fraction = if total == 0 { 0.0 } else { done as f32 / total as f32 };
        let bar = ui.add(egui::ProgressBar::new(fraction).show_percentage());
        let text = crate::tlf!("Baking: frame {done} of {total}", done = done, total = total);
        bar.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ProgressIndicator, true, text.clone()));
        ui.ctx().accesskit_node_builder(bar.id, |n| n.set_numeric_value(f64::from(fraction * 100.0)));
        ui.label(RichText::new(tl!("Escape or Cancel stops it after the current frame")).color(theme::TEXT_DIM).small());
    } else {
        ui.label(RichText::new(cache_line(app, sim_id, &sim)).color(theme::TEXT_DIM).small());
    }
    ui.add_space(4.0);
    egui::CollapsingHeader::new(tl!("Frames and seed")).default_open(true).show(ui, |ui| timing_fields(app, ui, sim_id, &sim));
    egui::CollapsingHeader::new(tl!("Settings")).default_open(true).show(ui, |ui| kind_fields(app, ui, sim_id, &sim));
    if matches!(sim.kind, SimKind::Cloth(_)) {
        ui.horizontal_wrapped(|ui| {
            for a in [SimAction::PinVertices, SimAction::ReleaseVertices] {
                if ui.button(a.label()).clicked() {
                    act(app, a);
                }
            }
        });
    }
    let e = app.doc.eval.evaluate(&app.doc.scene, sim_id);
    for err in &e.errors {
        ui.label(RichText::new(&err.message).color(theme::TEXT_DIM).small());
    }
}
