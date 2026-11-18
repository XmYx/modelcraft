//! Hair in the scene: a `Hair` object is a child of the mesh it grows from (the emitter). It holds
//! the guide curves, how they were grown, how render hairs are made from them, and two per-vertex
//! maps on the emitter: the density that growth follows and the mask that grooming respects.
//! Everything is editable data; render hairs are made on demand by [`HairData::strands`].

use std::sync::Arc;

use modelcraft_hair::{DynamicsSettings, GrowSpec, Guides, HairCurves, HairError, InterpSpec, Limits, Surface, SurfaceMaps, guides_from_points, interpolate};
use modelcraft_mesh::PolyMesh;
use modelcraft_sim::{FrameCache, SimTiming};
use serde::{Deserialize, Serialize};

use crate::{ObjectId, ObjectKind, Scene};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HairData {
    pub grow: GrowSpec,
    pub interp: InterpSpec,
    pub guides: Guides,
    /// Hair density on the emitter's vertices (0 to 1); growth follows it. `None` is 1 everywhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub density: Option<Vec<f32>>,
    /// Grooming mask on the emitter's vertices (0 to 1): brushes act in proportion to it, so 0
    /// protects. `None` is 1 everywhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Vec<f32>>,
    /// How the guides move when baked: gravity, wind, damping, stiffness, collision.
    #[serde(default)]
    pub dynamics: DynamicsSettings,
    /// The frames a bake covers and its frame rate and substeps.
    #[serde(default)]
    pub timing: SimTiming,
    /// A bake restarts from this seed; the same seed gives the same cache.
    #[serde(default = "one")]
    pub seed: u64,
    /// The guides at every baked frame (frame after frame, all guide points). Not saved with the
    /// project: it is derived data, and the bake command in the undo history rebuilds it when a
    /// project is opened.
    #[serde(skip)]
    pub baked: Option<Arc<FrameCache>>,
}

fn one() -> u64 {
    1
}

impl HairData {
    pub fn heap_bytes(&self) -> usize {
        self.guides.bytes()
            + 4 * (self.density.as_ref().map_or(0, Vec::len) + self.mask.as_ref().map_or(0, Vec::len))
            + 512
            + self.baked.as_ref().map_or(0, |c| c.bytes())
    }

    /// The hair as it is at scene frame `frame`: this very hair without a bake, otherwise the same
    /// hair with its guides taken from the baked cache (interpolated between frames).
    pub fn at_frame(&self, frame: f64) -> std::borrow::Cow<'_, HairData> {
        let Some(cache) = self.baked.as_ref().filter(|c| c.points() == self.guides.len() * self.guides.points_per_strand()) else {
            return std::borrow::Cow::Borrowed(self);
        };
        let mut points = Vec::new();
        if !cache.sample(frame, &mut points) {
            return std::borrow::Cow::Borrowed(self);
        }
        match guides_from_points(&self.guides, &points, &Limits::default()) {
            Ok(guides) => std::borrow::Cow::Owned(HairData { guides, ..self.clone() }),
            Err(_) => std::borrow::Cow::Borrowed(self),
        }
    }

    /// Grow fresh guides on `mesh` from `grow`.
    pub fn grow(mesh: &PolyMesh, grow: GrowSpec, interp: InterpSpec) -> Result<HairData, HairError> {
        let surface = Surface::from_mesh(mesh)?;
        let guides = modelcraft_hair::grow_guides(&surface, &grow, &SurfaceMaps::default(), &Limits::default())?;
        Ok(HairData {
            grow,
            interp,
            guides,
            density: None,
            mask: None,
            dynamics: DynamicsSettings::default(),
            timing: SimTiming::default(),
            seed: 1,
            baked: None,
        })
    }

    /// Repair data read from a file: maps of the wrong length are dropped, and guide roots that no
    /// longer fit the emitter make the whole hair unusable (`false`).
    pub fn sanitize(&mut self, mesh: &PolyMesh) -> bool {
        let n = mesh.vertex_count();
        for m in [&mut self.density, &mut self.mask] {
            if m.as_ref().is_some_and(|v| v.len() != n) {
                *m = None;
            }
        }
        match Surface::from_mesh(mesh) {
            Ok(s) => self.guides.roots().iter().all(|r| s.contains(r)),
            Err(_) => false,
        }
    }

    /// The render hairs for the current guides on `mesh`.
    pub fn strands(&self, mesh: &PolyMesh) -> Result<HairCurves, HairError> {
        self.strands_with_amount(mesh, self.interp.amount)
    }

    /// The same hairs made with another amount. Strand `i` does not depend on the amount, so the
    /// result is a prefix of the full groom: this is how the viewport draws a percentage of it.
    pub fn strands_with_amount(&self, mesh: &PolyMesh, amount: modelcraft_hair::Amount) -> Result<HairCurves, HairError> {
        let surface = Surface::from_mesh(mesh)?;
        let maps = SurfaceMaps { density: self.density.as_deref(), length: None };
        let spec = InterpSpec { amount, ..self.interp.clone() };
        interpolate(&surface, &self.guides, &spec, &maps, &Limits::default())
    }

    /// How many render hairs the settings ask for on `mesh`.
    pub fn strand_count(&self, mesh: &PolyMesh) -> usize {
        match self.interp.amount {
            modelcraft_hair::Amount::Count(n) => n,
            modelcraft_hair::Amount::PerUnitArea(d) => {
                Surface::from_mesh(mesh).map_or(0, |s| (s.area() * f64::from(d.max(0.0))).round().min(usize::MAX as f64) as usize)
            }
        }
    }
}

impl Scene {
    /// The hair objects growing from a mesh: its `Hair` children, in order.
    pub fn hair_of(&self, mesh: ObjectId) -> Vec<ObjectId> {
        self.objects().iter().filter(|o| o.parent == Some(mesh) && matches!(o.kind, ObjectKind::Hair { .. })).map(|o| o.id).collect()
    }
}

/// Drop hair objects that lost their emitter (a hair file read with its mesh missing).
pub(crate) fn repair(scene: &mut Scene) {
    let orphans: Vec<ObjectId> = scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Hair { .. }) && o.parent.is_none()).map(|o| o.id).collect();
    for id in orphans {
        scene.remove(id);
    }
}
