//! Brush presets: named brush settings. The built-in ones below are the starting palette; an
//! app adds the user's own next to them (a preset is plain data and serialises as JSON).

use serde::{Deserialize, Serialize};

use crate::alpha::Alpha;
use crate::brush::{BrushKind, BrushSettings, Falloff, Stamp};
use crate::stroke::StrokeMode;

/// Longest preset name, in characters.
pub const MAX_PRESET_NAME: usize = 48;
/// Most presets a library keeps.
pub const MAX_PRESETS: usize = 256;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushPreset {
    pub name: String,
    pub brush: BrushSettings,
}

impl BrushPreset {
    /// A preset from a brush; the radius is kept only as a proportion (apps size brushes on
    /// screen), symmetry and live detail are left to the session.
    pub fn new(name: &str, brush: &BrushSettings) -> BrushPreset {
        let mut name: String = name.trim().chars().filter(|c| !c.is_control()).take(MAX_PRESET_NAME).collect();
        if name.is_empty() {
            name = brush.kind.label().to_string();
        }
        let mut brush = brush.clone().sanitized();
        brush.symmetry = Default::default();
        brush.live_detail = None;
        BrushPreset { name, brush }
    }

    /// The preset's brush with the session's radius, symmetry and live detail.
    pub fn apply(&self, current: &BrushSettings) -> BrushSettings {
        let mut b = self.brush.clone().sanitized();
        b.radius = current.radius;
        b.symmetry = current.symmetry;
        b.live_detail = current.live_detail;
        b
    }
}

impl BrushSettings {
    /// The same brush with every number made finite and in range (presets come from files).
    pub fn sanitized(mut self) -> BrushSettings {
        let fix = |v: f32, lo: f32, hi: f32, or: f32| if v.is_finite() { v.clamp(lo, hi) } else { or };
        self.radius = fix(self.radius, 1e-6, 1e6, 0.2);
        self.strength = fix(self.strength, 0.0, 1.0, 0.4);
        self.hardness = fix(self.hardness, 0.0, 1.0, 0.0);
        self.spacing = fix(self.spacing, 0.02, 4.0, 0.2);
        self.live_detail = self.live_detail.filter(|e| e.is_finite() && *e > 0.0);
        self.stamp = match self.stamp {
            Stamp::Noise { scale } => Stamp::Noise { scale: fix(scale, 0.1, 64.0, 6.0) },
            Stamp::Rings { count } => Stamp::Rings { count: fix(count, 1.0, 32.0, 4.0) },
            Stamp::Image { alpha, turn } => Stamp::Image { alpha, turn: fix(turn, -100.0, 100.0, 0.0) },
            s => s,
        };
        self.stroke = match self.stroke {
            StrokeMode::Spray { scatter } => StrokeMode::Spray { scatter: fix(scatter, 0.0, 5.0, 1.0) },
            StrokeMode::Trail { leash } => StrokeMode::Trail { leash: fix(leash, 0.0, 10.0, 1.0) },
            m => m,
        };
        self
    }
}

/// The built-in palette.
pub fn builtin() -> Vec<BrushPreset> {
    let make = |name: &str, kind: BrushKind, f: &dyn Fn(&mut BrushSettings)| {
        let mut b = BrushSettings::new(kind, 0.2);
        f(&mut b);
        BrushPreset { name: name.to_string(), brush: b }
    };
    vec![
        make("Soft clay", BrushKind::Clay, &|b| b.strength = 0.35),
        make("Clay strips", BrushKind::Buildup, &|b| {
            b.stamp = Stamp::Square;
            b.falloff = Falloff::Constant;
            b.strength = 0.5;
        }),
        make("Hard edge", BrushKind::Gouge, &|b| {
            b.falloff = Falloff::Needle;
            b.stroke = StrokeMode::Trail { leash: 1.5 };
            b.strength = 0.6;
        }),
        make("Planar cut", BrushKind::Trim, &|b| {
            b.falloff = Falloff::Root;
            b.hardness = 0.5;
        }),
        make("Burnish", BrushKind::Polish, &|b| b.hardness = 0.3),
        make("Skin pores", BrushKind::Draw, &|b| {
            b.stamp = Stamp::Image { alpha: Alpha::Pores, turn: 0.0 };
            b.stroke = StrokeMode::Spray { scatter: 0.6 };
            b.invert = true;
            b.strength = 0.25;
        }),
        make("Scales", BrushKind::Draw, &|b| {
            b.stamp = Stamp::Image { alpha: Alpha::Scales, turn: 0.0 };
            b.stroke = StrokeMode::Dots;
            b.falloff = Falloff::Constant;
        }),
        make("Cracked earth", BrushKind::Draw, &|b| {
            b.stamp = Stamp::Image { alpha: Alpha::Cracks, turn: 0.0 };
            b.stroke = StrokeMode::Drag;
            b.falloff = Falloff::Root;
            b.invert = true;
        }),
        make("Tendril", BrushKind::Pull, &|b| b.falloff = Falloff::Sharp),
        make("Stamp star", BrushKind::Plateau, &|b| {
            b.stamp = Stamp::Image { alpha: Alpha::Star, turn: 0.0 };
            b.stroke = StrokeMode::Drag;
            b.falloff = Falloff::Constant;
        }),
    ]
}

/// Add or replace (by name) a preset in a library, keeping it within [`MAX_PRESETS`].
/// Returns false when the library is full.
pub fn upsert(library: &mut Vec<BrushPreset>, preset: BrushPreset) -> bool {
    if let Some(slot) = library.iter_mut().find(|p| p.name == preset.name) {
        *slot = preset;
        return true;
    }
    if library.len() >= MAX_PRESETS {
        return false;
    }
    library.push(preset);
    true
}
