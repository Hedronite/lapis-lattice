//! Barnes-Hut quadtree for the force layout's repulsion.

/// How one tick's repulsion is applied: its strength (already scaled by the
/// temperature), the Barnes-Hut opening angle, and the distance under which a
/// pair counts as that close.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Charge {
    pub strength: f32,
    pub theta: f32,
    pub near: f32,
}

/// A cell smaller than this stops subdividing. Without it, two nodes at the
/// same point would split forever.
const MIN_CELL: f32 = 1e-3;
const EMPTY: u32 = u32::MAX;

/// Barnes-Hut quadtree over the current positions, stored as parallel arrays
/// and rebuilt every tick.
///
/// Flattened from the octree in `obsidian-3d-graph`'s `physics.rs`: same
/// centre-of-mass accumulation on insert, same `size / distance < theta`
/// opening angle on traversal, two dimensions instead of three.
#[derive(Debug, Default, Clone)]
pub(crate) struct QuadTree {
    com: Vec<[f32; 2]>,
    mass: Vec<f32>,
    /// Cell width, for the opening-angle test.
    size: Vec<f32>,
    center: Vec<[f32; 2]>,
    children: Vec<[u32; 4]>,
    /// The single body in a leaf, or `EMPTY`.
    body: Vec<u32>,
    child_count: Vec<u8>,
    insert_stack: Vec<(u32, u32, [f32; 2])>,
}

impl QuadTree {
    fn clear(&mut self) {
        self.com.clear();
        self.mass.clear();
        self.size.clear();
        self.center.clear();
        self.children.clear();
        self.body.clear();
        self.child_count.clear();
    }

    fn push_cell(&mut self, center: [f32; 2], size: f32) -> u32 {
        let i = self.mass.len() as u32;
        self.com.push([0.0; 2]);
        self.mass.push(0.0);
        self.size.push(size);
        self.center.push(center);
        self.children.push([EMPTY; 4]);
        self.body.push(EMPTY);
        self.child_count.push(0);
        i
    }

    #[inline]
    fn quadrant(&self, cell: u32, p: [f32; 2]) -> usize {
        let c = self.center[cell as usize];
        usize::from(p[0] >= c[0]) | (usize::from(p[1] >= c[1]) << 1)
    }

    fn child(&mut self, cell: u32, q: usize) -> u32 {
        let i = cell as usize;
        if self.children[i][q] != EMPTY {
            return self.children[i][q];
        }
        let half = self.size[i] * 0.5;
        let c = self.center[i];
        let quarter = half * 0.5;
        let center = [
            if q & 1 != 0 { c[0] + quarter } else { c[0] - quarter },
            if q & 2 != 0 { c[1] + quarter } else { c[1] - quarter },
        ];
        let new = self.push_cell(center, half);
        self.children[i][q] = new;
        self.child_count[i] += 1;
        new
    }

    /// `weight[i]` is body i's mass; bodies with `skip[i]` set are left out.
    pub(crate) fn build(&mut self, pos: &[[f32; 2]], weight: &[f32], skip: &[bool]) {
        self.clear();
        let mut live = pos.iter().zip(skip).filter(|(_, s)| !**s).map(|(p, _)| *p);
        let Some(first) = live.next() else { return };
        let (mut lo, mut hi) = (first, first);
        for p in live {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
        let center = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
        // Square root cell, with slack so a body on the boundary still lands
        // inside it.
        let size = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1.0) * 1.01;
        self.push_cell(center, size);
        for (i, &p) in pos.iter().enumerate() {
            if !skip[i] {
                self.insert(i as u32, p, pos, weight);
            }
        }
    }

    fn insert(&mut self, body: u32, p: [f32; 2], pos: &[[f32; 2]], weight: &[f32]) {
        self.insert_stack.clear();
        self.insert_stack.push((0, body, p));
        while let Some((cell, body, p)) = self.insert_stack.pop() {
            let i = cell as usize;
            let m = self.mass[i];
            let w = weight[body as usize];
            let next = m + w;
            self.com[i] = [(self.com[i][0] * m + p[0] * w) / next, (self.com[i][1] * m + p[1] * w) / next];
            self.mass[i] = next;

            let held = self.body[i];
            if held == EMPTY && self.child_count[i] == 0 {
                self.body[i] = body;
                continue;
            }
            // Two bodies on top of each other: stop splitting and let the mass
            // speak for both. Their mutual force is softened to nothing anyway.
            if self.size[i] < MIN_CELL {
                continue;
            }
            if held != EMPTY {
                self.body[i] = EMPTY;
                let hp = pos[held as usize];
                let q = self.quadrant(cell, hp);
                let c = self.child(cell, q);
                self.insert_stack.push((c, held, hp));
            }
            let q = self.quadrant(cell, p);
            let c = self.child(cell, q);
            self.insert_stack.push((c, body, p));
        }
    }

    /// Repulsion per unit of `body`'s own mass at `p`, opening a cell only when it
    /// subtends more than `charge.theta`. The force falls off as 1/d (ForceAtlas2), and
    /// distances under `charge.near` count as that, so close pairs get a firm, finite shove.
    ///
    /// Read-only over the tree, with the caller's `stack`, so many threads can
    /// walk one tree at once.
    pub(crate) fn repulsion(&self, p: [f32; 2], body: u32, charge: Charge, stack: &mut Vec<u32>) -> [f32; 2] {
        let Charge { strength, theta, near } = charge;
        let mut acc = [0.0f32; 2];
        if self.mass.is_empty() {
            return acc;
        }
        stack.clear();
        stack.push(0);
        while let Some(cell) = stack.pop() {
            let i = cell as usize;
            let m = self.mass[i];
            if m == 0.0 {
                continue;
            }
            let d = [p[0] - self.com[i][0], p[1] - self.com[i][1]];
            let d2 = (d[0] * d[0] + d[1] * d[1]).max(1.0);
            let dist = d2.sqrt();
            if self.child_count[i] == 0 || (self.size[i] / dist) < theta {
                if self.body[i] != body {
                    let f = strength * m / d2.max(near * near);
                    acc[0] += d[0] * f;
                    acc[1] += d[1] * f;
                }
            } else {
                for &c in &self.children[i] {
                    if c != EMPTY {
                        stack.push(c);
                    }
                }
            }
        }
        acc
    }
}
