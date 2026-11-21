//! Rebuilds surfaces from a mesh of plate elements (what STAAD, Robot, Prokon, ... export).
//!
//! 1. Plates are grouped by plane (normal and offset within tolerances) and by property
//!    (thickness / material key): only alike plates may form one surface.
//! 2. Within a plane, plates that share an edge form one patch (union-find over edges).
//! 3. A patch's boundary edges are the edges only one of its plates uses. Chained, they form
//!    closed loops; the loop with the largest area is the surface outline, the others are
//!    openings.
//! 4. Loops are simplified: a node is kept where the outline turns, or where it is a
//!    "feature" node (a corner of another surface, a member end, a support). Nodes along a
//!    straight edge are dropped, so a meshed slab becomes one surface with few lines.

use std::collections::{BTreeMap, BTreeSet, HashMap};

type P3 = [f64; 3];

/// A plate of the mesh.
pub struct Plate {
    pub id: u32,
    pub nodes: Vec<u32>,
    /// Plates only merge with plates of the same key (e.g. thickness + material).
    pub key: u32,
}

/// One rebuilt surface: node loops (unsimplified, in order).
#[derive(Debug, Clone)]
pub struct Region {
    pub key: u32,
    pub plates: Vec<u32>,
    pub outer: Vec<u32>,
    pub holes: Vec<Vec<u32>>,
    /// Unit normal of the plane.
    pub normal: P3,
}

fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: P3, b: P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: P3, b: P3) -> P3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: P3) -> f64 {
    dot(a, a).sqrt()
}

/// Newell normal of a polygon (area-weighted, also for warped quads); length = 2 x area.
fn newell(pts: &[P3]) -> P3 {
    let mut n = [0.0; 3];
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    n
}

/// Makes the normal's sign canonical (first clearly non-zero component positive).
fn canonical(mut n: P3) -> P3 {
    let len = norm(n);
    if len > 0.0 {
        n = [n[0] / len, n[1] / len, n[2] / len];
    }
    if let Some(c) = n.iter().find(|c| c.abs() > 1e-6)
        && *c < 0.0
    {
        n = [-n[0], -n[1], -n[2]];
    }
    n
}

struct Dsu(Vec<usize>);
impl Dsu {
    fn find(&mut self, i: usize) -> usize {
        let mut r = i;
        while self.0[r] != r {
            r = self.0[r];
        }
        let mut c = i;
        while self.0[c] != r {
            let next = self.0[c];
            self.0[c] = r;
            c = next;
        }
        r
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a] = b;
        }
    }
}

/// Groups plates into surfaces. `tol` is the distance tolerance (m) for "in the same plane".
pub fn rebuild(nodes: &BTreeMap<u32, P3>, plates: &[Plate], tol: f64) -> Vec<Region> {
    let pos = |n: u32| nodes.get(&n).copied();
    // Plane of each usable plate.
    let mut planes: Vec<(usize, P3, f64)> = Vec::new();
    for (i, p) in plates.iter().enumerate() {
        let Some(pts) = p.nodes.iter().map(|n| pos(*n)).collect::<Option<Vec<_>>>() else {
            continue;
        };
        let n = newell(&pts);
        if norm(n) < 1e-12 {
            continue; // degenerate
        }
        let n = canonical(n);
        let c = pts
            .iter()
            .fold([0.0; 3], |s, q| [s[0] + q[0], s[1] + q[1], s[2] + q[2]]);
        let k = pts.len() as f64;
        planes.push((i, n, dot(n, [c[0] / k, c[1] / k, c[2] / k])));
    }
    // 1. Plane groups (greedy clustering; meshes have few distinct planes).
    let cos_tol = 1.0 - 1e-5;
    let mut groups: Vec<(u32, P3, f64, Vec<usize>)> = Vec::new();
    for (i, n, d) in planes {
        let key = plates[i].key;
        match groups
            .iter_mut()
            .find(|g| g.0 == key && dot(g.1, n) > cos_tol && (g.2 - d).abs() < tol)
        {
            Some(g) => g.3.push(i),
            None => groups.push((key, n, d, vec![i])),
        }
    }
    let mut out = Vec::new();
    for (key, normal, _, members) in groups {
        // 2. Patches: plates sharing an edge.
        let mut dsu = Dsu((0..members.len()).collect());
        let mut edge_owner: HashMap<(u32, u32), usize> = HashMap::new();
        for (li, &pi) in members.iter().enumerate() {
            let ns = &plates[pi].nodes;
            for j in 0..ns.len() {
                let e = edge(ns[j], ns[(j + 1) % ns.len()]);
                if let Some(&o) = edge_owner.get(&e) {
                    dsu.union(li, o);
                } else {
                    edge_owner.insert(e, li);
                }
            }
        }
        let mut patches: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for li in 0..members.len() {
            let r = dsu.find(li);
            patches.entry(r).or_default().push(members[li]);
        }
        for patch in patches.values() {
            // 3. Boundary edges: used by one plate of the patch.
            let mut count: HashMap<(u32, u32), usize> = HashMap::new();
            for &pi in patch {
                let ns = &plates[pi].nodes;
                for j in 0..ns.len() {
                    *count
                        .entry(edge(ns[j], ns[(j + 1) % ns.len()]))
                        .or_default() += 1;
                }
            }
            let boundary: Vec<(u32, u32)> = count
                .into_iter()
                .filter(|(_, c)| *c == 1)
                .map(|(e, _)| e)
                .collect();
            let mut loops = chain(&boundary, nodes, normal);
            if loops.is_empty() {
                continue;
            }
            // Outline = largest area.
            loops.sort_by(|a, b| {
                loop_area(b, nodes, normal)
                    .abs()
                    .total_cmp(&loop_area(a, nodes, normal).abs())
            });
            let outer = loops.remove(0);
            let mut ids: Vec<u32> = patch.iter().map(|&pi| plates[pi].id).collect();
            ids.sort_unstable();
            out.push(Region {
                key,
                plates: ids,
                outer,
                holes: loops,
                normal,
            });
        }
    }
    out
}

fn edge(a: u32, b: u32) -> (u32, u32) {
    if a < b { (a, b) } else { (b, a) }
}

/// Signed area of a loop in its plane.
fn loop_area(l: &[u32], nodes: &BTreeMap<u32, P3>, normal: P3) -> f64 {
    let pts: Vec<P3> = l.iter().filter_map(|n| nodes.get(n).copied()).collect();
    dot(newell(&pts), normal) / 2.0
}

/// Chains undirected boundary edges into closed loops. Where more than two boundary edges
/// meet (patches touching at a corner), the walk takes the sharpest left turn so loops do
/// not cross.
fn chain(edges: &[(u32, u32)], nodes: &BTreeMap<u32, P3>, normal: P3) -> Vec<Vec<u32>> {
    let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(a, b) in edges {
        adj.entry(a).or_default().push(b);
        adj.entry(b).or_default().push(a);
    }
    let mut used: BTreeSet<(u32, u32)> = BTreeSet::new();
    let mut loops = Vec::new();
    let mut sorted: Vec<(u32, u32)> = edges.to_vec();
    sorted.sort_unstable();
    for &(a, b) in &sorted {
        if used.contains(&(a, b)) {
            continue;
        }
        used.insert((a, b));
        let mut lp = vec![a];
        let (mut prev, mut cur) = (a, b);
        let mut closed = false;
        for _ in 0..=edges.len() {
            if cur == a {
                closed = true;
                break;
            }
            lp.push(cur);
            let cands: Vec<u32> = adj[&cur]
                .iter()
                .copied()
                .filter(|&n| !used.contains(&edge(cur, n)))
                .collect();
            let next = match cands.len() {
                0 => break,
                1 => cands[0],
                _ => {
                    // Smallest turn angle measured around the normal.
                    let (Some(pp), Some(pc)) = (nodes.get(&prev), nodes.get(&cur)) else {
                        break;
                    };
                    let din = sub(*pc, *pp);
                    *cands
                        .iter()
                        .min_by(|&&x, &&y| {
                            let ang = |n: u32| {
                                let d = sub(nodes[&n], *pc);
                                dot(cross(din, d), normal).atan2(dot(din, d))
                            };
                            ang(y).total_cmp(&ang(x))
                        })
                        .unwrap()
                }
            };
            used.insert(edge(cur, next));
            prev = cur;
            cur = next;
        }
        if closed && lp.len() >= 3 {
            loops.push(lp);
        }
    }
    loops
}

/// Nodes of `l` where the loop turns (not collinear with its neighbours).
pub fn corners(l: &[u32], nodes: &BTreeMap<u32, P3>, tol: f64) -> Vec<u32> {
    let n = l.len();
    (0..n)
        .filter(|&i| {
            let (a, b, c) = (
                nodes[&l[(i + n - 1) % n]],
                nodes[&l[i]],
                nodes[&l[(i + 1) % n]],
            );
            let (u, v) = (sub(b, a), sub(c, a));
            let len = norm(v);
            // Distance of b from the line a-c, or a fold-back.
            len < 1e-12 || norm(cross(u, v)) / len > tol || dot(u, sub(c, b)) < 0.0
        })
        .map(|i| l[i])
        .collect()
}

/// Keeps only the nodes of `l` in `keep` (at least its corners), in loop order.
pub fn simplify(l: &[u32], keep: &BTreeSet<u32>) -> Vec<u32> {
    l.iter().copied().filter(|n| keep.contains(n)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3 x 3 quads in z = 0 with the middle one missing (an opening), plus a wall (x = 0).
    #[test]
    fn slab_with_hole_and_wall() {
        let mut nodes = BTreeMap::new();
        let id = |i: u32, j: u32| 1 + i + 4 * j;
        for j in 0..4 {
            for i in 0..4 {
                nodes.insert(id(i, j), [i as f64, j as f64, 0.0]);
            }
        }
        let mut plates = Vec::new();
        for j in 0..3 {
            for i in 0..3 {
                if (i, j) != (1, 1) {
                    plates.push(Plate {
                        id: 100 + i + 3 * j,
                        nodes: vec![id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)],
                        key: 1,
                    });
                }
            }
        }
        // Wall below the edge x = 0: nodes 50.. at z = -1.
        for j in 0..4 {
            nodes.insert(50 + j, [0.0, j as f64, -1.0]);
        }
        for j in 0..3 {
            plates.push(Plate {
                id: 200 + j,
                nodes: vec![id(0, j), id(0, j + 1), 51 + j, 50 + j],
                key: 1,
            });
        }
        let regions = rebuild(&nodes, &plates, 1e-3);
        assert_eq!(regions.len(), 2);
        let slab = regions.iter().find(|r| r.plates.len() == 8).unwrap();
        assert_eq!(slab.outer.len(), 12);
        assert_eq!(slab.holes.len(), 1);
        assert_eq!(slab.holes[0].len(), 4);
        let c = corners(&slab.outer, &nodes, 1e-6);
        assert_eq!(c.len(), 4);
        let wall = regions.iter().find(|r| r.plates.len() == 3).unwrap();
        assert!(wall.holes.is_empty());
        assert_eq!(corners(&wall.outer, &nodes, 1e-6).len(), 4);
    }
}
