//! Patches made while sculpting, hiding and showing parts, and shells from a mask. Each of these
//! changes the sculpt layer in the scene (one undo snapshot); the sculpt session is then brought
//! in line without being rebuilt.

use std::sync::Arc;

use modelcraft_mesh::PolyMesh;
use modelcraft_scene::{ObjectId, ObjectKind, Source, Step, StepOp};
use modelcraft_sculpt::groups::{self, ScreenRegion};
use serde::{Deserialize, Serialize};

use crate::command::Outcome;
use crate::document::{Document, EngineError, invalid};

/// Make patches on the mesh being sculpted.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupOp {
    /// A patch per region between creases sharper than `angle` degrees.
    ByNormal { angle: f32 },
    /// A patch per UV island.
    ByUvIsland,
    /// A patch per connected part.
    ByPart,
    /// The masked faces become one new patch.
    FromMask,
}

/// Hide and show faces of the mesh being sculpted (nothing is removed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityOp {
    ShowAll,
    /// Swap what shows and what is hidden.
    Invert,
    /// Hide the patch of a triangle.
    HidePatch {
        triangle: u32,
    },
    /// Show the patch of a triangle again.
    ShowPatch {
        triangle: u32,
    },
    /// Show only the patch of a triangle.
    Isolate {
        triangle: u32,
    },
    HideMasked,
    HideUnmasked,
}

/// What to do with the faces or vertices inside a screen region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionAction {
    Mask,
    Unmask,
    Hide,
    /// Hide everything outside the region.
    Isolate,
}

fn bad(e: groups::GroupError) -> EngineError {
    EngineError::Invalid(e.to_string())
}

impl Document {
    /// The mesh the sculpt layer of `id` shows (its faces are what patches and visibility count).
    fn layer_mesh(&mut self, id: ObjectId) -> Result<Arc<PolyMesh>, EngineError> {
        if self.sculpt_layer(id).is_none() {
            return invalid("the object has no sculpt layer");
        }
        Ok(self.eval.steps_output(&self.scene, id, usize::MAX).mesh)
    }

    fn set_layer_view(&mut self, id: ObjectId, patches: Option<Vec<u32>>, hidden: Option<Vec<bool>>) -> Result<(), EngineError> {
        if let ObjectKind::Mesh { steps, .. } = &mut self.scene.edit(id)?.kind
            && let Some(Step { op: StepOp::Sculpt { layer }, .. }) = steps.last_mut()
        {
            if let Some(p) = patches {
                layer.patches = Arc::new(p);
            }
            if let Some(h) = hidden {
                layer.hidden = Arc::new(if h.iter().any(|x| *x) { h } else { Vec::new() });
            }
            return Ok(());
        }
        invalid("the object has no sculpt layer")
    }

    /// Bring the session's patches and hidden faces in line with the layer (after a change or
    /// when a session starts). A session that no longer fits is rebuilt.
    pub(crate) fn sync_session_view(&mut self, id: ObjectId) -> Result<(), EngineError> {
        let Some(layer) = self.sculpt_layer(id) else { return Ok(()) };
        let (patches, hidden) = (Arc::clone(&layer.patches), Arc::clone(&layer.hidden));
        let Some(s) = self.sculpt.as_mut().filter(|s| s.id == id) else { return Ok(()) };
        let mut fits = true;
        if !patches.is_empty() {
            fits &= s.mesh.set_face_patches(&patches);
        }
        if !hidden.is_empty() || s.mesh.any_hidden() {
            fits &= s.mesh.set_face_hidden(&hidden);
        }
        if !fits {
            self.sculpt = None;
            self.ensure_session(id)?;
        }
        Ok(())
    }

    pub(crate) fn apply_groups(&mut self, id: ObjectId, op: GroupOp) -> Result<Outcome, EngineError> {
        let mesh = self.layer_mesh(id)?;
        let mask = self.sculpt_layer(id).map(|l| Arc::clone(&l.mask)).unwrap_or_default();
        let patches = match op {
            GroupOp::ByNormal { angle } => groups::patches_by_normal(&mesh, angle).0,
            GroupOp::ByUvIsland => groups::patches_by_uv_island(&mesh).map_err(bad)?.0,
            GroupOp::ByPart => groups::patches_by_part(&mesh).0,
            GroupOp::FromMask => groups::patch_from_mask(&mesh, &mask).map_err(bad)?.0,
        };
        self.set_layer_view(id, Some(patches), None)?;
        Ok(Outcome::default())
    }

    fn hidden_now(&self, id: ObjectId, faces: usize) -> Vec<bool> {
        let h = self.sculpt_layer(id).map(|l| Arc::clone(&l.hidden)).unwrap_or_default();
        if h.len() == faces { h.to_vec() } else { vec![false; faces] }
    }

    pub(crate) fn apply_visibility(&mut self, id: ObjectId, op: VisibilityOp) -> Result<Outcome, EngineError> {
        let mesh = self.layer_mesh(id)?;
        let nf = mesh.face_count();
        let mut hidden = self.hidden_now(id, nf);
        let patch_of = |doc: &Document, triangle: u32| {
            doc.sculpt
                .as_ref()
                .filter(|s| s.id == id)
                .and_then(|s| s.mesh.patch_of_triangle(triangle))
                .ok_or_else(|| EngineError::Invalid(format!("there is no triangle {triangle}")))
        };
        let each = |hidden: &mut Vec<bool>, f: &dyn Fn(u32, bool) -> bool| {
            for (i, h) in hidden.iter_mut().enumerate() {
                *h = f(i as u32, *h);
            }
        };
        match op {
            VisibilityOp::ShowAll => hidden.clear(),
            VisibilityOp::Invert => each(&mut hidden, &|_, h| !h),
            VisibilityOp::HidePatch { triangle } => {
                let p = patch_of(self, triangle)?;
                each(&mut hidden, &|f, h| h || mesh.patch(f) == p);
            }
            VisibilityOp::ShowPatch { triangle } => {
                let p = patch_of(self, triangle)?;
                each(&mut hidden, &|f, h| h && mesh.patch(f) != p);
            }
            VisibilityOp::Isolate { triangle } => {
                let p = patch_of(self, triangle)?;
                each(&mut hidden, &|f, _| mesh.patch(f) != p);
            }
            VisibilityOp::HideMasked | VisibilityOp::HideUnmasked => {
                let mask = self.sculpt_layer(id).map(|l| Arc::clone(&l.mask)).unwrap_or_default();
                let masked = groups::faces_masked(&mesh, &mask);
                let want = op == VisibilityOp::HideMasked;
                each(&mut hidden, &|f, h| h || masked.get(f as usize).copied().unwrap_or(false) == want);
            }
        }
        if !hidden.is_empty() && hidden.iter().all(|h| *h) {
            return invalid("that would hide everything");
        }
        self.set_layer_view(id, None, Some(hidden))?;
        Ok(Outcome::default())
    }

    /// Hide the faces inside a screen region, or everything outside it.
    pub(crate) fn apply_region_view(&mut self, id: ObjectId, region: &ScreenRegion, isolate: bool) -> Result<Outcome, EngineError> {
        if !region.is_valid() {
            return invalid("the region needs 3 to 4096 corners with finite numbers");
        }
        let mesh = self.layer_mesh(id)?;
        let inside = groups::faces_in_region(&mesh, region);
        let mut hidden = self.hidden_now(id, mesh.face_count());
        for (h, i) in hidden.iter_mut().zip(&inside) {
            *h = *h || (*i != isolate);
        }
        if hidden.iter().all(|h| *h) {
            return invalid("that would hide everything");
        }
        self.set_layer_view(id, None, Some(hidden))?;
        Ok(Outcome::default())
    }

    /// A new object: the masked faces of `id` as a closed shell.
    pub(crate) fn apply_extract(&mut self, id: ObjectId, thickness: f32) -> Result<Outcome, EngineError> {
        let mesh = self.layer_mesh(id)?;
        let mask = self.sculpt_layer(id).map(|l| Arc::clone(&l.mask)).unwrap_or_default();
        let shell = groups::extract_shell(&mesh, &mask, thickness).map_err(bad)?;
        let Some(o) = self.scene.get(id).cloned() else { return invalid("the object is gone") };
        let name = self.scene.unique_name(&format!("{} shell", o.name));
        let nid = self.scene.add(name, ObjectKind::Mesh { source: Source::Mesh { mesh: Arc::new(shell) }, steps: Vec::new() }, o.parent)?;
        let n = self.scene.edit(nid)?;
        n.transform = o.transform;
        n.material = o.material.clone();
        Ok(Outcome { created: vec![nid], ..Outcome::default() })
    }
}

/// The parent face of every face of `fine`, when `fine` is `coarse` subdivided once and `coarse`
/// carried its face numbers as patches (see [`tag_faces`]).
pub(crate) fn parents(fine: &PolyMesh) -> Vec<u32> {
    (0..fine.face_count() as u32).map(|f| fine.patch(f)).collect()
}

/// Number the faces of `m` through its patches, so a subdivided copy tells each face's parent.
pub(crate) fn tag_faces(m: &mut PolyMesh) {
    let ids: Vec<u32> = (0..m.face_count() as u32).collect();
    let _ = m.set_patches(&ids);
}

/// Carry one value per face up a detail level (every child takes its parent's).
pub(crate) fn refine_faces<T: Copy + Default>(old: &[T], parent: &[u32]) -> Vec<T> {
    parent.iter().map(|p| old.get(*p as usize).copied().unwrap_or_default()).collect()
}

/// Carry one value per face down a detail level (every parent takes its first child's).
pub(crate) fn coarsen_faces<T: Copy + Default>(old: &[T], parent: &[u32], faces: usize) -> Vec<T> {
    let mut out = vec![T::default(); faces];
    for (c, p) in parent.iter().enumerate().rev() {
        if let (Some(x), Some(v)) = (out.get_mut(*p as usize), old.get(c)) {
            *x = *v;
        }
    }
    out
}
