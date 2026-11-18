//! Hair in the scene: a `Hair` object is a child of the mesh it grows from (the emitter). It holds
//! the guide curves, how they were grown, how render hairs are made from them, and two per-vertex
//! maps on the emitter: the density that growth follows and the mask that grooming respects.
//! Everything is editable data; render hairs are made on demand by [`HairData::strands`].

use modelcraft_hair::{GrowSpec, Guides, HairCurves, HairError, InterpSpec, Limits, Surface, SurfaceMaps, interpolate};
use modelcraft_mesh::PolyMesh;
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
}

impl HairData {
    pub fn heap_bytes(&self) -> usize {
        self.guides.bytes() + 4 * (self.density.as_ref().map_or(0, Vec::len) + self.mask.as_ref().map_or(0, Vec::len)) + 512
    }

    /// Grow fresh guides on `mesh` from `grow`.
    pub fn grow(mesh: &PolyMesh, grow: GrowSpec, interp: InterpSpec) -> Result<HairData, HairError> {
        let surface = Surface::from_mesh(mesh)?;
        let guides = modelcraft_hair::grow_guides(&surface, &grow, &SurfaceMaps::default(), &Limits::default())?;
        Ok(HairData { grow, interp, guides, density: None, mask: None })
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
        let surface = Surface::from_mesh(mesh)?;
        let maps = SurfaceMaps { density: self.density.as_deref(), length: None };
        interpolate(&surface, &self.guides, &self.interp, &maps, &Limits::default())
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
