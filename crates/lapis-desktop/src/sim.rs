//! 2D force-directed layout for the whole-vault graph.
//!
//! Ported from the forces two open graph views agree on — Quartz's d3 stack
//! (`charge`, `center`, `link`, link distance) and the Barnes-Hut Rust sim in
//! `obsidian-3d-graph`, flattened to two dimensions. Neither library is a
//! dependency: there is no Pixi, no d3, no Cytoscape here, only the arithmetic.
//!
//! The sim is deliberately headless. It owns positions and velocities, ticks
//! them, and reports kinetic energy; drawing belongs to the window and storage
//! belongs to the engine. That is what makes it testable without a GPU.
//!
//! Repulsion is all-pairs below [`BARNES_HUT_THRESHOLD`] nodes and a Barnes-Hut
//! quadtree above it. The exact sum is cheaper than a tree on a small vault; on
//! a big one the tree is the difference between 2 000 nodes at 60 fps and 2 000
//! nodes at 30.

use lapis_lattice::GraphSnapshot;
use rayon::prelude::*;

use crate::quadtree::{Charge, QuadTree};

/// Above this many nodes, repulsion goes through the quadtree.
pub const BARNES_HUT_THRESHOLD: usize = 64;
/// Default [`ForceParams::start_radius`]. On a 13,000-note vault a start this
/// compact put a hub's own notes at a sixth of the graph's radius from it,
/// against more than half from a start as wide as the graph.
pub const START_RADIUS: f32 = 6.0;
/// Share of a link length under which two nodes count as that far apart.
pub const NEAR_SHARE: f32 = 0.05;
/// Share of [`ForceParams::max_velocity`] a cool layout (a drag, a warm start)
/// may move at.
pub const WARM_SPEED_SHARE: f32 = 0.2;
/// How many links out from a dragged or released node may move with it.
pub const MOBILE_HOPS: usize = 2;
/// Most nodes a drag frees, so a drag on a huge hub stays a local edit.
pub const MOBILE_CAP: usize = 1_500;
/// Most threads the layout uses. rayon's global pool takes one per logical core;
/// the settle leaves one free for the window and the rest of the machine.
const LAYOUT_THREADS: usize = 8;

/// The layout's own thread pool, built once; `None` if the system refuses one,
/// and the settle then runs on rayon's global pool.
fn layout_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        rayon::ThreadPoolBuilder::new()
            .num_threads(cores.saturating_sub(1).clamp(1, LAYOUT_THREADS))
            .thread_name(|i| format!("lapis-layout-{i}"))
            .build()
            .ok()
    })
    .as_ref()
}

/// Nodes per parallel repulsion task: enough work to outweigh the hand-off.
const REPULSION_CHUNK: usize = 256;
/// Starting places: the nodes in link order, laid along a spiral.
///
/// The order is breadth-first from the busiest node of each component, biggest
/// component first, so a hub's notes follow it and a chain stays in sequence.
/// The spiral is Archimedean with even spacing, so consecutive slots touch:
/// a hub starts beside its notes and a chain starts unfolded. A golden-angle
/// spiral spreads consecutive slots round the centre, which started every
/// hub's notes on all sides of the graph and folded chains into zig-zags.
fn start_positions(
    n: usize,
    edges: &[(usize, usize)],
    degree: &[u32],
    params: &ForceParams,
) -> Vec<[f32; 2]> {
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(a, b) in edges {
        adj[a].push(b);
        adj[b].push(a);
    }
    let mut by_degree: Vec<usize> = (0..n).collect();
    by_degree.sort_by_key(|&i| (std::cmp::Reverse(degree[i]), i));
    let (mut order, mut seen) = (Vec::with_capacity(n), vec![false; n]);
    for root in by_degree {
        if seen[root] {
            continue;
        }
        seen[root] = true;
        let mut queue = std::collections::VecDeque::from([root]);
        while let Some(i) = queue.pop_front() {
            order.push(i);
            for &j in &adj[i] {
                if !seen[j] {
                    seen[j] = true;
                    queue.push_back(j);
                }
            }
        }
    }
    // Even spacing `s` along the curve and between its turns fills a disc of the
    // same radius the start spread asks for.
    let radius = params.link_distance * (n as f32).sqrt().clamp(1.0, params.start_radius.max(1.0));
    let s = (radius * (std::f32::consts::PI / n.max(1) as f32).sqrt()).max(1e-3);
    let mut pos = vec![[0.0f32; 2]; n];
    for (k, &i) in order.iter().enumerate() {
        let theta = (4.0 * std::f32::consts::PI * k as f32).sqrt();
        let r = s * theta / std::f32::consts::TAU;
        pos[i] = [r * theta.cos(), r * theta.sin()];
    }
    pos
}

/// Force constants. Names follow the settings both references expose, so a
/// later settings panel has somewhere to land.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForceParams {
    /// Gravity toward the origin, per unit of a node's mass and of constant
    /// strength at any distance (ForceAtlas2's non-strong gravity). It keeps
    /// components together without squeezing the core into a disc.
    pub center: f32,
    /// A gentle linear pull on top (ForceAtlas2's strong gravity), so small
    /// disconnected components come home instead of drifting off the board.
    pub gravity_linear: f32,
    /// Charge between every pair, as `repel · m_i · m_j / d` with `m = degree + 1`
    /// (ForceAtlas2): a hub pushes in proportion to how connected it is, so it
    /// claims room instead of collapsing into a blob.
    pub repel: f32,
    /// Spring scale on a link. Each link's own constant is this over the smaller
    /// endpoint degree (d3's default), so a hub's many links do not crush it.
    pub link: f32,
    /// Rest length of a link.
    pub link_distance: f32,
    /// Widest the starting spiral gets, in link lengths. A small graph starts
    /// at its natural size (a link length per square root of its node count);
    /// a big one starts no wider than this and lets the charge open it out, the
    /// way d3 starts compact. A spiral as wide as the finished graph left a big
    /// vault's linked notes too far apart ever to meet.
    pub start_radius: f32,
    /// Velocity retained per tick. Under 1.0 this is what makes the settle
    /// damped instead of a big bang that never stops.
    pub damping: f32,
    pub max_velocity: f32,
    pub dt: f32,
    /// Temperature below which the graph is at rest and ticking stops.
    pub alpha_min: f32,
    /// How fast the temperature falls toward its target each tick: d3's
    /// `alphaDecay`. Forces are scaled by the temperature, so the layout always
    /// eases to a stop on a fixed schedule, whatever residual force is left.
    pub alpha_decay: f32,
    /// Temperature held while the operator drags a node: warm enough for the
    /// neighbours to follow, cool enough that nothing else wanders.
    pub alpha_drag: f32,
    /// Barnes-Hut opening angle. Smaller is more exact and slower.
    pub theta: f32,
}

impl Default for ForceParams {
    fn default() -> Self {
        Self {
            center: 0.1,
            gravity_linear: 0.002,
            repel: 10.0,
            link: 1.0,
            link_distance: 100.0,
            start_radius: START_RADIUS,
            // Momentum lets a big vault unfold from its spiral while it is still
            // warm; the cooling schedule, not friction, is what brings it to rest.
            damping: 0.85,
            // Fast enough for a compact start to open out into clusters before
            // it cools; at 40 a big vault froze half way.
            max_velocity: 200.0,
            dt: 0.3,
            alpha_min: 0.001,
            // From 1 to `alpha_min` in about 600 ticks: twice d3's default, so a
            // vault of thousands of notes spreads out before it freezes. The
            // settle runs off the paint thread, so the operator sees the result.
            alpha_decay: 1.0 - 0.001f32.powf(1.0 / 600.0),
            alpha_drag: 0.12,
            // 0.8 is the reference value and it converges. A coarser angle
            // saves a little tick time and leaves a residual energy that never
            // falls under `alpha_min`, which `the_settle_is_damped_and_ends`
            // catches: a graph that jitters forever is not a faster graph.
            theta: 0.8,
        }
    }
}

/// A ticking 2D layout. Positions start on a deterministic spiral, never on
/// hardcoded coordinates, so the settled shape comes from the forces.
#[derive(Debug, Clone)]
pub struct ForceSim {
    pub params: ForceParams,
    pos: Vec<[f32; 2]>,
    vel: Vec<[f32; 2]>,
    force: Vec<[f32; 2]>,
    edges: Vec<(usize, usize)>,
    /// Nodes the operator is holding. They push on everyone else and are never
    /// pushed themselves, which is d3's `fx`/`fy`.
    pinned: Vec<bool>,
    /// Links per node, and the mass repulsion and gravity read: `degree + 1`.
    degree: Vec<u32>,
    weight: Vec<f32>,
    /// Isolated notes in a graph that has links. They take no part in the forces;
    /// [`ForceSim::place_isolates`] lays them on a belt around the core instead.
    inert: Vec<bool>,
    energy: f32,
    /// Temperature: every force is scaled by it, and it eases toward `alpha_target`.
    alpha: f32,
    alpha_target: f32,
    tree: QuadTree,
    /// Each node's linked neighbours, for freeing a neighbourhood.
    adj: Vec<Vec<usize>>,
    /// While an interaction settles: the only nodes that may move. `None` lets
    /// every node move, as a fresh or remembered layout does.
    mobile: Option<Vec<bool>>,
    /// Nodes freed since the layout last rested, against [`MOBILE_CAP`].
    freed: usize,
}

impl ForceSim {
    /// `edges` are index pairs into the node list; out-of-range pairs and
    /// self-loops are dropped, because a snapshot may be filtered after it is
    /// built and a spring on one node is not a force.
    pub fn new(node_count: usize, edges: &[(usize, usize)], params: ForceParams) -> Self {
        // One spring per linked pair: A→B, B→A and a link written twice are the
        // same pair, and counting them twice would double their pull and degree.
        let mut edges: Vec<(usize, usize)> = edges
            .iter()
            .copied()
            .filter(|(a, b)| a != b && *a < node_count && *b < node_count)
            .map(|(a, b)| (a.min(b), a.max(b)))
            .collect();
        edges.sort_unstable();
        edges.dedup();
        let mut degree = vec![0u32; node_count];
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); node_count];
        for &(a, b) in &edges {
            degree[a] += 1;
            degree[b] += 1;
            adj[a].push(b);
            adj[b].push(a);
        }
        let pos = start_positions(node_count, &edges, &degree, &params);
        let weight = degree.iter().map(|d| 1.0 + *d as f32).collect();
        let inert = degree.iter().map(|d| *d == 0 && !edges.is_empty()).collect();
        Self {
            params,
            vel: vec![[0.0; 2]; node_count],
            force: vec![[0.0; 2]; node_count],
            pinned: vec![false; node_count],
            degree,
            weight,
            inert,
            pos,
            edges,
            energy: 1.0,
            alpha: 1.0,
            alpha_target: 0.0,
            tree: QuadTree::default(),
            adj,
            mobile: None,
            freed: 0,
        }
    }

    /// A sim with nothing in it, for a layout that does not use forces.
    pub fn empty() -> Self {
        Self::new(0, &[], ForceParams::default())
    }

    /// Build straight from an engine snapshot. Node order is the snapshot's, so
    /// `positions()[i]` belongs to `snapshot.nodes[i]`.
    pub fn from_snapshot(snap: &GraphSnapshot, params: ForceParams) -> Self {
        let index: std::collections::HashMap<&str, usize> =
            snap.nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
        let edges: Vec<(usize, usize)> = snap
            .edges
            .iter()
            .filter_map(|e| Some((*index.get(e.src.as_str())?, *index.get(e.dst.as_str())?)))
            .collect();
        Self::new(snap.nodes.len(), &edges, params)
    }

    pub fn len(&self) -> usize {
        self.pos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pos.is_empty()
    }

    pub fn positions(&self) -> &[[f32; 2]] {
        &self.pos
    }

    /// Mean kinetic energy over the nodes. Falling energy is a settling graph.
    pub fn energy(&self) -> f32 {
        self.energy
    }

    /// Move a node and wake the sim, so the rest of the graph settles around it.
    pub fn place(&mut self, i: usize, x: f32, y: f32) {
        if let Some(p) = self.pos.get_mut(i) {
            *p = [x, y];
            self.vel[i] = [0.0; 2];
            self.wake();
        }
    }

    /// Move a node the operator is dragging. Unlike [`ForceSim::place`] this
    /// frees only its neighbourhood: the rest of the map holds still, so a drag
    /// never reorganises the whole graph.
    pub fn drag_to(&mut self, i: usize, x: f32, y: f32) {
        self.free_around(&[i]);
        self.place(i, x, y);
    }

    /// Hold a node still. It keeps pushing on its neighbours; nothing pushes it.
    /// Releasing one frees its neighbourhood to settle round it.
    pub fn set_pinned(&mut self, i: usize, pinned: bool) {
        if let Some(p) = self.pinned.get_mut(i) {
            *p = pinned;
            if !pinned {
                self.free_around(&[i]);
                self.wake();
            }
        }
    }

    /// Let `roots` and the nodes within [`MOBILE_HOPS`] links of them move until
    /// the layout rests again, at most [`MOBILE_CAP`] of them; everything else
    /// stays where it is.
    fn free_around(&mut self, roots: &[usize]) {
        let n = self.pos.len();
        let mobile = self.mobile.get_or_insert_with(|| vec![false; n]);
        let mut frontier: Vec<usize> = roots.iter().copied().filter(|&i| i < n).collect();
        for &i in &frontier {
            mobile[i] = true;
        }
        for _ in 0..MOBILE_HOPS {
            let mut next = Vec::new();
            for &i in &frontier {
                for &j in &self.adj[i] {
                    // The cap runs until the layout rests, not per call: a
                    // drag of many ticks or Unpin all stays a local edit.
                    if !mobile[j] && self.freed < MOBILE_CAP {
                        mobile[j] = true;
                        self.freed += 1;
                        next.push(j);
                    }
                }
            }
            frontier = next;
        }
    }

    pub fn is_pinned(&self, i: usize) -> bool {
        self.pinned.get(i).copied().unwrap_or(false)
    }

    pub fn unpin_all(&mut self) {
        let held: Vec<usize> = (0..self.pinned.len()).filter(|&i| self.pinned[i]).collect();
        self.pinned.iter_mut().for_each(|p| *p = false);
        self.free_around(&held);
        self.wake();
    }

    /// Warm the layout a little after an interaction, so the graph settles
    /// around the change and then stops again.
    pub fn wake(&mut self) {
        self.alpha = self.alpha.max(self.params.alpha_drag);
    }

    /// Start no hotter than `alpha`: a layout seeded from remembered positions
    /// only needs to settle the seams, not unfold from scratch.
    pub fn cool_to(&mut self, alpha: f32) {
        self.alpha = self.alpha.min(alpha);
    }

    /// Hold the layout warm while a node is being dragged, and let it cool once
    /// the drag ends.
    pub fn hold_warm(&mut self, on: bool) {
        self.alpha_target = if on { self.params.alpha_drag } else { 0.0 };
        if on {
            self.wake();
        }
    }

    /// Current temperature, for the debug overlay.
    pub fn alpha(&self) -> f32 {
        self.alpha
    }

    /// True while the sim still has heat to spend, so a caller can tell
    /// whether the next frame is already scheduled.
    pub fn is_running(&self) -> bool {
        !self.pos.is_empty()
            && (self.alpha >= self.params.alpha_min || self.alpha_target >= self.params.alpha_min)
    }

    /// Bounding box of the current positions, for fitting a camera to it.
    pub fn extent(&self) -> ([f32; 2], [f32; 2]) {
        let mut lo = [0.0f32; 2];
        let mut hi = [0.0f32; 2];
        for (k, p) in self.pos.iter().enumerate() {
            if k == 0 {
                lo = *p;
                hi = *p;
            } else {
                lo = [lo[0].min(p[0]), lo[1].min(p[1])];
                hi = [hi[0].max(p[0]), hi[1].max(p[1])];
            }
        }
        (lo, hi)
    }

    /// One step. Returns false once the graph is at rest, so a render loop can
    /// stop asking.
    pub fn tick(&mut self) -> bool {
        let n = self.pos.len();
        if !self.is_running() {
            // At rest the next interaction starts from a still map.
            self.mobile = None;
            self.freed = 0;
            return false;
        }
        self.alpha += (self.alpha_target - self.alpha) * self.params.alpha_decay;
        let alpha = self.alpha;
        self.force.fill([0.0; 2]);

        // charge: ForceAtlas2's degree-weighted repulsion, `repel · m_i · m_j / d`.
        // Exact while the vault is small, Barnes-Hut once it is not.
        let repel = self.params.repel * alpha;
        // Pairs closer than this count as this close. Small: a compact start packs
        // notes well inside a link length, and a wide floor there turned the
        // push between them to nothing, so a small graph never opened out.
        let near = self.params.link_distance * NEAR_SHARE;
        if n > BARNES_HUT_THRESHOLD {
            self.tree.build(&self.pos, &self.weight, &self.inert);
            let charge = Charge { strength: repel, theta: self.params.theta, near };
            // Each node's charge is read-only over the tree, so the nodes split
            // across cores in chunks, one traversal stack per chunk.
            let (tree, pos, weight, inert) = (&self.tree, &self.pos, &self.weight, &self.inert);
            let mobile = self.mobile.as_deref();
            let repel_all = |force: &mut [[f32; 2]]| {
                force.par_chunks_mut(REPULSION_CHUNK).enumerate().for_each(|(c, chunk)| {
                    let mut stack = Vec::with_capacity(64);
                    for (k, f) in chunk.iter_mut().enumerate() {
                        let i = c * REPULSION_CHUNK + k;
                        // A frozen node feels no force worth computing.
                        if inert[i] || mobile.is_some_and(|m| !m[i]) {
                            continue;
                        }
                        let r = tree.repulsion(pos[i], i as u32, charge, &mut stack);
                        f[0] += r[0] * weight[i];
                        f[1] += r[1] * weight[i];
                    }
                })
            };
            match layout_pool() {
                Some(pool) => pool.install(|| repel_all(&mut self.force)),
                None => repel_all(&mut self.force),
            }
        } else {
            for i in 0..n {
                if self.inert[i] {
                    continue;
                }
                let pi = self.pos[i];
                let mut fi = [0.0f32; 2];
                for j in (i + 1)..n {
                    if self.inert[j] {
                        continue;
                    }
                    let dx = pi[0] - self.pos[j][0];
                    let dy = pi[1] - self.pos[j][1];
                    let d2 = (dx * dx + dy * dy).max(near * near);
                    let k = repel * self.weight[i] * self.weight[j] / d2;
                    fi[0] += dx * k;
                    fi[1] += dy * k;
                    self.force[j][0] -= dx * k;
                    self.force[j][1] -= dy * k;
                }
                self.force[i][0] += fi[0];
                self.force[i][1] += fi[1];
            }
        }

        // link: a spring toward the rest length, pulling when long and pushing
        // when short.
        let (k, rest) = (self.params.link * alpha, self.params.link_distance);
        for &(a, b) in &self.edges {
            let dx = self.pos[b][0] - self.pos[a][0];
            let dy = self.pos[b][1] - self.pos[a][1];
            let d = (dx * dx + dy * dy).sqrt().max(0.1);
            let ke = k / self.degree[a].min(self.degree[b]).max(1) as f32;
            let f = ke * (d - rest) / d;
            self.force[a][0] += dx * f;
            self.force[a][1] += dy * f;
            self.force[b][0] -= dx * f;
            self.force[b][1] -= dy * f;
        }

        // center: a constant pull toward the origin per unit of mass, which keeps
        // components on screen without the linear spring that packed the core.
        let g = self.params.center * alpha;
        for i in 0..n {
            let p = self.pos[i];
            let r = (p[0] * p[0] + p[1] * p[1]).sqrt().max(1.0);
            let pull = (g / r + self.params.gravity_linear * alpha) * self.weight[i];
            self.force[i][0] -= p[0] * pull;
            self.force[i][1] -= p[1] * pull;
        }

        // The speed cap cools with the layout: a cold start opens out at full
        // speed, but a drag or a warm start moves at most a fifth of it.
        let max_v = self.params.max_velocity * alpha.clamp(WARM_SPEED_SHARE, 1.0);
        let (dt, damping) = (self.params.dt, self.params.damping);
        let max_v2 = max_v * max_v;
        let mut total = 0.0f32;
        let mobile = self.mobile.as_deref();
        for (i, (((((p, v), f), held), inert), m)) in self
            .pos
            .iter_mut()
            .zip(self.vel.iter_mut())
            .zip(self.force.iter())
            .zip(self.pinned.iter())
            .zip(self.inert.iter())
            .zip(self.weight.iter())
            .enumerate()
        {
            if *held || *inert || mobile.is_some_and(|m| !m[i]) {
                *v = [0.0; 2];
                continue;
            }
            // A hub is heavy as well as loud: it pushes the others away in
            // proportion to its mass and moves under a force in inverse proportion,
            // so it anchors its cluster instead of being flung out by its own charge.
            let mut vx = (v[0] + f[0] / m * dt) * damping;
            let mut vy = (v[1] + f[1] / m * dt) * damping;
            let v2 = vx * vx + vy * vy;
            if v2 > max_v2 {
                let s = max_v / v2.sqrt();
                vx *= s;
                vy *= s;
            }
            total += vx * vx + vy * vy;
            *v = [vx, vy];
            p[0] += vx * dt;
            p[1] += vy * dt;
        }
        self.energy = total / n as f32;
        true
    }

    /// A node's share of the separation gap: busy notes keep more room.
    fn size(&self, i: usize) -> f32 {
        1.0 + (self.degree[i] as f32).sqrt() * 0.3
    }

    /// Push apart active nodes that sit closer than `unit · (size_i + size_j)`, for
    /// up to `passes` passes (Aquilum's separation pass, made size-aware). Run after
    /// the settle: forces place the clusters, this guarantees no two discs sit on
    /// top of each other. Small nodes are compared through a grid; the few big ones
    /// against everyone. Held nodes do not move.
    pub fn separate(&mut self, unit: f32, passes: usize) {
        let n = self.pos.len();
        if n < 2 || unit <= 0.0 {
            return;
        }
        let cell = unit * 2.0 * 2.0;
        let is_big: Vec<bool> = (0..n).map(|i| !self.inert[i] && self.size(i) > 2.0).collect();
        let big: Vec<usize> = (0..n).filter(|&i| is_big[i]).collect();
        let mut grid: std::collections::HashMap<(i32, i32), Vec<usize>> = std::collections::HashMap::new();
        for _ in 0..passes {
            grid.clear();
            for i in (0..n).filter(|i| !self.inert[*i] && self.size(*i) <= 2.0) {
                let p = self.pos[i];
                grid.entry(((p[0] / cell).floor() as i32, (p[1] / cell).floor() as i32)).or_default().push(i);
            }
            let mut pairs: Vec<(usize, usize)> = Vec::new();
            // Cells in a fixed order: each part moves the next pair's ends, and a
            // hash map's own order changes from run to run.
            let mut cells: Vec<(i32, i32)> = grid.keys().copied().collect();
            cells.sort_unstable();
            for (cx, cy) in cells {
                for &i in &grid[&(cx, cy)] {
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1), (1, -1)] {
                        let Some(others) = grid.get(&(cx + dx, cy + dy)) else { continue };
                        for &j in others {
                            if (dx, dy) != (0, 0) || j > i {
                                pairs.push((i, j));
                            }
                        }
                    }
                }
            }
            // A hub needs a wider gap than a grid cell, so it is checked against
            // every active node; a pair of hubs is checked once.
            for &i in &big {
                pairs.extend(
                    (0..n).filter(|&j| !self.inert[j] && j != i && !(is_big[j] && j < i)).map(|j| (i, j)),
                );
            }
            let mut moved = false;
            for (i, j) in pairs {
                moved |= self.part(i, j, unit * (self.size(i) + self.size(j)));
            }
            if !moved {
                break;
            }
        }
    }

    /// Move `i` and `j` apart along their line if they are closer than `gap`.
    fn part(&mut self, i: usize, j: usize, gap: f32) -> bool {
        let (a, b) = (self.pos[i], self.pos[j]);
        let (ox, oy) = (b[0] - a[0], b[1] - a[1]);
        let d = (ox * ox + oy * oy).sqrt();
        if d >= gap {
            return false;
        }
        // Coincident nodes part along a fixed per-pair direction.
        let (ux, uy) = if d > 1e-4 {
            (ox / d, oy / d)
        } else {
            let t = (i * 7 + j * 13) as f32;
            (t.cos(), t.sin())
        };
        let (fi, fj) = (!self.pinned[i], !self.pinned[j]);
        let push = (gap - d) * 0.55 * if fi && fj { 0.5 } else { 1.0 };
        if fi {
            self.pos[i] = [a[0] - ux * push, a[1] - uy * push];
        }
        if fj {
            self.pos[j] = [b[0] + ux * push, b[1] + uy * push];
        }
        fi || fj
    }

    /// Lay the isolated notes on a golden-angle belt just outside the core, `gap`
    /// apart, so they frame the graph instead of crowding it.
    pub fn place_isolates(&mut self, gap: f32) {
        let active: Vec<[f32; 2]> =
            self.pos.iter().zip(&self.inert).filter(|(_, i)| !**i).map(|(p, _)| *p).collect();
        if active.len() == self.pos.len() {
            return;
        }
        let c = active.iter().fold([0.0f32; 2], |s, p| [s[0] + p[0], s[1] + p[1]]);
        let c = [c[0] / active.len().max(1) as f32, c[1] / active.len().max(1) as f32];
        let mut radii: Vec<f32> = active.iter().map(|p| (p[0] - c[0]).hypot(p[1] - c[1])).collect();
        radii.sort_by(f32::total_cmp);
        let core = radii.get(radii.len() * 95 / 100).copied().unwrap_or(0.0);
        // An orphan gets a little more room than a connected note has in the core,
        // so the belt reads as the same sky, slightly thinner.
        let per = (std::f32::consts::PI * core * core / active.len() as f32 * 1.5).max(gap * gap);
        let core = core + gap * 3.0;
        let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
        let mut k = 0.0f32;
        for i in 0..self.pos.len() {
            if self.inert[i] && !self.pinned[i] {
                // Equal area per note: r grows with sqrt(k) from the core's edge.
                let r = (core * core + k * per / std::f32::consts::PI).sqrt();
                self.pos[i] = [c[0] + r * (k * golden).cos(), c[1] + r * (k * golden).sin()];
                k += 1.0;
            }
        }
    }

    /// Tick until the graph is at rest or `max_ticks` is spent. Returns the
    /// number of ticks actually run.
    pub fn settle(&mut self, max_ticks: usize) -> usize {
        let mut run = 0;
        while run < max_ticks && self.tick() {
            run += 1;
        }
        run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture with a hub, a chain and a disconnected pair, so the sim has
    /// both springs and free bodies to settle.
    fn fixture() -> ForceSim {
        let edges = [(0, 1), (0, 2), (0, 3), (1, 2), (3, 4), (4, 5), (6, 7)];
        ForceSim::new(9, &edges, ForceParams::default())
    }

    #[test]
    fn positions_move_then_energy_falls() {
        let mut sim = fixture();
        let start = sim.positions().to_vec();

        assert!(sim.tick(), "a fresh graph is not at rest");
        let after_one = sim.positions().to_vec();
        let moved = start.iter().zip(&after_one).filter(|(a, b)| a != b).count();
        // Node 8 has no links: it waits for the orphan belt instead of drifting.
        assert_eq!(moved, start.len() - 1, "every linked node moves on the first tick");
        assert_eq!(start[8], after_one[8], "an isolated note holds still");

        // Energy climbs while repulsion unwinds the seed spiral, so the early
        // reading is taken after the graph has opened up, not at t=0.
        for _ in 0..9 {
            sim.tick();
        }
        let early = sim.energy();
        assert!(early > 0.0 && early.is_finite());

        let ran = sim.settle(4_000);
        let late = sim.energy();
        assert!(late < early, "energy falls: {early} -> {late} over {ran} ticks");
        assert!(late < sim.params.alpha_min, "the settle is damped, not endless jitter");
        assert!(!sim.tick(), "a graph at rest stops asking to be ticked");
        assert!(
            sim.positions().iter().all(|p| p[0].is_finite() && p[1].is_finite()),
            "no NaN escapes the integrator"
        );
    }

    #[test]
    fn seed_layout_is_derived_not_hardcoded() {
        let a = ForceSim::new(9, &[], ForceParams::default());
        let wide = ForceSim::new(9, &[], ForceParams { link_distance: 300.0, ..ForceParams::default() });
        assert_ne!(a.positions()[3], wide.positions()[3], "the spiral scales with link distance");
        assert!(a.positions().iter().all(|p| p[0].is_finite()));
        let mut ids: Vec<String> = a.positions().iter().map(|p| format!("{p:?}")).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 9, "no two nodes start on top of each other");
    }

    /// The link force has to actually pull, or the layout is just repulsion in a
    /// gravity well and the edges mean nothing. Measured on a crowd, because on
    /// three nodes gravity alone already sets the spacing near the rest length.
    #[test]
    fn a_hub_keeps_its_own_notes_round_it() {
        // Three hubs of 40 notes each, the hubs linked in a ring. Notes are
        // numbered so a hub's notes are not consecutive: the start must find the
        // clusters from the links, not from the ids.
        let (hubs, per) = (3usize, 40usize);
        let mut edges = vec![(0, 1), (1, 2), (2, 0)];
        for k in 0..hubs * per {
            edges.push((k % hubs, hubs + k));
        }
        let mut sim = ForceSim::new(hubs + hubs * per, &edges, ForceParams::default());
        sim.settle(4_000);
        let p = sim.positions();
        let d = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
        for h in 0..hubs {
            let (mut own, mut other) = (Vec::new(), Vec::new());
            for k in 0..hubs * per {
                let dist = d(p[h], p[hubs + k]);
                if k % hubs == h { own.push(dist) } else { other.push(dist) }
            }
            let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
            assert!(
                mean(&own) * 1.5 < mean(&other),
                "hub {h}: own notes at {} should sit well inside the others at {}",
                mean(&own),
                mean(&other)
            );
        }
    }

    #[test]
    fn linked_neighbours_settle_closer_than_the_crowd() {
        let chain: Vec<(usize, usize)> = (0..9).map(|i| (i, i + 1)).collect();
        let mut sim = ForceSim::new(40, &chain, ForceParams::default());
        sim.settle(6_000);
        let p = sim.positions();
        let d = |a: [f32; 2], b: [f32; 2]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();

        let linked: f32 = chain.iter().map(|&(a, b)| d(p[a], p[b])).sum::<f32>() / chain.len() as f32;
        let mut all = Vec::new();
        for i in 0..p.len() {
            for j in (i + 1)..p.len() {
                all.push(d(p[i], p[j]));
            }
        }
        let crowd: f32 = all.iter().sum::<f32>() / all.len() as f32;
        assert!(linked < crowd, "linked mean {linked} should sit under crowd mean {crowd}");
    }

    #[test]
    fn no_javascript_graph_library_is_a_dependency() {
        for toml in [include_str!("../Cargo.toml"), include_str!("../../../Cargo.toml")] {
            let lower = toml.to_lowercase();
            for banned in ["pixi", "cytoscape", "d3-force", "juggl", "electron", "webview"] {
                assert!(!lower.contains(banned), "`{banned}` must not be a dependency");
            }
        }
    }

    /// G3d: the settle is damped, and it stops. A graph that never falls under
    /// `alpha_min` is the "big bang then jitter forever" the visual bar rules out.
    #[test]
    fn the_settle_is_damped_and_ends() {
        for n in [9usize, 200] {
            let edges: Vec<(usize, usize)> = (0..n - 1).map(|i| (i, i + 1)).collect();
            let mut sim = ForceSim::new(n, &edges, ForceParams::default());
            let ticks = sim.settle(20_000);
            assert!(ticks < 20_000, "{n} nodes never came to rest");
            assert!(sim.energy() < sim.params.alpha_min, "{n} nodes still jittering");

            // At rest, another thousand ticks move nothing.
            let before = sim.positions().to_vec();
            assert_eq!(sim.settle(1_000), 0, "a graph at rest does not ask to tick");
            assert_eq!(sim.positions(), before.as_slice());

            // And an interaction wakes it again rather than leaving it dead.
            sim.place(0, 900.0, 900.0);
            assert!(sim.tick(), "moving a node wakes the sim");
            assert!(sim.settle(20_000) < 20_000, "and it settles again");
        }
    }

    /// The tree is an optimisation, not different physics. `theta = 0` never
    /// satisfies the opening test, so it walks to the leaves and is the exact
    /// sum through the same code path.
    #[test]
    fn barnes_hut_agrees_with_the_exact_sum() {
        let n = BARNES_HUT_THRESHOLD * 2;
        let edges: Vec<(usize, usize)> = (0..n - 1).map(|i| (i, i + 1)).collect();
        // The tree's accuracy, not the layout: from a wide start the cells are
        // well separated, which is what the opening angle is tuned for.
        let tree_params = ForceParams { start_radius: 100.0, ..ForceParams::default() };
        let exact_params = ForceParams { theta: 0.0, ..tree_params };
        let mut tree_sim = ForceSim::new(n, &edges, tree_params);
        let mut exact_sim = ForceSim::new(n, &edges, exact_params);
        assert_eq!(tree_sim.positions(), exact_sim.positions(), "same start");

        // One tick isolates the force error from the chaos a long run adds.
        tree_sim.tick();
        exact_sim.tick();
        let worst = tree_sim
            .positions()
            .iter()
            .zip(exact_sim.positions())
            .map(|(a, b)| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.25, "one tick of tree repulsion drifted {worst} units");

        // And over a full settle it lands on the same shape, judged by the
        // measures that describe a layout rather than by node coordinates,
        // which a force sim is free to permute.
        tree_sim.settle(3_000);
        exact_sim.settle(3_000);
        let mean_link = |s: &ForceSim| {
            let p = s.positions();
            edges
                .iter()
                .map(|&(a, b)| ((p[a][0] - p[b][0]).powi(2) + (p[a][1] - p[b][1]).powi(2)).sqrt())
                .sum::<f32>()
                / edges.len() as f32
        };
        let span = |s: &ForceSim| {
            let (lo, hi) = s.extent();
            (hi[0] - lo[0]).max(hi[1] - lo[1])
        };
        let (lt, le) = (mean_link(&tree_sim), mean_link(&exact_sim));
        let (st, se) = (span(&tree_sim), span(&exact_sim));
        assert!((lt - le).abs() / le < 0.2, "mean link {lt} vs {le}");
        assert!((st - se).abs() / se < 0.3, "span {st} vs {se}");
        assert!(tree_sim.positions().iter().all(|p| p[0].is_finite() && p[1].is_finite()));
    }

    /// A deterministic 2 000-node / 4 000-edge graph, the size the perf bar names.
    fn big(n: usize, m: usize) -> ForceSim {
        // A cheap reproducible spread: a spanning chain so nothing is isolated,
        // then extra chords from a linear congruential sequence.
        let mut edges: Vec<(usize, usize)> = (0..n - 1).map(|i| (i, i + 1)).collect();
        let mut r: u64 = 0x2545_F491_4F6C_DD1D;
        while edges.len() < m {
            r = r.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let a = (r >> 33) as usize % n;
            r = r.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let b = (r >> 33) as usize % n;
            if a != b {
                edges.push((a, b));
            }
        }
        ForceSim::new(n, &edges, ForceParams::default())
    }

    /// G3c's budget, measured rather than felt. Timing depends on the machine,
    /// so it is `--ignored`: run it with `--release --ignored` to get a number,
    /// and read the on-screen overlay for the number that actually counts.
    #[test]
    #[ignore = "timing; run with --release -- --ignored --nocapture"]
    fn two_thousand_nodes_tick_inside_a_frame() {
        let mut sim = big(2_000, 4_000);
        // warm the tree and get past the opening burst
        for _ in 0..20 {
            sim.tick();
        }
        let runs = 120;
        let t0 = std::time::Instant::now();
        for _ in 0..runs {
            sim.tick();
        }
        let per_tick = t0.elapsed().as_secs_f64() * 1000.0 / runs as f64;
        println!("2000 nodes / 4000 edges: {per_tick:.3} ms per tick");
        assert!(per_tick < 16.6, "{per_tick:.3} ms per tick misses the 60 fps budget");
    }

    /// The layout cools on a schedule: a big graph comes to rest in about 600
    /// ticks however much residual force is left, sits still, and a drag warms
    /// it only until the drag ends.
    #[test]
    fn the_layout_cools_to_stillness_and_a_drag_only_warms_it_while_held() {
        // Past the Barnes-Hut threshold, but small enough for CI's per-test budget.
        let mut sim = big(300, 450);
        let ran = sim.settle(5_000);
        assert!(ran <= 620, "a cooling schedule ends near 600 ticks, ran {ran}");
        assert!(!sim.is_running(), "at rest the sim stops asking for frames");
        let still = sim.positions().to_vec();
        assert!(!sim.tick(), "a cooled graph does not tick");
        assert_eq!(sim.positions(), still.as_slice(), "and nothing drifts");

        sim.place(0, 500.0, 500.0);
        sim.set_pinned(0, true);
        sim.hold_warm(true);
        for _ in 0..200 {
            assert!(sim.tick(), "a held drag keeps the neighbours following");
        }
        assert!(sim.alpha() <= sim.params.alpha_drag + 1e-6, "a drag is warm, never a fresh big bang");
        sim.hold_warm(false);
        let cooled = sim.settle(5_000);
        assert!(cooled <= 620, "after the drop it cools again, ran {cooled}");
        assert_eq!(sim.positions()[0], [500.0, 500.0], "the dropped node stays where it was put");
    }

    #[test]
    fn a_drag_moves_its_neighbourhood_and_leaves_the_rest_of_the_map_still() {
        let mut sim = big(300, 450);
        sim.settle(5_000);
        let before = sim.positions().to_vec();
        let near: std::collections::HashSet<usize> = {
            let mut s: std::collections::HashSet<usize> = std::collections::HashSet::from([0]);
            for _ in 0..MOBILE_HOPS {
                let next: Vec<usize> = s.iter().flat_map(|&i| sim.adj[i].clone()).collect();
                s.extend(next);
            }
            s
        };
        sim.hold_warm(true);
        for k in 0..60 {
            sim.drag_to(0, before[0][0] + k as f32 * 4.0, before[0][1]);
            sim.set_pinned(0, true);
            sim.tick();
        }
        sim.hold_warm(false);
        sim.settle(5_000);
        let after = sim.positions();
        let far_moved = (0..after.len()).filter(|i| !near.contains(i) && after[*i] != before[*i]).count();
        assert_eq!(far_moved, 0, "nothing beyond two links of the drag moves");
        let near_moved = near.iter().filter(|&&i| i != 0 && after[i] != before[i]).count();
        assert!(near_moved > 0, "its neighbours follow it");
        // At rest the map is free again for the next layout or interaction.
        assert!(sim.mobile.is_none() || !sim.tick());
    }

    #[test]
    fn a_pinned_node_holds_still_and_the_rest_moves_around_it() {
        let mut sim = fixture();
        sim.settle(4_000);
        sim.place(0, 120.0, -60.0);
        sim.set_pinned(0, true);
        assert!(sim.is_pinned(0));
        let others = sim.positions().to_vec();
        sim.settle(4_000);
        assert_eq!(sim.positions()[0], [120.0, -60.0], "the held node does not drift");
        assert!(
            sim.positions().iter().skip(1).zip(others.iter().skip(1)).any(|(a, b)| a != b),
            "everything else settles around it"
        );
        sim.unpin_all();
        assert!(!sim.is_pinned(0));
        sim.settle(4_000);
        assert_ne!(sim.positions()[0], [120.0, -60.0], "released, it rejoins the layout");
    }
}
