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
use std::sync::Arc;

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
#[serde(from = "StoredGraph")]
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
    #[error("the graph needs more working memory than a map may use")]
    TooMuchWork,
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
    fn picture(&self, path: &str) -> Option<Arc<Picture>>;
}

/// A resolver that finds nothing (image nodes then give mid grey).
pub struct NoImages;

impl Resolver for NoImages {
    fn picture(&self, _: &str) -> Option<Arc<Picture>> {
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

    /// Every node's numbers put in their ranges (what loading a graph does).
    pub fn sanitize(&mut self) {
        for n in &mut self.nodes {
            sanitize(&mut n.kind);
            if n.name.chars().count() > 64 {
                n.name = n.name.chars().take(64).collect();
            }
        }
    }

    /// The order nodes are worked out in to make `targets`: every node a target needs, each once,
    /// after the nodes it reads (indices into `nodes`). Only for a graph that passed [`validate`],
    /// so the walk is no deeper than [`MAX_DEPTH`].
    ///
    /// [`validate`]: Graph::validate
    fn order(&self, targets: &[NodeId]) -> Vec<usize> {
        let index: HashMap<NodeId, usize> = self.nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
        let mut done = vec![false; self.nodes.len()];
        let mut out = Vec::new();
        // An explicit stack of (node, next input to visit), so no recursion at all.
        for t in targets {
            let Some(&root) = index.get(t) else { continue };
            if done[root] {
                continue;
            }
            let mut stack = vec![(root, 0usize)];
            while let Some(top) = stack.last_mut() {
                let (i, k) = *top;
                top.1 += 1;
                match self.nodes[i].inputs.get(k).copied() {
                    Some(Some(id)) => {
                        if let Some(&j) = index.get(&id)
                            && !done[j]
                            && !stack.iter().any(|(s, _)| *s == j)
                            && stack.len() <= MAX_DEPTH + 1
                        {
                            stack.push((j, 0));
                        }
                    }
                    Some(None) => {}
                    None => {
                        stack.pop();
                        if !done[i] {
                            done[i] = true;
                            out.push(i);
                        }
                    }
                }
            }
        }
        out
    }

    /// Work out `targets` over a `size` by `size` square of UV space (the top row is `v = 1`):
    /// each node once per texel, or once in all where nothing it reads changes across the square.
    /// Buffers are freed as soon as nothing else reads them, and the texels alive at once are held
    /// under [`MAX_LIVE_TEXELS`].
    fn run(&self, targets: &[NodeId], size: u32, images: &dyn Resolver) -> Result<(Vec<Option<Value>>, Stats), GraphError> {
        if size > MAX_SIZE {
            return Err(GraphError::TooLarge(size));
        }
        self.validate()?;
        let size = size.max(1) as usize;
        let texels = size * size;
        let order = self.order(targets);
        let index: HashMap<NodeId, usize> = self.nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
        let link = |node: &Node, port: usize| node.inputs.get(port).copied().flatten().and_then(|id| index.get(&id).copied());
        // How many later nodes (and targets) still read each node's value.
        let mut readers = vec![0usize; self.nodes.len()];
        for &i in &order {
            for port in 0..self.nodes[i].inputs.len() {
                if let Some(j) = link(&self.nodes[i], port) {
                    readers[j] += 1;
                }
            }
        }
        for t in targets {
            if let Some(&i) = index.get(t) {
                readers[i] += 1;
            }
        }
        let mut values: Vec<Option<Value>> = vec![None; self.nodes.len()];
        let mut stats = Stats::default();
        let mut live = 0usize;
        let mut pictures: HashMap<&str, Option<Arc<Picture>>> = HashMap::new();
        let uv_of = |t: usize| Vec2::new(((t % size) as f32 + 0.5) / size as f32, 1.0 - ((t / size) as f32 + 0.5) / size as f32);
        for &i in &order {
            let node = &self.nodes[i];
            if let NodeKind::Image { path } = &node.kind {
                pictures.entry(path.as_str()).or_insert_with(|| images.picture(path));
            }
            let value = {
                let input = |port: usize| link(node, port).and_then(|j| values[j].as_ref());
                let varies = match &node.kind {
                    NodeKind::Noise { .. } | NodeKind::Image { .. } => true,
                    NodeKind::Ramp { .. } => input(0).is_none_or(Value::varies),
                    NodeKind::NormalMap { .. } => input(0).is_some_and(Value::varies),
                    _ => (0..node.inputs.len()).any(|p| input(p).is_some_and(Value::varies)),
                };
                if varies && live + texels > MAX_LIVE_TEXELS {
                    return Err(GraphError::TooMuchWork);
                }
                let stops = match &node.kind {
                    NodeKind::Ramp { stops } => sorted_stops(stops),
                    _ => Vec::new(),
                };
                let picture = match &node.kind {
                    NodeKind::Image { path } => pictures.get(path.as_str()).cloned().flatten(),
                    _ => None,
                };
                let at = |t: usize, uv: Vec2| -> Vec3 {
                    let get = |port: usize| input(port).map(|v| v.at(t));
                    finite3(match &node.kind {
                        NodeKind::Value { value } => Vec3::splat(*value),
                        NodeKind::Colour { color } => Vec3::from_array(*color),
                        NodeKind::Noise { scale, detail, roughness, seed } => Vec3::splat(noise::fractal(uv, *scale, *detail, *roughness, *seed)),
                        NodeKind::Ramp { .. } => ramp(&stops, get(0).map_or(uv.x, |v| v.x)),
                        NodeKind::Mix { blend } => {
                            mix(*blend, get(0).map_or(0.5, |v| v.x), get(1).unwrap_or(Vec3::splat(0.5)), get(2).unwrap_or(Vec3::splat(0.5)))
                        }
                        NodeKind::Math { op } => Vec3::splat(math(*op, get(0).map_or(0.0, |v| v.x), get(1).map_or(0.5, |v| v.x))),
                        NodeKind::Image { .. } => picture.as_ref().map_or(Vec3::splat(0.5), |p| p.sample(uv)),
                        NodeKind::NormalMap { strength } => match input(0) {
                            Some(Value::Map(h)) => normal_at(h, size, t, *strength),
                            _ => Vec3::new(0.5, 0.5, 1.0),
                        },
                        NodeKind::Surface { base_color, .. } => get(0).unwrap_or(Vec3::from_array(*base_color)),
                    })
                };
                if varies {
                    stats.evaluations += texels;
                    let mut buf = Vec::new();
                    buf.try_reserve_exact(texels).map_err(|_| GraphError::TooMuchWork)?;
                    buf.extend((0..texels).map(|t| at(t, uv_of(t))));
                    Value::Map(buf)
                } else {
                    stats.evaluations += 1;
                    Value::Flat(at(0, Vec2::splat(0.5)))
                }
            };
            if value.varies() {
                live += texels;
            }
            stats.peak_live_texels = stats.peak_live_texels.max(live);
            values[i] = Some(value);
            // Free what nothing reads any more.
            for port in 0..node.inputs.len() {
                if let Some(j) = link(node, port) {
                    readers[j] = readers[j].saturating_sub(1);
                    if readers[j] == 0 {
                        if values[j].as_ref().is_some_and(Value::varies) {
                            live -= texels;
                        }
                        values[j] = None;
                    }
                }
            }
        }
        let out = targets.iter().map(|t| index.get(t).and_then(|&i| values[i].clone())).collect();
        Ok((out, stats))
    }

    /// Check the graph and evaluate every linked surface input over a `size` by `size` square of
    /// UV space (the picture's top row is `v = 1`, as the renderer reads maps).
    pub fn compile(&self, size: u32, images: &dyn Resolver) -> Result<Compiled, GraphError> {
        self.compile_counted(size, images).map(|(c, _)| c)
    }

    /// [`compile`](Graph::compile), also saying how much work it took.
    pub fn compile_counted(&self, size: u32, images: &dyn Resolver) -> Result<(Compiled, Stats), GraphError> {
        if size > MAX_SIZE {
            return Err(GraphError::TooLarge(size));
        }
        self.validate()?;
        let surface = self.surface().ok_or(GraphError::NoSurface)?;
        let NodeKind::Surface { base_color, metallic, roughness, emission } = &surface.kind else { return Err(GraphError::NoSurface) };
        let links: Vec<Option<NodeId>> = (0..5).map(|p| surface.inputs.get(p).copied().flatten()).collect();
        let targets: Vec<NodeId> = links.iter().flatten().copied().collect();
        let (values, stats) = self.run(&targets, size, images)?;
        let side = size.max(1);
        let mut values = values.into_iter();
        let mut maps = links.iter().map(|l| l.and_then(|_| values.next().flatten().map(|v| v.into_map(side)))).collect::<Vec<_>>().into_iter();
        let mut next = || maps.next().flatten();
        Ok((
            Compiled {
                base_color: next(),
                metallic: next(),
                roughness: next(),
                emission: next(),
                normal: next(),
                flat_base_color: *base_color,
                flat_metallic: *metallic,
                flat_roughness: *roughness,
                flat_emission: *emission,
            },
            stats,
        ))
    }

    /// What node `id` makes over a `size` by `size` square of UV space.
    pub fn evaluate(&self, id: NodeId, size: u32, images: &dyn Resolver) -> Result<Map, GraphError> {
        let (values, _) = self.run(&[id], size, images)?;
        values.into_iter().next().flatten().map(|v| v.into_map(size.max(1))).ok_or(GraphError::NoNode(id))
    }

    /// Small pictures of what every node makes, `size` by `size` linear colours each, for the
    /// cards, worked out in one evaluation (each node once per texel, however many cards read it).
    /// A graph that does not validate gives flat grey pictures rather than any work.
    pub fn thumbnails(&self, size: u32, images: &dyn Resolver) -> HashMap<NodeId, Vec<[f32; 3]>> {
        self.thumbnails_counted(size, images).0
    }

    /// [`thumbnails`](Graph::thumbnails), also saying how much work it took.
    pub fn thumbnails_counted(&self, size: u32, images: &dyn Resolver) -> (HashMap<NodeId, Vec<[f32; 3]>>, Stats) {
        let size = size.clamp(1, 256);
        let ids: Vec<NodeId> = self.nodes.iter().map(|n| n.id).collect();
        match self.run(&ids, size, images) {
            Ok((values, stats)) => (ids.into_iter().zip(values).map(|(id, v)| (id, v.map_or_else(|| vec![[0.5; 3]; size as usize * size as usize], |v| v.into_map(size).rgb))).collect(), stats),
            Err(_) => (ids.into_iter().map(|id| (id, vec![[0.5; 3]; size as usize * size as usize])).collect(), Stats::default()),
        }
    }

    /// A small picture of what node `id` makes, `size` by `size` linear colours, for a card. A
    /// graph that does not validate gives a flat grey picture rather than any work.
    pub fn thumbnail(&self, id: NodeId, size: u32, images: &dyn Resolver) -> Vec<[f32; 3]> {
        let size = size.clamp(1, 256);
        match self.evaluate(id, size, images) {
            Ok(map) => map.rgb,
            Err(_) => vec![[0.5; 3]; size as usize * size as usize],
        }
    }
}

/// Most texels of working buffers alive at once while a graph is worked out (about 200 MB).
pub const MAX_LIVE_TEXELS: usize = 16 << 20;

/// How much work an evaluation took.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Node evaluations: a texel of a node that varies, or one for a node that does not.
    pub evaluations: usize,
    /// The most buffer texels alive at once.
    pub peak_live_texels: usize,
}

/// A node's value over the square: one colour everywhere, or one per texel.
#[derive(Clone, Debug)]
enum Value {
    Flat(Vec3),
    Map(Vec<Vec3>),
}

impl Value {
    fn varies(&self) -> bool {
        matches!(self, Value::Map(_))
    }

    fn at(&self, t: usize) -> Vec3 {
        match self {
            Value::Flat(v) => *v,
            Value::Map(m) => m.get(t).copied().unwrap_or(Vec3::ZERO),
        }
    }

    fn into_map(self, side: u32) -> Map {
        let n = side as usize * side as usize;
        let rgb = match self {
            Value::Flat(v) => vec![v.to_array(); n],
            Value::Map(m) => m.iter().map(Vec3::to_array).collect(),
        };
        Map { width: side, height: side, rgb }
    }
}

/// A tangent-space normal from the slope of the brightness `h` around texel `t` (the map wraps,
/// as the UV square tiles). Height rising along +u tilts the normal towards -u; the rows run
/// against v, so the green channel follows +dh/dv, as the engine reads maps.
fn normal_at(h: &[Vec3], size: usize, t: usize, strength: f32) -> Vec3 {
    let (x, y) = (t % size, t / size);
    let lum = |x: usize, y: usize| h.get(y * size + x).map_or(0.0, |v| (v.x + v.y + v.z) / 3.0);
    let (left, right) = ((x + size - 1) % size, (x + 1) % size);
    let (up, down) = ((y + size - 1) % size, (y + 1) % size);
    // Central differences in UV units: neighbours are 1/size apart.
    let half = size as f32 * 0.5;
    let dhdu = (lum(right, y) - lum(left, y)) * half;
    let dhdv = (lum(x, up) - lum(x, down)) * half;
    let k = strength.clamp(0.0, 64.0) * 0.05;
    let n = Vec3::new(-dhdu * k, dhdv * k, 1.0).normalize_or(Vec3::Z);
    n * 0.5 + Vec3::splat(0.5)
}

fn mix(blend: Blend, fac: f32, a: Vec3, b: Vec3) -> Vec3 {
    let fac = if fac.is_finite() { fac.clamp(0.0, 1.0) } else { 0.5 };
    let mixed = match blend {
        Blend::Mix => b,
        Blend::Add => a + b,
        Blend::Multiply => a * b,
        Blend::Screen => Vec3::ONE - (Vec3::ONE - a) * (Vec3::ONE - b),
        Blend::Difference => (a - b).abs(),
    };
    a.lerp(mixed, fac)
}

fn math(op: MathOp, a: f32, b: f32) -> f32 {
    match op {
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
    }
}

/// Largest magnitude a node's output keeps; beyond it values are held, and NaN becomes zero.
const MAX_MAGNITUDE: f32 = 1e6;

/// A colour every channel of which is finite and in range.
fn finite3(v: Vec3) -> Vec3 {
    Vec3::from_array(v.to_array().map(|c| if c.is_finite() { c.clamp(-MAX_MAGNITUDE, MAX_MAGNITUDE) } else { 0.0 }))
}

/// A ramp's stops with finite places, sorted, at most [`MAX_STOPS`]: done once per evaluation.
fn sorted_stops(stops: &[(f32, [f32; 3])]) -> Vec<(f32, Vec3)> {
    let mut s: Vec<(f32, Vec3)> = stops.iter().take(MAX_STOPS).filter(|(p, _)| p.is_finite()).map(|(p, c)| (*p, finite3(Vec3::from_array(*c)))).collect();
    s.sort_by(|a, b| a.0.total_cmp(&b.0));
    s
}

/// The colour of sorted stops at `fac`.
fn ramp(s: &[(f32, Vec3)], fac: f32) -> Vec3 {
    let (Some(first), Some(last)) = (s.first(), s.last()) else { return Vec3::splat(fac.clamp(0.0, 1.0)) };
    if fac.is_nan() || fac <= first.0 {
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
            stops.sort_by(|a, b| a.0.total_cmp(&b.0));
            if stops.is_empty() {
                *stops = vec![(0.0, [0.0; 3]), (1.0, [1.0; 3])];
            }
        }
        NodeKind::Image { path } => {
            if path.len() > MAX_PATH {
                *path = path.chars().take(MAX_PATH / 4).collect();
            }
        }
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

/// Longest picture path an image node keeps, in bytes.
pub const MAX_PATH: usize = 4096;

/// A graph as it is stored; loading puts every number in range ([`Graph::sanitize`]).
#[derive(Deserialize)]
struct StoredGraph {
    nodes: Vec<Node>,
    #[serde(default)]
    next_id: NodeId,
}

impl From<StoredGraph> for Graph {
    fn from(s: StoredGraph) -> Graph {
        let mut g = Graph { nodes: s.nodes, next_id: s.next_id };
        g.sanitize();
        g
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

    /// What node `id` makes at texel (x, y) of a `size` square.
    fn at(g: &Graph, id: NodeId, size: u32, x: u32, y: u32, images: &dyn Resolver) -> Vec3 {
        let map = g.evaluate(id, size, images).unwrap();
        Vec3::from_array(map.rgb[(y * size + x) as usize])
    }

    #[test]
    fn noise_tiles_across_the_uv_square() {
        for t in [0.13f32, 0.5, 0.77] {
            let f = |u: f32, v: f32| noise::fractal(Vec2::new(u, v), 6.0, 3, 0.5, 7);
            assert!((f(0.0, t) - f(1.0, t)).abs() < 1e-4, "left and right edges meet");
            assert!((f(t, 0.0) - f(t, 1.0)).abs() < 1e-4, "top and bottom edges meet");
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
        assert_eq!(at(&g, m, 1, 0, 0, &NoImages), Vec3::splat(0.125));
        let ramp = g.add(NodeKind::Ramp { stops: vec![(0.0, [0.0, 0.0, 0.0]), (1.0, [1.0, 0.5, 0.0])] }).unwrap();
        g.link(ramp, 0, Some(b)).unwrap();
        assert_eq!(at(&g, ramp, 1, 0, 0, &NoImages), Vec3::new(0.5, 0.25, 0.0));
        let mix = g.add(NodeKind::Mix { blend: Blend::Add }).unwrap();
        let (red, blue) = (g.add(NodeKind::Colour { color: [1.0, 0.0, 0.0] }).unwrap(), g.add(NodeKind::Colour { color: [0.0, 0.0, 1.0] }).unwrap());
        g.link(mix, 0, Some(b)).unwrap();
        g.link(mix, 1, Some(red)).unwrap();
        g.link(mix, 2, Some(blue)).unwrap();
        assert_eq!(at(&g, mix, 1, 0, 0, &NoImages), Vec3::new(1.0, 0.0, 0.5), "red plus half of blue");
        let d = g.add(NodeKind::Math { op: MathOp::Divide }).unwrap();
        let zero = g.add(NodeKind::Value { value: 0.0 }).unwrap();
        g.link(d, 0, Some(a)).unwrap();
        g.link(d, 1, Some(zero)).unwrap();
        assert_eq!(at(&g, d, 1, 0, 0, &NoImages), Vec3::ZERO, "dividing by nothing is zero, not infinity");
        // Every blend and operation, as the editor sets them.
        g.set(mix, NodeKind::Mix { blend: Blend::Multiply }).unwrap();
        assert_eq!(at(&g, mix, 1, 0, 0, &NoImages), Vec3::new(0.5, 0.0, 0.0), "half way to red times blue");
        g.set(m, NodeKind::Math { op: MathOp::Maximum }).unwrap();
        assert_eq!(at(&g, m, 1, 0, 0, &NoImages), Vec3::splat(0.5));
    }

    #[test]
    fn a_flat_normal_map_is_blue_and_slopes_tilt_red_and_green_the_right_way() {
        let mut g = Graph::with_surface();
        let flat = g.add(NodeKind::Value { value: 0.3 }).unwrap();
        let nm = g.add(NodeKind::NormalMap { strength: 1.0 }).unwrap();
        g.link(nm, 0, Some(flat)).unwrap();
        let n = at(&g, nm, 8, 3, 3, &NoImages);
        assert!((n - Vec3::new(0.5, 0.5, 1.0)).length() < 1e-4, "{n:?}");
        // A ramp along u is a slope up to +u: the normal leans towards -u, so red drops below half.
        let ramp = g.add(NodeKind::Ramp { stops: vec![(0.0, [0.0; 3]), (1.0, [1.0; 3])] }).unwrap();
        let nm2 = g.add(NodeKind::NormalMap { strength: 4.0 }).unwrap();
        g.link(nm2, 0, Some(ramp)).unwrap();
        let n = at(&g, nm2, 64, 32, 32, &NoImages);
        assert!(n.x < 0.45 && (n.y - 0.5).abs() < 0.02 && n.z > 0.5, "{n:?}");
        // A picture that brightens towards its top row rises along +v: green goes above half.
        struct Rising;
        impl Resolver for Rising {
            fn picture(&self, _: &str) -> Option<Arc<Picture>> {
                let h = 64;
                Some(Arc::new(Picture { width: 1, height: h, rgb: (0..h).map(|y| [1.0 - y as f32 / h as f32; 3]).collect() }))
            }
        }
        let img = g.add(NodeKind::Image { path: "rise.png".into() }).unwrap();
        let nm3 = g.add(NodeKind::NormalMap { strength: 4.0 }).unwrap();
        g.link(nm3, 0, Some(img)).unwrap();
        let n = at(&g, nm3, 64, 32, 32, &Rising);
        assert!(n.y > 0.55 && (n.x - 0.5).abs() < 0.02, "{n:?}");
    }

    #[test]
    fn shared_inputs_are_worked_out_once_per_texel() {
        // A chain as deep as allowed where both inputs of every mix read the node before it: a
        // walk per path would be 2^64 evaluations a texel; each node is evaluated once instead.
        let mut g = Graph::with_surface();
        let surface = g.surface().map(|n| n.id).unwrap();
        let mut prev = g.add(NodeKind::Noise { scale: 4.0, detail: 1, roughness: 0.5, seed: 1 }).unwrap();
        for _ in 0..MAX_DEPTH - 1 {
            let m = g.add(NodeKind::Mix { blend: Blend::Add }).unwrap();
            g.link(m, 1, Some(prev)).unwrap();
            g.link(m, 2, Some(prev)).unwrap();
            prev = m;
        }
        g.link(surface, 2, Some(prev)).unwrap();
        let (c, stats) = g.compile_counted(16, &NoImages).unwrap();
        assert!(c.roughness.is_some());
        assert_eq!(stats.evaluations, MAX_DEPTH * 256, "one evaluation per node and texel");
        assert!(stats.peak_live_texels <= 2 * 256, "buffers are freed as they are used: {}", stats.peak_live_texels);
        // Normal maps read their input's buffer; a chain of them is linear too.
        let mut h = Graph::with_surface();
        let surface = h.surface().map(|n| n.id).unwrap();
        let mut prev = h.add(NodeKind::Noise { scale: 4.0, detail: 1, roughness: 0.5, seed: 1 }).unwrap();
        for _ in 0..MAX_DEPTH - 1 {
            let n = h.add(NodeKind::NormalMap { strength: 1.0 }).unwrap();
            h.link(n, 0, Some(prev)).unwrap();
            prev = n;
        }
        h.link(surface, 4, Some(prev)).unwrap();
        let (_, stats) = h.compile_counted(16, &NoImages).unwrap();
        assert_eq!(stats.evaluations, MAX_DEPTH * 256);
        // Nodes that do not vary across the square are worked out once.
        let mut k = Graph::with_surface();
        let surface = k.surface().map(|n| n.id).unwrap();
        let v = k.add(NodeKind::Value { value: 0.2 }).unwrap();
        let m = k.add(NodeKind::Math { op: MathOp::Add }).unwrap();
        k.link(m, 0, Some(v)).unwrap();
        k.link(surface, 1, Some(m)).unwrap();
        let (c, stats) = k.compile_counted(64, &NoImages).unwrap();
        assert_eq!(stats.evaluations, 2);
        assert_eq!(c.metallic.map(|m| m.rgb[100]), Some([0.7; 3]));
    }

    #[test]
    fn every_card_thumbnail_comes_from_one_evaluation() {
        // A chain 64 deep: a thumbnail per node, each evaluated on its own, would redo the whole
        // chain above it (64 * 65 / 2 node passes); together each node is evaluated once.
        let mut g = Graph::with_surface();
        let mut prev = g.add(NodeKind::Noise { scale: 4.0, detail: 2, roughness: 0.5, seed: 3 }).unwrap();
        for _ in 0..MAX_DEPTH - 1 {
            let m = g.add(NodeKind::Math { op: MathOp::Add }).unwrap();
            g.link(m, 0, Some(prev)).unwrap();
            prev = m;
        }
        let (thumbs, stats) = g.thumbnails_counted(8, &NoImages);
        assert_eq!(thumbs.len(), g.nodes.len());
        // The 64 varying nodes once per texel, and the unlinked surface node once.
        assert_eq!(stats.evaluations, MAX_DEPTH * 64 + 1);
        assert!(stats.peak_live_texels <= g.nodes.len() * 64);
        assert_eq!(thumbs[&prev], g.thumbnail(prev, 8, &NoImages), "the same pictures as one at a time");
        let separately: usize = g.nodes.iter().map(|n| g.run(&[n.id], 8, &NoImages).map_or(0, |(_, s)| s.evaluations)).sum();
        assert!(separately > 30 * stats.evaluations, "one at a time costs {separately}");
        // A broken graph gives grey cards and no work.
        let looped: Graph = serde_json::from_str(r#"{"nodes":[{"id":0,"name":"Mix","kind":{"type":"mix","blend":"add"},"inputs":[null,0,0]}],"next_id":1}"#).unwrap();
        let (thumbs, stats) = looped.thumbnails_counted(4, &NoImages);
        assert_eq!((thumbs[&0].clone(), stats.evaluations), (vec![[0.5; 3]; 16], 0));
    }

    #[test]
    fn working_memory_is_capped_before_it_is_taken() {
        // Five noise maps alive at once at 2048 a side is 20M texels, over the 16M cap: the fifth
        // is refused before its buffer is made.
        let mut g = Graph::with_surface();
        let surface = g.surface().map(|n| n.id).unwrap();
        let noise: Vec<NodeId> = (0..5).map(|s| g.add(NodeKind::Noise { scale: 2.0, detail: 1, roughness: 0.5, seed: s }).unwrap()).collect();
        let inner = g.add(NodeKind::Mix { blend: Blend::Add }).unwrap();
        let outer = g.add(NodeKind::Mix { blend: Blend::Add }).unwrap();
        for (p, n) in noise[2..].iter().enumerate() {
            g.link(inner, p, Some(*n)).unwrap();
        }
        g.link(outer, 0, Some(noise[0])).unwrap();
        g.link(outer, 1, Some(noise[1])).unwrap();
        g.link(outer, 2, Some(inner)).unwrap();
        g.link(surface, 0, Some(outer)).unwrap();
        assert_eq!(g.compile(2048, &NoImages), Err(GraphError::TooMuchWork));
        assert!(g.compile(64, &NoImages).is_ok());
    }

    #[test]
    fn a_graph_from_a_file_is_sanitized_and_checked_before_any_work() {
        // Numbers too large for f32 load as infinity: loading puts the node's default back.
        let json = r#"{"nodes":[
            {"id":0,"name":"Surface","kind":{"type":"surface","base_color":[1e39,0.5,-1e39],"metallic":1e39,"roughness":-1e39,"emission":[0,0,0]},"inputs":[1,null,null,null,null]},
            {"id":1,"name":"Ramp","kind":{"type":"ramp","stops":[]},"inputs":[2]},
            {"id":2,"name":"Big","kind":{"type":"value","value":1e39},"inputs":[]}
        ],"next_id":3}"#;
        let mut stops = String::new();
        for i in 0..100 {
            stops.push_str(&format!("{}[{}, [0.1, 0.2, 0.3]]", if i == 0 { "" } else { "," }, 1.0 - i as f32 / 100.0));
        }
        let json = json.replace(r#""stops":[]"#, &format!(r#""stops":[{stops}]"#));
        let g: Graph = serde_json::from_str(&json).unwrap();
        let NodeKind::Surface { base_color, metallic, roughness, .. } = g.get(0).unwrap().kind else { panic!() };
        assert_eq!((base_color, metallic, roughness), ([0.0, 0.5, 0.0], 0.0, 0.5));
        assert!(matches!(&g.get(2).unwrap().kind, NodeKind::Value { value } if *value == 0.0));
        let NodeKind::Ramp { stops } = &g.get(1).unwrap().kind else { panic!() };
        assert_eq!(stops.len(), MAX_STOPS);
        assert!(stops.windows(2).all(|w| w[0].0 <= w[1].0), "stops are sorted once on load");
        let c = g.compile(4, &NoImages).unwrap();
        assert!(c.base_color.unwrap().rgb.iter().flatten().all(|v| v.is_finite()));
        // Sums that overflow stay finite.
        let mut big = Graph::with_surface();
        let v = big.add(NodeKind::Value { value: 1e6 }).unwrap();
        let mix = big.add(NodeKind::Mix { blend: Blend::Add }).unwrap();
        let fac = big.add(NodeKind::Value { value: 1.0 }).unwrap();
        big.link(mix, 0, Some(fac)).unwrap();
        big.link(mix, 1, Some(v)).unwrap();
        big.link(mix, 2, Some(v)).unwrap();
        assert!(at(&big, mix, 1, 0, 0, &NoImages).is_finite());
        // A self-linked mix from a file is refused before any work, thumbnails included.
        let looped = r#"{"nodes":[{"id":0,"name":"Mix","kind":{"type":"mix","blend":"add"},"inputs":[null,0,0]}],"next_id":1}"#;
        let g: Graph = serde_json::from_str(looped).unwrap();
        assert_eq!(g.validate(), Err(GraphError::Cycle));
        assert_eq!(g.thumbnail(0, 8, &NoImages), vec![[0.5; 3]; 64], "a broken graph gives a grey card");
        assert_eq!(g.evaluate(0, 8, &NoImages), Err(GraphError::Cycle));
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
            fn picture(&self, path: &str) -> Option<Arc<Picture>> {
                (path == "wood.png").then(|| Arc::new(Picture { width: 2, height: 1, rgb: vec![[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]] }))
            }
        }
        assert_eq!(at(&g, img, 2, 0, 0, &Two), Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(at(&g, img, 2, 1, 0, &Two), Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(at(&g, img, 2, 0, 0, &NoImages), Vec3::splat(0.5), "an unreadable picture is mid grey");
        assert_eq!(g.thumbnail(n, 8, &NoImages).len(), 64);
        // The graph keeps through its saved form.
        let json = serde_json::to_string(&g).unwrap();
        let back: Graph = serde_json::from_str(&json).unwrap();
        assert_eq!(back, g);
    }
}
