//! Material graphs: a small graph of nodes that makes the maps of a layered surface.
//!
//! A [`Graph`] holds nodes; each node has one output (a colour, where a single number is grey)
//! and named inputs that link to other nodes' outputs. The one [`NodeKind::Surface`] node gathers
//! what the surface uses: base colour, metallic, roughness, emission and normal. [`Graph::compile`]
//! checks the graph (no loops, bounded size and depth), then evaluates every linked surface input
//! over a square of UV space into an image, which the render engine reads as that input's map on
//! the CPU and the GPU alike. Unlinked inputs keep the surface node's own number.
//!
//! The graph is plain serializable data with no dependence on the renderer or the scene; images
//! it refers to are read through a [`Resolver`] the caller supplies.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod noise;

use std::collections::{HashMap, HashSet};

use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

/// Most nodes in a graph.
pub const MAX_NODES: usize = 256;
/// Longest chain of links from the surface node.
pub const MAX_DEPTH: usize = 64;
/// Largest side of a compiled map.
pub const MAX_SIZE: u32 = 4096;
/// Most colour stops in a ramp.
pub const MAX_STOPS: usize = 32;

pub type NodeId = u32;

/// How a node mixes two colours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Blend {
    #[default]
    Mix,
    Add,
    Multiply,
    Screen,
    Difference,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MathOp {
    #[default]
    Add,
    Subtract,
    Multiply,
    Divide,
    Power,
    Minimum,
    Maximum,
    Absolute,
    /// One minus the first number.
    Invert,
}

/// What a node does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeKind {
    /// A number (grey).
    Value { value: f32 },
    /// A flat colour (linear).
    Colour { color: [f32; 3] },
    /// Fractal noise over the surface's UV: `scale` cells across the map, `detail` octaves, each
    /// octave `roughness` times as strong as the one before.
    Noise { scale: f32, detail: u32, roughness: f32, seed: u32 },
    /// Colours along 0 to 1: the input `fac` picks the place (the U coordinate without one).
    Ramp { stops: Vec<(f32, [f32; 3])> },
    /// `a` and `b` mixed by `fac`.
    Mix { blend: Blend },
    /// A calculation on the first channel of `a` and `b`.
    Math { op: MathOp },
    /// A picture, tiled across the UV square.
    Image { path: String },
    /// A normal map made from the brightness of its `colour` input (flat blue is flat), turned by
    /// `strength`.
    NormalMap { strength: f32 },
    /// The surface: the inputs below, or these numbers where nothing is linked.
    Surface { base_color: [f32; 3], metallic: f32, roughness: f32, emission: [f32; 3] },
}

impl NodeKind {
    /// The names of the inputs, in port order.
    pub fn inputs(&self) -> &'static [&'static str] {
        match self {
            NodeKind::Value { .. } | NodeKind::Colour { .. } | NodeKind::Noise { .. } | NodeKind::Image { .. } => &[],
            NodeKind::Ramp { .. } => &["fac"],
            NodeKind::Mix { .. } => &["fac", "a", "b"],
            NodeKind::Math { .. } => &["a", "b"],
            NodeKind::NormalMap { .. } => &["colour"],
            NodeKind::Surface { .. } => &["base_color", "metallic", "roughness", "emission", "normal"],
        }
    }

    /// The name in commands and files.
    pub fn id(&self) -> &'static str {
        match self {
            NodeKind::Value { .. } => "value",
            NodeKind::Colour { .. } => "colour",
            NodeKind::Noise { .. } => "noise",
            NodeKind::Ramp { .. } => "ramp",
            NodeKind::Mix { .. } => "mix",
            NodeKind::Math { .. } => "math",
            NodeKind::Image { .. } => "image",
            NodeKind::NormalMap { .. } => "normal_map",
            NodeKind::Surface { .. } => "surface",
        }
    }

    /// The English name.
    pub fn title(&self) -> &'static str {
        match self {
            NodeKind::Value { .. } => "Number",
            NodeKind::Colour { .. } => "Colour",
            NodeKind::Noise { .. } => "Noise",
            NodeKind::Ramp { .. } => "Ramp",
            NodeKind::Mix { .. } => "Mix",
            NodeKind::Math { .. } => "Math",
            NodeKind::Image { .. } => "Image",
            NodeKind::NormalMap { .. } => "Normal Map",
            NodeKind::Surface { .. } => "Surface",
        }
    }

    /// A node of the kind with its starting values (`None` for an unknown id).
    pub fn default_of(id: &str) -> Option<NodeKind> {
        Some(match id {
            "value" => NodeKind::Value { value: 0.5 },
            "colour" => NodeKind::Colour { color: [0.8, 0.8, 0.8] },
            "noise" => NodeKind::Noise { scale: 8.0, detail: 4, roughness: 0.5, seed: 0 },
            "ramp" => NodeKind::Ramp { stops: vec![(0.0, [0.0; 3]), (1.0, [1.0; 3])] },
            "mix" => NodeKind::Mix { blend: Blend::Mix },
            "math" => NodeKind::Math { op: MathOp::Multiply },
            "image" => NodeKind::Image { path: String::new() },
            "normal_map" => NodeKind::NormalMap { strength: 1.0 },
            "surface" => NodeKind::Surface { base_color: [0.8, 0.8, 0.8], metallic: 0.0, roughness: 0.5, emission: [0.0; 3] },
            _ => return None,
        })
    }

    /// The ids of every kind, in the order a picker lists them.
    pub const IDS: [&'static str; 9] = ["value", "colour", "noise", "ramp", "mix", "math", "image", "normal_map", "surface"];
}

/// A node and where its inputs come from (`inputs[k]` feeds the input named `kind.inputs()[k]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    pub kind: NodeKind,
    #[serde(default)]
    pub inputs: Vec<Option<NodeId>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    #[serde(default)]
    next_id: NodeId,
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum GraphError {
    #[error("no node {0}")]
    NoNode(NodeId),
    #[error("the node {0} has no input {1}")]
    NoInput(NodeId, usize),
    #[error("that link would make the graph loop back on itself")]
    Cycle,
    #[error("a graph holds at most {MAX_NODES} nodes")]
    TooManyNodes,
    #[error("the links are more than {MAX_DEPTH} deep")]
    TooDeep,
    #[error("the graph has no surface node")]
    NoSurface,
    #[error("a map is {0} texels on a side; the most is {MAX_SIZE}")]
    TooLarge(u32),
    #[error("{0}")]
    Invalid(String),
}

/// A picture the graph reads: linear RGB, rows from the top.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<[f32; 3]>,
}

impl Picture {
    /// The colour at `uv`, tiled and bilinearly filtered.
    pub fn sample(&self, uv: Vec2) -> Vec3 {
        let (w, h) = (self.width as i64, self.height as i64);
        if w == 0 || h == 0 || self.rgb.len() != (w * h) as usize {
            return Vec3::ZERO;
        }
        let (fx, fy) = (uv.x * w as f32 - 0.5, (1.0 - uv.y) * h as f32 - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let at = |x: i64, y: i64| Vec3::from_array(self.rgb[(y.rem_euclid(h) * w + x.rem_euclid(w)) as usize]);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let top = at(x0, y0).lerp(at(x0 + 1, y0), tx);
        let bottom = at(x0, y0 + 1).lerp(at(x0 + 1, y0 + 1), tx);
        top.lerp(bottom, ty)
    }
}

/// Finds the pictures image nodes name; `None` means it could not be read.
pub trait Resolver {
    fn picture(&self, path: &str) -> Option<Picture>;
}

/// A resolver that finds nothing (image nodes then give mid grey).
pub struct NoImages;

impl Resolver for NoImages {
    fn picture(&self, _: &str) -> Option<Picture> {
        None
    }
}

/// What a surface input makes.
#[derive(Clone, Debug, PartialEq)]
pub struct Map {
    pub width: u32,
    pub height: u32,
    /// Linear values, rows from the top.
    pub rgb: Vec<[f32; 3]>,
}

/// The maps of a compiled graph; an input that is not linked has none and uses its flat number.
#[derive(Clone, Debug, PartialEq)]
pub struct Compiled {
    pub base_color: Option<Map>,
    pub metallic: Option<Map>,
    pub roughness: Option<Map>,
    pub emission: Option<Map>,
    /// Tangent-space normal map; flat is (0.5, 0.5, 1).
    pub normal: Option<Map>,
    /// The surface node's own numbers.
    pub flat_base_color: [f32; 3],
    pub flat_metallic: f32,
    pub flat_roughness: f32,
    pub flat_emission: [f32; 3],
}

impl Graph {
    pub fn new() -> Graph {
        Graph::default()
    }

    /// A graph with just a surface node.
    pub fn with_surface() -> Graph {
        let mut g = Graph::new();
        let _ = g.add(NodeKind::Surface { base_color: [0.8; 3], metallic: 0.0, roughness: 0.5, emission: [0.0; 3] });
        g
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    fn index(&self, id: NodeId) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }

    /// The surface node, if there is one.
    pub fn surface(&self) -> Option<&Node> {
        self.nodes.iter().find(|n| matches!(n.kind, NodeKind::Surface { .. }))
    }

    /// Add a node; returns its id. A second surface node is refused.
    pub fn add(&mut self, kind: NodeKind) -> Result<NodeId, GraphError> {
        if self.nodes.len() >= MAX_NODES {
            return Err(GraphError::TooManyNodes);
        }
        if matches!(kind, NodeKind::Surface { .. }) && self.surface().is_some() {
            return Err(GraphError::Invalid("a graph has one surface node".into()));
        }
        let id = self.next_id.max(self.nodes.iter().map(|n| n.id.saturating_add(1)).max().unwrap_or(0));
        self.next_id = id.saturating_add(1);
        let mut kind = kind;
        sanitize(&mut kind);
        let inputs = vec![None; kind.inputs().len()];
        self.nodes.push(Node { id, name: kind.title().to_string(), kind, inputs });
        Ok(id)
    }

    /// Replace a node's settings (the kind must stay the same kind).
    pub fn set(&mut self, id: NodeId, kind: NodeKind) -> Result<(), GraphError> {
        let i = self.index(id).ok_or(GraphError::NoNode(id))?;
        if std::mem::discriminant(&self.nodes[i].kind) != std::mem::discriminant(&kind) {
            return Err(GraphError::Invalid("a node cannot change what it is".into()));
        }
        let mut kind = kind;
        sanitize(&mut kind);
        self.nodes[i].kind = kind;
        Ok(())
    }

    pub fn rename(&mut self, id: NodeId, name: &str) -> Result<(), GraphError> {
        let i = self.index(id).ok_or(GraphError::NoNode(id))?;
        self.nodes[i].name = name.chars().take(64).collect();
        Ok(())
    }

    /// Feed input `port` of node `to` from node `from` (`None` unlinks it). Refuses a link that
    /// would make a loop.
    pub fn link(&mut self, to: NodeId, port: usize, from: Option<NodeId>) -> Result<(), GraphError> {
        let i = self.index(to).ok_or(GraphError::NoNode(to))?;
        if port >= self.nodes[i].kind.inputs().len() {
            return Err(GraphError::NoInput(to, port));
        }
        if let Some(f) = from {
            if self.index(f).is_none() {
                return Err(GraphError::NoNode(f));
            }
            if f == to || self.reaches(f, to) {
                return Err(GraphError::Cycle);
            }
        }
        let n = self.nodes[i].kind.inputs().len();
        self.nodes[i].inputs.resize(n, None);
        self.nodes[i].inputs[port] = from;
        Ok(())
    }

    /// Whether input `port` of `to` could be fed from `from`: both exist, the port exists and the
    /// link would not make a loop.
    pub fn can_link(&self, to: NodeId, port: usize, from: NodeId) -> bool {
        self.get(to).is_some_and(|n| port < n.kind.inputs().len()) && self.get(from).is_some() && from != to && !self.reaches(from, to)
    }

    /// Whether `from`'s inputs lead back to `target`.
    fn reaches(&self, from: NodeId, target: NodeId) -> bool {
        let mut seen = HashSet::new();
        let mut stack = vec![from];
        while let Some(n) = stack.pop() {
            if n == target {
                return true;
            }
            if !seen.insert(n) {
                continue;
            }
            if let Some(node) = self.get(n) {
                stack.extend(node.inputs.iter().flatten().copied());
            }
        }
        false
    }

    /// Remove a node and the links that came from it. The surface node cannot be removed.
    pub fn remove(&mut self, id: NodeId) -> Result<(), GraphError> {
        let i = self.index(id).ok_or(GraphError::NoNode(id))?;
        if matches!(self.nodes[i].kind, NodeKind::Surface { .. }) {
            return Err(GraphError::Invalid("the surface node stays".into()));
        }
        self.nodes.remove(i);
        for n in &mut self.nodes {
            for l in &mut n.inputs {
                if *l == Some(id) {
                    *l = None;
                }
            }
        }
        Ok(())
    }

    /// Check a graph that came from a file: sizes, links to nodes that exist, no loops, depth.
    pub fn validate(&self) -> Result<(), GraphError> {
        if self.nodes.len() > MAX_NODES {
            return Err(GraphError::TooManyNodes);
        }
        let ids: HashSet<NodeId> = self.nodes.iter().map(|n| n.id).collect();
        if ids.len() != self.nodes.len() {
            return Err(GraphError::Invalid("two nodes share an id".into()));
        }
        for n in &self.nodes {
            if n.inputs.len() > n.kind.inputs().len() {
                return Err(GraphError::NoInput(n.id, n.inputs.len() - 1));
            }
            for l in n.inputs.iter().flatten() {
                if !ids.contains(l) {
                    return Err(GraphError::NoNode(*l));
                }
            }
        }
        if self.nodes.iter().filter(|n| matches!(n.kind, NodeKind::Surface { .. })).count() > 1 {
            return Err(GraphError::Invalid("a graph has one surface node".into()));
        }
        let mut depth: HashMap<NodeId, usize> = HashMap::new();
        for n in &self.nodes {
            self.depth_of(n.id, &mut depth, &mut Vec::new())?;
        }
        Ok(())
    }

    fn depth_of(&self, id: NodeId, memo: &mut HashMap<NodeId, usize>, path: &mut Vec<NodeId>) -> Result<usize, GraphError> {
        if let Some(d) = memo.get(&id) {
            return Ok(*d);
        }
        if path.contains(&id) {
            return Err(GraphError::Cycle);
        }
        if path.len() > MAX_DEPTH {
            return Err(GraphError::TooDeep);
        }
        path.push(id);
        let mut deepest = 0;
        if let Some(n) = self.get(id) {
            for l in n.inputs.iter().flatten() {
                deepest = deepest.max(self.depth_of(*l, memo, path)? + 1);
            }
        }
        path.pop();
        if deepest > MAX_DEPTH {
            return Err(GraphError::TooDeep);
        }
        memo.insert(id, deepest);
        Ok(deepest)
    }

    /// The colour node `id` makes at `uv` (`v` is 1 at the top of a picture).
    pub fn eval(&self, id: NodeId, uv: Vec2, images: &dyn Resolver) -> Vec3 {
        let mut cache = Cache::default();
        self.eval_at(id, uv, images, &mut cache, 0)
    }

    fn input(&self, node: &Node, port: usize, uv: Vec2, images: &dyn Resolver, cache: &mut Cache, depth: usize) -> Option<Vec3> {
        node.inputs.get(port).copied().flatten().map(|l| self.eval_at(l, uv, images, cache, depth + 1))
    }

    fn eval_at(&self, id: NodeId, uv: Vec2, images: &dyn Resolver, cache: &mut Cache, depth: usize) -> Vec3 {
        if depth > MAX_DEPTH {
            return Vec3::ZERO;
        }
        let Some(node) = self.get(id) else { return Vec3::ZERO };
        match &node.kind {
            NodeKind::Value { value } => Vec3::splat(*value),
            NodeKind::Colour { color } => Vec3::from_array(*color),
            NodeKind::Noise { scale, detail, roughness, seed } => Vec3::splat(noise::fractal(uv, *scale, *detail, *roughness, *seed)),
            NodeKind::Ramp { stops } => {
                let fac = self.input(node, 0, uv, images, cache, depth).map_or(uv.x, |v| v.x);
                ramp(stops, fac)
            }
            NodeKind::Mix { blend } => {
                let fac = self.input(node, 0, uv, images, cache, depth).map_or(0.5, |v| v.x).clamp(0.0, 1.0);
                let a = self.input(node, 1, uv, images, cache, depth).unwrap_or(Vec3::splat(0.5));
                let b = self.input(node, 2, uv, images, cache, depth).unwrap_or(Vec3::splat(0.5));
                let mixed = match blend {
                    Blend::Mix => b,
                    Blend::Add => a + b,
                    Blend::Multiply => a * b,
                    Blend::Screen => Vec3::ONE - (Vec3::ONE - a) * (Vec3::ONE - b),
                    Blend::Difference => (a - b).abs(),
                };
                a.lerp(mixed, fac)
            }
            NodeKind::Math { op } => {
                let a = self.input(node, 0, uv, images, cache, depth).map_or(0.0, |v| v.x);
                let b = self.input(node, 1, uv, images, cache, depth).map_or(0.5, |v| v.x);
                let r = match op {
                    MathOp::Add => a + b,
                    MathOp::Subtract => a - b,
                    MathOp::Multiply => a * b,
                    MathOp::Divide => {
                        if b.abs() > 1e-6 {
                            a / b
                        } else {
                            0.0
                        }
                    }
                    MathOp::Power => a.max(0.0).powf(b.clamp(-16.0, 16.0)),
                    MathOp::Minimum => a.min(b),
                    MathOp::Maximum => a.max(b),
                    MathOp::Absolute => a.abs(),
                    MathOp::Invert => 1.0 - a,
                };
                Vec3::splat(if r.is_finite() { r } else { 0.0 })
            }
            NodeKind::Image { path } => cache.picture(path, images).map_or(Vec3::splat(0.5), |p| p.sample(uv)),
            NodeKind::NormalMap { strength } => {
                // The slope of the input's brightness over the UV square (height change per unit
                // of u and v, from points a texel of 1/512 either side), tilting the normal away
                // from rising height; the picture's y runs against v, so the green channel follows
                // +dh/dv, as the engine reads maps.
                let e = 1.0 / 512.0;
                let mut h = |p: Vec2| self.input(node, 0, p, images, cache, depth).map_or(0.0, |v| (v.x + v.y + v.z) / 3.0);
                let dhdu = (h(uv + Vec2::X * e) - h(uv - Vec2::X * e)) / (2.0 * e);
                let dhdv = (h(uv + Vec2::Y * e) - h(uv - Vec2::Y * e)) / (2.0 * e);
                let k = strength.clamp(0.0, 64.0) * 0.05;
                let n = Vec3::new(-dhdu * k, dhdv * k, 1.0).normalize_or(Vec3::Z);
                n * 0.5 + Vec3::splat(0.5)
            }
            NodeKind::Surface { base_color, .. } => self.input(node, 0, uv, images, cache, depth).unwrap_or(Vec3::from_array(*base_color)),
        }
    }

    /// Check the graph and evaluate every linked surface input over a `size` by `size` square of
    /// UV space (the picture's top row is `v = 1`, as the renderer reads maps).
    pub fn compile(&self, size: u32, images: &dyn Resolver) -> Result<Compiled, GraphError> {
        if size > MAX_SIZE {
            return Err(GraphError::TooLarge(size));
        }
        self.validate()?;
        let surface = self.surface().ok_or(GraphError::NoSurface)?;
        let NodeKind::Surface { base_color, metallic, roughness, emission } = &surface.kind else { return Err(GraphError::NoSurface) };
        let size = size.max(1);
        let make = |port: usize| -> Option<Map> {
            let link = surface.inputs.get(port).copied().flatten()?;
            let mut cache = Cache::default();
            let mut rgb = Vec::with_capacity(size as usize * size as usize);
            for y in 0..size {
                for x in 0..size {
                    let uv = Vec2::new((x as f32 + 0.5) / size as f32, 1.0 - (y as f32 + 0.5) / size as f32);
                    rgb.push(self.eval_at(link, uv, images, &mut cache, 1).to_array());
                }
            }
            Some(Map { width: size, height: size, rgb })
        };
        Ok(Compiled {
            base_color: make(0),
            metallic: make(1),
            roughness: make(2),
            emission: make(3),
            normal: make(4),
            flat_base_color: *base_color,
            flat_metallic: *metallic,
            flat_roughness: *roughness,
            flat_emission: *emission,
        })
    }

    /// A small picture of what node `id` makes, `size` by `size` linear colours, for a card.
    pub fn thumbnail(&self, id: NodeId, size: u32, images: &dyn Resolver) -> Vec<[f32; 3]> {
        let size = size.clamp(1, 256);
        let mut cache = Cache::default();
        let mut out = Vec::with_capacity(size as usize * size as usize);
        for y in 0..size {
            for x in 0..size {
                let uv = Vec2::new((x as f32 + 0.5) / size as f32, 1.0 - (y as f32 + 0.5) / size as f32);
                out.push(self.eval_at(id, uv, images, &mut cache, 0).to_array());
            }
        }
        out
    }
}

/// Pictures already read while one evaluation runs.
#[derive(Default)]
struct Cache {
    pictures: HashMap<String, Option<Picture>>,
}

impl Cache {
    fn picture(&mut self, path: &str, images: &dyn Resolver) -> Option<&Picture> {
        self.pictures.entry(path.to_string()).or_insert_with(|| images.picture(path)).as_ref()
    }
}

fn ramp(stops: &[(f32, [f32; 3])], fac: f32) -> Vec3 {
    let mut s: Vec<(f32, Vec3)> = stops.iter().filter(|(p, _)| p.is_finite()).map(|(p, c)| (*p, Vec3::from_array(*c))).collect();
    s.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (Some(first), Some(last)) = (s.first(), s.last()) else { return Vec3::splat(fac.clamp(0.0, 1.0)) };
    if fac <= first.0 {
        return first.1;
    }
    if fac >= last.0 {
        return last.1;
    }
    for w in s.windows(2) {
        if fac >= w[0].0 && fac <= w[1].0 {
            let span = w[1].0 - w[0].0;
            return if span > 1e-9 { w[0].1.lerp(w[1].1, (fac - w[0].0) / span) } else { w[1].1 };
        }
    }
    last.1
}

fn finite(v: f32, lo: f32, hi: f32, d: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { d }
}

/// Put a node's numbers in their ranges.
fn sanitize(kind: &mut NodeKind) {
    let col = |c: &mut [f32; 3]| *c = c.map(|v| finite(v, 0.0, 1e4, 0.0));
    match kind {
        NodeKind::Value { value } => *value = finite(*value, -1e6, 1e6, 0.0),
        NodeKind::Colour { color } => col(color),
        NodeKind::Noise { scale, detail, roughness, .. } => {
            *scale = finite(*scale, 1.0, 4096.0, 8.0);
            *detail = (*detail).clamp(1, 12);
            *roughness = finite(*roughness, 0.0, 1.0, 0.5);
        }
        NodeKind::Ramp { stops } => {
            stops.truncate(MAX_STOPS);
            for (p, c) in stops.iter_mut() {
                *p = finite(*p, 0.0, 1.0, 0.0);
                col(c);
            }
            if stops.is_empty() {
                *stops = vec![(0.0, [0.0; 3]), (1.0, [1.0; 3])];
            }
        }
        NodeKind::Image { path } => path.truncate(4096),
        NodeKind::NormalMap { strength } => *strength = finite(*strength, 0.0, 64.0, 1.0),
        NodeKind::Surface { base_color, metallic, roughness, emission } => {
            col(base_color);
            col(emission);
            *metallic = finite(*metallic, 0.0, 1.0, 0.0);
            *roughness = finite(*roughness, 0.0, 1.0, 0.5);
        }
        NodeKind::Mix { .. } | NodeKind::Math { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noisy_roughness() -> Graph {
        let mut g = Graph::with_surface();
        let surface = g.surface().map_or(0, |n| n.id);
        let noise = g.add(NodeKind::Noise { scale: 6.0, detail: 3, roughness: 0.5, seed: 7 }).unwrap();
        g.link(surface, 2, Some(noise)).unwrap();
        g
    }

    #[test]
    fn a_linked_input_becomes_a_map_and_an_unlinked_one_stays_a_number() {
        let g = noisy_roughness();
        let c = g.compile(32, &NoImages).unwrap();
        let map = c.roughness.clone().expect("roughness is linked");
        assert_eq!((map.width, map.height, map.rgb.len()), (32, 32, 1024));
        assert!(c.base_color.is_none() && c.metallic.is_none() && c.normal.is_none());
        assert_eq!((c.flat_roughness, c.flat_metallic), (0.5, 0.0));
        let (lo, hi) = map.rgb.iter().fold((1.0f32, 0.0f32), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
        assert!(lo >= 0.0 && hi <= 1.0 && hi - lo > 0.2, "noise spans {lo}..{hi}");
        assert_eq!(g.compile(32, &NoImages).unwrap(), c, "the same graph gives the same maps");
        assert!(map.rgb.iter().all(|p| p[0] == p[1] && p[1] == p[2]), "noise is grey");
    }

    #[test]
    fn noise_tiles_across_the_uv_square() {
        let g = noisy_roughness();
        let n = g.nodes.iter().find(|n| matches!(n.kind, NodeKind::Noise { .. })).map(|n| n.id).unwrap();
        for t in [0.13f32, 0.5, 0.77] {
            let (a, b) = (g.eval(n, Vec2::new(0.0, t), &NoImages).x, g.eval(n, Vec2::new(1.0, t), &NoImages).x);
            assert!((a - b).abs() < 1e-4, "left and right edges meet: {a} {b}");
            let (a, b) = (g.eval(n, Vec2::new(t, 0.0), &NoImages).x, g.eval(n, Vec2::new(t, 1.0), &NoImages).x);
            assert!((a - b).abs() < 1e-4, "top and bottom edges meet: {a} {b}");
        }
    }

    #[test]
    fn nodes_make_what_they_say() {
        let mut g = Graph::with_surface();
        let a = g.add(NodeKind::Value { value: 0.25 }).unwrap();
        let b = g.add(NodeKind::Value { value: 0.5 }).unwrap();
        let m = g.add(NodeKind::Math { op: MathOp::Multiply }).unwrap();
        g.link(m, 0, Some(a)).unwrap();
        g.link(m, 1, Some(b)).unwrap();
        assert_eq!(g.eval(m, Vec2::ZERO, &NoImages), Vec3::splat(0.125));
        let ramp = g.add(NodeKind::Ramp { stops: vec![(0.0, [0.0, 0.0, 0.0]), (1.0, [1.0, 0.5, 0.0])] }).unwrap();
        g.link(ramp, 0, Some(b)).unwrap();
        assert_eq!(g.eval(ramp, Vec2::ZERO, &NoImages), Vec3::new(0.5, 0.25, 0.0));
        let mix = g.add(NodeKind::Mix { blend: Blend::Add }).unwrap();
        let (red, blue) = (g.add(NodeKind::Colour { color: [1.0, 0.0, 0.0] }).unwrap(), g.add(NodeKind::Colour { color: [0.0, 0.0, 1.0] }).unwrap());
        g.link(mix, 0, Some(b)).unwrap();
        g.link(mix, 1, Some(red)).unwrap();
        g.link(mix, 2, Some(blue)).unwrap();
        assert_eq!(g.eval(mix, Vec2::ZERO, &NoImages), Vec3::new(1.0, 0.0, 0.5), "red plus half of blue");
        let d = g.add(NodeKind::Math { op: MathOp::Divide }).unwrap();
        let zero = g.add(NodeKind::Value { value: 0.0 }).unwrap();
        g.link(d, 0, Some(a)).unwrap();
        g.link(d, 1, Some(zero)).unwrap();
        assert_eq!(g.eval(d, Vec2::ZERO, &NoImages), Vec3::ZERO, "dividing by nothing is zero, not infinity");
    }

    #[test]
    fn a_flat_normal_map_is_blue_and_a_slope_tilts_it() {
        let mut g = Graph::with_surface();
        let flat = g.add(NodeKind::Value { value: 0.3 }).unwrap();
        let nm = g.add(NodeKind::NormalMap { strength: 1.0 }).unwrap();
        g.link(nm, 0, Some(flat)).unwrap();
        let n = g.eval(nm, Vec2::new(0.4, 0.4), &NoImages);
        assert!((n - Vec3::new(0.5, 0.5, 1.0)).length() < 1e-4, "{n:?}");
        // A ramp along u is a slope up to +u: the normal leans towards -u, so red drops below half.
        let ramp = g.add(NodeKind::Ramp { stops: vec![(0.0, [0.0; 3]), (1.0, [1.0; 3])] }).unwrap();
        let nm2 = g.add(NodeKind::NormalMap { strength: 4.0 }).unwrap();
        g.link(nm2, 0, Some(ramp)).unwrap();
        let n = g.eval(nm2, Vec2::new(0.5, 0.5), &NoImages);
        assert!(n.x < 0.45 && (n.y - 0.5).abs() < 0.02 && n.z > 0.5, "{n:?}");
    }

    #[test]
    fn loops_and_oversized_graphs_are_refused_cleanly() {
        let mut g = Graph::with_surface();
        let a = g.add(NodeKind::Math { op: MathOp::Add }).unwrap();
        let b = g.add(NodeKind::Math { op: MathOp::Add }).unwrap();
        g.link(b, 0, Some(a)).unwrap();
        assert_eq!(g.link(a, 0, Some(b)), Err(GraphError::Cycle));
        assert_eq!(g.link(a, 0, Some(a)), Err(GraphError::Cycle));
        assert!(!g.can_link(a, 0, b) && !g.can_link(a, 0, a) && g.can_link(b, 1, a) && !g.can_link(b, 9, a) && !g.can_link(b, 0, 777));
        assert_eq!(g.link(a, 5, None), Err(GraphError::NoInput(a, 5)));
        assert_eq!(g.link(99, 0, None), Err(GraphError::NoNode(99)));
        assert!(g.add(NodeKind::default_of("surface").unwrap()).is_err(), "one surface node");
        let surface = g.surface().map(|n| n.id).unwrap();
        assert!(g.remove(surface).is_err());
        // A file that smuggles a loop in is caught on validation and compile.
        let mut bad = g.clone();
        if let Some(n) = bad.nodes.iter_mut().find(|n| n.id == a) {
            n.inputs = vec![Some(b), None];
        }
        assert_eq!(bad.validate(), Err(GraphError::Cycle));
        assert_eq!(bad.compile(8, &NoImages), Err(GraphError::Cycle));
        // A link to a node that is not there, too many nodes, a chain too deep and a huge map.
        let mut dangling = g.clone();
        dangling.nodes[0].inputs = vec![Some(500)];
        assert_eq!(dangling.validate(), Err(GraphError::NoNode(500)));
        let mut many = Graph::with_surface();
        for _ in 1..MAX_NODES {
            many.add(NodeKind::Value { value: 0.0 }).unwrap();
        }
        assert_eq!(many.add(NodeKind::Value { value: 0.0 }), Err(GraphError::TooManyNodes));
        let mut deep = Graph::with_surface();
        let mut prev = deep.add(NodeKind::Value { value: 0.0 }).unwrap();
        for _ in 0..MAX_DEPTH + 5 {
            let next = deep.add(NodeKind::Math { op: MathOp::Add }).unwrap();
            deep.link(next, 0, Some(prev)).unwrap();
            prev = next;
        }
        assert_eq!(deep.validate(), Err(GraphError::TooDeep));
        assert_eq!(g.compile(MAX_SIZE + 1, &NoImages), Err(GraphError::TooLarge(MAX_SIZE + 1)));
        assert_eq!(Graph::new().compile(8, &NoImages), Err(GraphError::NoSurface));
        // Removing a node unlinks what used it.
        let mut h = Graph::with_surface();
        let surface = h.surface().map(|n| n.id).unwrap();
        let v = h.add(NodeKind::Value { value: 0.5 }).unwrap();
        h.link(surface, 1, Some(v)).unwrap();
        h.remove(v).unwrap();
        assert_eq!(h.surface().unwrap().inputs[1], None);
    }

    #[test]
    fn hostile_numbers_are_held_in_range_and_images_come_from_the_resolver() {
        let mut g = Graph::with_surface();
        let n = g.add(NodeKind::Noise { scale: f32::NAN, detail: 9999, roughness: -5.0, seed: 0 }).unwrap();
        assert!(matches!(&g.get(n).unwrap().kind, NodeKind::Noise { scale, detail, roughness, .. } if *scale == 8.0 && *detail == 12 && *roughness == 0.0));
        assert!(g.set(n, NodeKind::Value { value: 1.0 }).is_err(), "a node does not change kind");
        let img = g.add(NodeKind::Image { path: "wood.png".into() }).unwrap();
        struct Two;
        impl Resolver for Two {
            fn picture(&self, path: &str) -> Option<Picture> {
                (path == "wood.png").then(|| Picture { width: 2, height: 1, rgb: vec![[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]] })
            }
        }
        assert_eq!(g.eval(img, Vec2::new(0.25, 0.5), &Two), Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(g.eval(img, Vec2::new(0.75, 0.5), &Two), Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(g.eval(img, Vec2::new(0.25, 0.5), &NoImages), Vec3::splat(0.5), "an unreadable picture is mid grey");
        assert_eq!(g.thumbnail(n, 8, &NoImages).len(), 64);
        // The graph keeps through its saved form.
        let json = serde_json::to_string(&g).unwrap();
        let back: Graph = serde_json::from_str(&json).unwrap();
        assert_eq!(back, g);
    }
}
