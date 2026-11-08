//! Selection geometry, as Chunky's shapes and iterators (`shape/`, `iterator/` in Chunky's common module) decide it:
//! a chunk is selected when its centre (`x * 16 + 8`, `z * 16 + 8`) lies in the shape, and the shape is aligned to the
//! chunk grid (its half-sizes are `(2 * chunk radius + 1) * 8` blocks, centred on the centre chunk's middle).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Square,
    Circle,
    /// Two radii: `radius` along x, `radius_z` along z.
    Rectangle,
    /// Two radii, as `Rectangle`.
    Ellipse,
    /// Half-diagonal `radius` (the x radius) along both axes.
    Diamond,
    Triangle,
    Pentagon,
    Hexagon,
    /// Pentagram: outer corners at `radius`, one pointing north.
    Star,
}

/// The shape names Chunky accepts, in its order of listing (`ShapeType.all()`).
pub const SHAPES: &str = "square|circle|rectangle|ellipse|diamond|triangle|pentagon|hexagon|star";

/// Shape names sorted as Chunky lists them in messages.
pub const SHAPES_SORTED: &str = "circle, diamond, ellipse, hexagon, pentagon, rectangle, square, star, triangle";

impl Shape {
    pub fn parse(s: &str) -> Option<Shape> {
        Some(match s.to_ascii_lowercase().as_str() {
            "square" => Shape::Square,
            "circle" => Shape::Circle,
            "rectangle" => Shape::Rectangle,
            "ellipse" => Shape::Ellipse,
            "diamond" => Shape::Diamond,
            "triangle" => Shape::Triangle,
            "pentagon" => Shape::Pentagon,
            "hexagon" => Shape::Hexagon,
            "star" => Shape::Star,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Shape::Square => "square",
            Shape::Circle => "circle",
            Shape::Rectangle => "rectangle",
            Shape::Ellipse => "ellipse",
            Shape::Diamond => "diamond",
            Shape::Triangle => "triangle",
            Shape::Pentagon => "pentagon",
            Shape::Hexagon => "hexagon",
            Shape::Star => "star",
        }
    }

    /// Shapes that use a second radius along z.
    pub fn two_radii(self) -> bool {
        matches!(self, Shape::Rectangle | Shape::Ellipse)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// Regions of 32 x 32 chunks in concentric rings, each region along a Hilbert curve when it is whole.
    Region,
    /// Chunks in square rings around the centre chunk, one ring after another.
    Concentric,
    /// Row by row: x outer, z inner, from the most negative chunk.
    Loop,
    /// One square spiral outwards from the centre chunk.
    Spiral,
    /// The chunks that already exist in the world's region files, scanned when the task starts.
    World,
    /// The chunks listed in `<name>.csv`, one `x,z` per line, in the file's order.
    Csv(String),
    /// Chunky's default: square rings 64 chunks wide, starting with the centre square.
    Ring,
    /// Strips 64 chunks wide along x, row by row.
    Strip,
}

pub const PATTERNS: &str = "concentric|loop|spiral|csv|region|world|strip|ring";

/// Pattern names sorted as Chunky lists them in messages.
pub const PATTERNS_SORTED: &str = "concentric, csv, loop, region, ring, spiral, strip, world";

impl Pattern {
    /// `csv` needs its file name as `value`; the other names ignore it.
    pub fn parse(name: &str, value: Option<&str>) -> Option<Pattern> {
        Some(match name.to_ascii_lowercase().as_str() {
            "region" => Pattern::Region,
            "concentric" => Pattern::Concentric,
            "loop" => Pattern::Loop,
            "spiral" => Pattern::Spiral,
            "world" => Pattern::World,
            "ring" => Pattern::Ring,
            "strip" => Pattern::Strip,
            "csv" => {
                let v = value.filter(|v| !v.is_empty())?;
                if !v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                    return None;
                }
                Pattern::Csv(v.to_owned())
            }
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Pattern::Region => "region",
            Pattern::Concentric => "concentric",
            Pattern::Loop => "loop",
            Pattern::Spiral => "spiral",
            Pattern::World => "world",
            Pattern::Csv(_) => "csv",
            Pattern::Ring => "ring",
            Pattern::Strip => "strip",
        }
    }

    /// Patterns whose chunks come from a list read at the start (`World`, `Csv`).
    pub fn needs_list(&self) -> bool {
        matches!(self, Pattern::World | Pattern::Csv(_))
    }
}

/// A region of the world in block coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub shape: Shape,
    pub pattern: Pattern,
    pub cx: f64,
    pub cz: f64,
    pub radius: f64,
    /// The z radius of `Rectangle` and `Ellipse` (and of the `loop` and `strip` walks); the others use `radius`.
    pub radius_z: f64,
}

pub type Chunk = (i32, i32);

/// A fixed list of chunks for the `World` and `Csv` patterns, shared by a task and its rewinds.
pub type ChunkList = std::sync::Arc<Vec<Chunk>>;

/// Chunky's iterator for a shape and pattern (`ChunkIteratorFactory`): rectangles and ellipses walk strips unless the
/// pattern is `loop` or `world`, whatever else was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Walk {
    Ring,
    Concentric,
    Spiral,
    Region,
    Loop,
    /// Strips along x; the flag says the strips' height is the z radius (rectangles and ellipses).
    Strip(bool),
    World,
    Csv,
}

/// Chunky's `ChunkIteratorFactory`, with its `default` branch for the ring.
fn walk_for(shape: Shape, pattern: &Pattern) -> Walk {
    let two = matches!(shape, Shape::Rectangle | Shape::Ellipse);
    match pattern {
        Pattern::World => Walk::World,
        Pattern::Loop => Walk::Loop,
        _ if two => Walk::Strip(true),
        Pattern::Spiral => Walk::Spiral,
        Pattern::Csv(_) => Walk::Csv,
        Pattern::Region => Walk::Region,
        Pattern::Concentric => Walk::Concentric,
        Pattern::Strip => Walk::Strip(false),
        Pattern::Ring => Walk::Ring,
    }
}

/// Chunky's number of chunks per axis for a radius: `ceil(radius / 16)`.
fn chunk_radius(r: f64) -> i32 {
    (r / 16.0).ceil() as i32
}

/// Chunky's `(int) centre >> 4`: the chunk that holds the centre (a truncated cast, then an arithmetic shift).
fn centre_chunk(v: f64) -> i32 {
    (v as i32) >> 4
}

/// Chunky's `isPastWorldLimit`: coordinates and radii beyond 3e7 blocks are refused.
pub fn past_world_limit(v: f64) -> bool {
    v.abs() > 3e7
}

/// `a -> b` has `c` on its left (Chunky's `ShapeUtil.insideLine`, strict).
fn inside_line(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> bool {
    (b.0 - a.0) * (c.1 - a.1) > (b.1 - a.1) * (c.0 - a.0)
}

/// The corners of a regular polygon on the circle of `r` around `(ax, az)`, at the given angles in degrees.
fn corners(ax: f64, az: f64, r: f64, degrees: &[f64]) -> Vec<(f64, f64)> {
    degrees
        .iter()
        .map(|d| (ax + r * d.to_radians().cos(), az + r * d.to_radians().sin()))
        .collect()
}

/// Every edge `p[i] -> p[i+1]` has the point on its left.
fn inside_polygon(p: &[(f64, f64)], c: (f64, f64)) -> bool {
    (0..p.len()).all(|i| inside_line(p[i], p[(i + 1) % p.len()], c))
}

impl Selection {
    /// The chunk radii along x and z: `ceil(radius / 16)`, as Chunky's `radiusChunksX` and `radiusChunksZ`.
    pub fn chunk_radii(&self) -> (i32, i32) {
        (chunk_radius(self.radius), chunk_radius(self.radius_z))
    }

    /// The centre chunk, as Chunky's `centerChunkX` and `centerChunkZ`.
    pub fn centre_chunks(&self) -> (i32, i32) {
        (centre_chunk(self.cx), centre_chunk(self.cz))
    }

    /// The shape's centre and half-sizes in blocks, chunk aligned (Chunky's `AbstractShape` with `chunkAligned`).
    fn aligned(&self) -> (f64, f64, f64, f64) {
        let (rx, rz) = self.chunk_radii();
        let (ccx, ccz) = self.centre_chunks();
        let ax = f64::from(ccx * 16 + 8);
        let az = f64::from(ccz * 16 + 8);
        let hx = f64::from((2 * rx + 1) * 8);
        let hz = f64::from((2 * rz + 1) * 8);
        (ax, az, hx, hz)
    }

    /// Whether the chunk is selected: its centre lies in the shape (Chunky's `isBounding` on the chunk centre).
    pub fn contains(&self, c: Chunk) -> bool {
        let (ax, az, rx, rz) = self.aligned();
        let px = f64::from(c.0) * 16.0 + 8.0;
        let pz = f64::from(c.1) * 16.0 + 8.0;
        let (dx, dz) = (px - ax, pz - az);
        match self.shape {
            Shape::Square => dx >= -rx && dx <= rx && dz >= -rx && dz <= rx,
            Shape::Rectangle => dx >= -rx && dx <= rx && dz >= -rz && dz <= rz,
            Shape::Circle => (ax - px).hypot(az - pz) <= rx,
            Shape::Ellipse => dx * dx / (rx * rx) + dz * dz / (rz * rz) <= 1.0,
            Shape::Diamond => {
                let p = [(ax, az + rx), (ax - rx, az), (ax, az - rx), (ax + rx, az)];
                inside_polygon(&p, (px, pz))
            }
            Shape::Triangle => {
                let p = [(ax + rx, az + rx), (ax - rx, az + rx), (ax, az - rx)];
                inside_polygon(&p, (px, pz))
            }
            Shape::Pentagon => {
                let p = corners(ax, az, rx, &[54.0, 126.0, 198.0, 270.0, 342.0]);
                inside_polygon(&p, (px, pz))
            }
            Shape::Hexagon => {
                let p = corners(ax, az, rx, &[60.0, 120.0, 180.0, 240.0, 300.0, 360.0]);
                inside_polygon(&p, (px, pz))
            }
            Shape::Star => {
                let p = corners(ax, az, rx, &[54.0, 126.0, 198.0, 270.0, 342.0]);
                let c = (px, pz);
                let i = |a: usize, b: usize| inside_line(p[a], p[b], c);
                let (i13, i24, i35, i41, i52) = (i(0, 2), i(1, 3), i(2, 4), i(3, 0), i(4, 1));
                (i13 && i24 && i35 && i41)
                    || (i24 && i35 && i41 && i52)
                    || (i35 && i41 && i52 && i13)
                    || (i41 && i52 && i13 && i24)
                    || (i52 && i13 && i24 && i35)
            }
        }
    }

    /// The number of chunks the pattern walks (selected or not: Chunky counts every chunk of the walk), in the walk's
    /// own order. `World` and `Csv` count their list (`World` only the entries inside the walk's box).
    pub fn total(&self, list: Option<&ChunkList>) -> u64 {
        let (rx, rz) = self.chunk_radii();
        let (dx, dz) = (2 * u64::from(rx as u32) + 1, 2 * u64::from(rz as u32) + 1);
        match walk_for(self.shape, &self.pattern) {
            Walk::Ring | Walk::Concentric | Walk::Spiral | Walk::Region => dx * dx,
            Walk::Loop => dx * dz,
            Walk::Strip(use_z) => dx * if use_z { dz } else { dx },
            Walk::World => {
                let inside = self.in_box(rx, rz);
                list.map_or(0, |l| l.iter().filter(|c| inside(*c)).count() as u64)
            }
            Walk::Csv => list.map_or(0, |l| l.len() as u64),
        }
    }

    /// Whether a chunk is inside the selection's box (`centre chunk +- radius chunks`, as Chunky's `isInBounds`).
    fn in_box(&self, rx: i32, rz: i32) -> impl Fn(&Chunk) -> bool + Send + 'static {
        let (ccx, ccz) = self.centre_chunks();
        move |c: &Chunk| (c.0 - ccx).abs() <= rx && (c.1 - ccz).abs() <= rz
    }

    /// Every chunk of the pattern's walk, once, in the walk's order. `World` and `Csv` walk `list` (none: nothing).
    /// The walk covers the chunks of the selection's box; the caller skips those the shape does not contain.
    pub fn sequence(&self, list: Option<ChunkList>) -> Box<dyn Iterator<Item = Chunk> + Send> {
        let (rx, rz) = self.chunk_radii();
        let (ccx, ccz) = self.centre_chunks();
        match walk_for(self.shape, &self.pattern) {
            Walk::Ring => ring_walk(ccx, ccz, rx),
            Walk::Concentric => concentric_walk(ccx, ccz, rx),
            Walk::Spiral => Box::new(spiral(ccx, ccz, rx)),
            Walk::Region => region_walk(ccx, ccz, rx),
            Walk::Loop => Box::new((ccx - rx..=ccx + rx).flat_map(move |x| (ccz - rz..=ccz + rz).map(move |z| (x, z)))),
            Walk::Strip(use_z) => strip_walk(ccx, ccz, rx, if use_z { rz } else { rx }),
            Walk::World => {
                let inside = self.in_box(rx, rz);
                let l = list.unwrap_or_default();
                Box::new((0..l.len()).map(move |i| l[i]).filter(move |c| inside(c)))
            }
            Walk::Csv => {
                let l = list.unwrap_or_default();
                Box::new((0..l.len()).map(move |i| l[i]))
            }
        }
    }
}

/// Chunky's `RingChunkIterator` with its default width of 64 chunks: the centre square first (rows from the most
/// negative x), then square bands of 64 chunks outwards, each band going around the way Chunky does.
fn ring_walk(cx: i32, cz: i32, radius: i32) -> Box<dyn Iterator<Item = Chunk> + Send> {
    const WIDTH: i32 = 64;
    const CENTRE: i32 = (WIDTH - 1) / 2;
    Box::new(
        (0i32..)
            .map(move |ring| {
                let inner = if ring == 0 { -1 } else { CENTRE + (ring - 1) * WIDTH };
                (inner, (CENTRE + ring * WIDTH).min(radius))
            })
            .take_while(move |&(inner, _)| inner < radius)
            .flat_map(move |(inner, outer)| {
                let side = 2 * i64::from(outer) + 1;
                let total = side * side - (if inner < 0 { 0 } else { (2 * i64::from(inner) + 1).pow(2) });
                (0..total).map(move |cur| {
                    let (dx, dz) = ring_offset(inner, outer, cur, total);
                    (cx + dx, cz + dz)
                })
            }),
    )
}

/// Offset of the `cur`-th chunk of one ring band (`inner` is -1 for the centre square), as `RingChunkProgress.next`.
fn ring_offset(inner: i32, outer: i32, cur: i64, total: i64) -> (i32, i32) {
    let side = 2 * i64::from(outer) + 1;
    if inner < 0 {
        return ((cur % side) as i32 - outer, (cur / side) as i32 - outer);
    }
    let (inner, outer) = (i64::from(inner), i64::from(outer));
    let length = outer - inner;
    let first_side = side * length;
    let mirrored = cur >= total / 2;
    let sign = if mirrored { -1 } else { 1 };
    let step = if mirrored { cur - total / 2 } else { cur };
    let (dx, dz) = if step < first_side {
        (sign * (step / length - outer), sign * (-inner - 1 - step % length))
    } else {
        let turn = step - first_side;
        (sign * (inner + 1 + turn % length), sign * (turn / length - inner))
    };
    (dx as i32, dz as i32)
}

/// Chunky's `ConcentricChunkIterator`: the centre, then each square ring from its start (one row east of the corner
/// above it) down the east side, west along the south side, up the west side and east along the north side.
fn concentric_walk(cx: i32, cz: i32, r: i32) -> Box<dyn Iterator<Item = Chunk> + Send> {
    Box::new(std::iter::once((cx, cz)).chain((1..=r).flat_map(move |k| {
        let down = (cz - k..=cz + k - 1).rev().map(move |z| (cx + k, z));
        let left = (cx - k..=cx + k - 1).rev().map(move |x| (x, cz - k));
        let up = (cz - k + 1..=cz + k).map(move |z| (cx - k, z));
        let right = (cx - k + 1..=cx + k).map(move |x| (x, cz + k));
        down.chain(left).chain(up).chain(right)
    })))
}

/// Chunky's `RegionChunkIterator`: the regions of the centre region in concentric rings, a whole region along a
/// Hilbert curve, a part region row by row (x outer), each inside the selection's chunk box.
fn region_walk(cx: i32, cz: i32, rx: i32) -> Box<dyn Iterator<Item = Chunk> + Send> {
    let (rgx, rgz) = (cx >> 5, cz >> 5);
    let region_radius = (f64::from(rx) / 32.0).ceil() as i32;
    let (lo_x, hi_x, lo_z, hi_z) = (cx - rx, cx + rx, cz - rx, cz + rx);
    Box::new(concentric_walk(rgx, rgz, region_radius).flat_map(move |(gx, gz)| {
        let (x0, x1) = ((gx * 32).max(lo_x), (gx * 32 + 31).min(hi_x));
        let (z0, z1) = ((gz * 32).max(lo_z), (gz * 32 + 31).min(hi_z));
        let cells: Box<dyn Iterator<Item = Chunk> + Send> = if x0 > x1 || z0 > z1 {
            Box::new(std::iter::empty())
        } else if (x1 - x0 + 1) * (z1 - z0 + 1) == 1024 {
            Box::new((0..1024u32).map(move |d| {
                let (hx, hz) = hilbert(d);
                (x0 + hx as i32, z0 + hz as i32)
            }))
        } else {
            Box::new((x0..=x1).flat_map(move |x| (z0..=z1).map(move |z| (x, z))))
        };
        cells
    }))
}

/// Chunky's `StripChunkIterator`: strips 64 chunks wide along x; inside a strip, row by row.
fn strip_walk(cx: i32, cz: i32, rx: i32, rz: i32) -> Box<dyn Iterator<Item = Chunk> + Send> {
    const WIDTH: i64 = 64;
    let (size_x, size_z) = (2 * i64::from(rx) + 1, 2 * i64::from(rz) + 1);
    let (min_x, min_z) = (i64::from(cx) - i64::from(rx), i64::from(cz) - i64::from(rz));
    let strip_size = WIDTH * size_z;
    Box::new((0..size_x * size_z).map(move |cur| {
        let strip_x = cur / strip_size * WIDTH;
        let offset = cur % strip_size;
        let width = WIDTH.min(size_x - strip_x);
        ((min_x + strip_x + offset % width) as i32, (min_z + offset / width) as i32)
    }))
}

/// A square spiral from `(cx, cz)`: east 1, south (-z) 1, west 2, north 2, east 3... It covers the square of
/// Chebyshev radius `k` in exactly `(2k + 1)^2` steps, as Chunky's `SpiralChunkIterator` does.
pub fn spiral(cx: i32, cz: i32, k: i32) -> impl Iterator<Item = Chunk> + Send {
    let count = (2 * i64::from(k) + 1).pow(2);
    let (mut x, mut z) = (cx, cz);
    let (mut dir, mut leg, mut walked, mut legs_done) = (0usize, 1i64, 0i64, 0u32);
    let mut n = 0i64;
    std::iter::from_fn(move || {
        if n >= count {
            return None;
        }
        const STEP: [(i32, i32); 4] = [(1, 0), (0, -1), (-1, 0), (0, 1)];
        let here = (x, z);
        x += STEP[dir].0;
        z += STEP[dir].1;
        walked += 1;
        if walked == leg {
            walked = 0;
            dir = (dir + 1) % 4;
            legs_done += 1;
            if legs_done % 2 == 0 {
                leg += 1;
            }
        }
        n += 1;
        Some(here)
    })
}

/// Position `d` (0..1024) on a 32x32 Hilbert curve, as Chunky's `Hilbert.regionDistanceToChunkCoordinateOffset`.
fn hilbert(d: u32) -> (u32, u32) {
    let (mut x, mut y, mut t, mut s) = (0u32, 0u32, d, 1u32);
    while s < 32 {
        let rx = 1 & (t / 2);
        let ry = 1 & (t ^ rx);
        if ry == 0 {
            if rx == 1 {
                x = s - 1 - x;
                y = s - 1 - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        x += s * rx;
        y += s * ry;
        t /= 4;
        s *= 2;
    }
    (x, y)
}

/// A decimal with at most two places and no trailing zeros, as Chunky's `Formatting.number` (`#.##`).
pub fn number(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// Chunky's `Input.tryDoubleSuffixed`: a number with an optional `c` (16), `r` (512) or `k` (1000) suffix.
pub fn suffixed(s: &str) -> Option<f64> {
    let last = s.chars().last()?;
    let (body, mul) = match last.to_ascii_lowercase() {
        'c' => (&s[..s.len() - 1], 16.0),
        'r' => (&s[..s.len() - 1], 512.0),
        'k' => (&s[..s.len() - 1], 1000.0),
        _ => (s, 1.0),
    };
    let v: f64 = body.parse().ok()?;
    v.is_finite().then_some(v * mul)
}

/// `radius`: `500`, `10k`, `625c`, `2r`, or `+1k` / `-1k` relative to `current`, as Chunky's `RadiusCommand`. A result
/// below zero, or past the world limit, is an error.
pub fn parse_radius(s: &str, current: f64) -> Result<f64, String> {
    let bad = || format!("not a radius: {s}");
    let (sign, body) = match s.as_bytes().first() {
        Some(b'+') => (1.0, &s[1..]),
        Some(b'-') => (-1.0, &s[1..]),
        _ => (0.0, s),
    };
    let v = suffixed(body).ok_or_else(bad)?;
    if v < 0.0 || past_world_limit(v) {
        return Err(bad());
    }
    let r = if sign == 0.0 { v } else { current + sign * v };
    if r < 0.0 || past_world_limit(r) {
        return Err(bad());
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn sel(shape: Shape, pattern: Pattern, r: f64) -> Selection {
        Selection { shape, pattern, cx: 0.0, cz: 0.0, radius: r, radius_z: r }
    }

    const ALL_SHAPES: [Shape; 9] = [
        Shape::Square,
        Shape::Circle,
        Shape::Rectangle,
        Shape::Ellipse,
        Shape::Diamond,
        Shape::Triangle,
        Shape::Pentagon,
        Shape::Hexagon,
        Shape::Star,
    ];

    /// Chunks of the selection's box (by its chunk radii) that the shape contains.
    fn contained(s: &Selection) -> Vec<Chunk> {
        let (rx, rz) = s.chunk_radii();
        let (ccx, ccz) = s.centre_chunks();
        let mut v = Vec::new();
        for x in ccx - rx..=ccx + rx {
            for z in ccz - rz..=ccz + rz {
                if s.contains((x, z)) {
                    v.push((x, z));
                }
            }
        }
        v
    }

    #[test]
    fn square_radius_500_is_65_by_65_chunks() {
        let s = sel(Shape::Square, Pattern::Ring, 500.0);
        assert_eq!(s.chunk_radii(), (32, 32));
        assert_eq!(s.total(None), 65 * 65);
        assert!(s.contains((32, 0)) && !s.contains((33, 0)));
        assert_eq!(contained(&s).len(), 65 * 65);
    }

    #[test]
    fn walks_visit_every_chunk_of_the_box_exactly_once() {
        for pattern in [Pattern::Ring, Pattern::Concentric, Pattern::Loop, Pattern::Spiral, Pattern::Region, Pattern::Strip] {
            for shape in ALL_SHAPES {
                let mut s = sel(shape, pattern.clone(), 700.0);
                s.cx = 37.0;
                s.cz = -300.5;
                if shape.two_radii() {
                    s.radius_z = 450.0;
                }
                let v: Vec<_> = s.sequence(None).collect();
                let set: HashSet<_> = v.iter().copied().collect();
                assert_eq!(v.len(), set.len(), "{pattern:?} {shape:?} repeats a chunk");
                assert_eq!(v.len() as u64, s.total(None), "{pattern:?} {shape:?} total");
                for c in contained(&s) {
                    assert!(set.contains(&c), "{pattern:?} {shape:?} misses {c:?}");
                }
            }
        }
    }

    #[test]
    fn circle_is_smaller_than_square_and_round() {
        let sq = contained(&sel(Shape::Square, Pattern::Ring, 800.0)).len() as f64;
        let ci = contained(&sel(Shape::Circle, Pattern::Ring, 800.0)).len() as f64;
        let ratio = ci / sq;
        assert!((0.75..0.85).contains(&ratio), "ratio {ratio}");
        let s = sel(Shape::Circle, Pattern::Ring, 800.0);
        assert!(!s.contains((49, 49)) && s.contains((0, 0)) && s.contains((49, 0)));
    }

    #[test]
    fn concentric_starts_in_the_centre_and_moves_outwards() {
        let s = sel(Shape::Square, Pattern::Concentric, 300.0);
        let v: Vec<_> = s.sequence(None).collect();
        assert_eq!(v[0], (0, 0));
        assert_eq!(v[1..9], [(1, 0), (1, -1), (0, -1), (-1, -1), (-1, 0), (-1, 1), (0, 1), (1, 1)]);
        let dist = |c: &Chunk| c.0.abs().max(c.1.abs());
        assert!(v.windows(2).all(|w| dist(&w[0]) <= dist(&w[1])));
    }

    #[test]
    fn ring_starts_with_the_centre_square_then_bands() {
        let s = sel(Shape::Square, Pattern::Ring, 40.0 * 16.0);
        let v: Vec<_> = s.sequence(None).collect();
        assert_eq!(v.len(), 81 * 81);
        assert_eq!(v[0], (-31, -31));
        assert_eq!(v[63], (-31, -30));
        // The centre square (radius 31) comes first, then the band of 64 outwards.
        let first_band = v.iter().position(|c| c.0.abs().max(c.1.abs()) > 31).unwrap();
        assert_eq!(first_band, 63 * 63);
    }

    #[test]
    fn region_pattern_finishes_a_region_before_the_next() {
        let s = sel(Shape::Square, Pattern::Region, 1200.0);
        let v: Vec<_> = s.sequence(None).collect();
        let region = |c: &Chunk| (c.0 >> 5, c.1 >> 5);
        let mut seen = HashSet::new();
        let mut last = region(&v[0]);
        seen.insert(last);
        for c in &v {
            let r = region(c);
            if r != last {
                assert!(seen.insert(r), "region {r:?} re-entered");
                last = r;
            }
        }
    }

    #[test]
    fn hilbert_curve_steps_to_a_neighbour() {
        let mut prev = hilbert(0);
        for d in 1..1024 {
            let p = hilbert(d);
            let step = (p.0 as i32 - prev.0 as i32).abs() + (p.1 as i32 - prev.1 as i32).abs();
            assert_eq!(step, 1, "d={d}");
            prev = p;
        }
    }

    #[test]
    fn shapes_have_the_expected_areas() {
        // Contained chunks (256 blocks each) against the shape's own area; the chunk-aligned radius is within 1.5 %.
        let r = 1500.0;
        let area = |shape| contained(&sel(shape, Pattern::Loop, r)).len() as f64 * 256.0;
        let near = |got: f64, want: f64| (got / want - 1.0).abs() < 0.06;
        let pent = 2.5 * r * r * 72f64.to_radians().sin();
        let hex = 1.5 * 3f64.sqrt() * r * r;
        assert!(near(area(Shape::Diamond), 2.0 * r * r), "diamond");
        // Corners (r, r), (-r, r), (0, -r): a right-angled base of 2r and height 2r.
        assert!(near(area(Shape::Triangle), 2.0 * r * r), "triangle");
        assert!(near(area(Shape::Pentagon), pent), "pentagon");
        assert!(near(area(Shape::Hexagon), hex), "hexagon");
        let star = area(Shape::Star);
        assert!(star < pent && star > 0.2 * pent, "star {star} vs pentagon {pent}");
        let mut e = sel(Shape::Ellipse, Pattern::Loop, r);
        e.radius_z = r / 2.0;
        let want = std::f64::consts::PI * r * r / 2.0;
        assert!(near(contained(&e).len() as f64 * 256.0, want), "ellipse");
        let mut rect = sel(Shape::Rectangle, Pattern::Loop, 500.0);
        rect.radius_z = 1000.0;
        assert_eq!(rect.chunk_radii(), (32, 63));
        assert_eq!(rect.total(None), 65 * 127);
        // Only the two-radius shapes read radius_z.
        let mut sq = sel(Shape::Square, Pattern::Ring, 500.0);
        sq.radius_z = 1000.0;
        assert_eq!(sq.total(None), 65 * 65);
    }

    #[test]
    fn triangle_points_north() {
        let s = sel(Shape::Triangle, Pattern::Ring, 1000.0);
        // The tip at z = -1016 (the aligned radius) is selected, the chunk beyond it is not; so is the base's edge.
        assert!(s.contains((0, -63)) && !s.contains((0, -64)));
        assert!(s.contains((0, 63)) && !s.contains((0, 64)));
    }

    #[test]
    fn rectangles_and_ellipses_walk_strips_unless_looping() {
        let mut r = sel(Shape::Rectangle, Pattern::Ring, 32.0);
        r.radius_z = 32.0;
        let v: Vec<_> = r.sequence(None).take(3).collect();
        assert_eq!(v, vec![(-2, -2), (-1, -2), (0, -2)]);
        assert_eq!(walk_for(Shape::Ellipse, &Pattern::Csv("x".into())), Walk::Strip(true));
        assert_eq!(walk_for(Shape::Ellipse, &Pattern::Loop), Walk::Loop);
    }

    #[test]
    fn loop_walks_rows_from_the_negative_corner() {
        let s = sel(Shape::Square, Pattern::Loop, 32.0);
        let v: Vec<_> = s.sequence(None).collect();
        assert_eq!(v.len(), 25);
        assert_eq!(v[..5], [(-2, -2), (-2, -1), (-2, 0), (-2, 1), (-2, 2)]);
        assert_eq!(v[24], (2, 2));
    }

    #[test]
    fn spiral_steps_to_a_neighbour_and_starts_in_the_centre() {
        let v: Vec<_> = spiral(5, -7, 6).collect();
        assert_eq!(v.len(), 13 * 13);
        assert_eq!(v[..9], [(5, -7), (6, -7), (6, -8), (5, -8), (4, -8), (4, -7), (4, -6), (5, -6), (6, -6)]);
        for w in v.windows(2) {
            assert_eq!((w[0].0 - w[1].0).abs() + (w[0].1 - w[1].1).abs(), 1, "{w:?}");
        }
        let set: HashSet<_> = v.iter().collect();
        assert_eq!(set.len(), v.len());
        assert_eq!(*v.last().unwrap(), (11, -1));
    }

    #[test]
    fn list_patterns_walk_the_list_and_keep_the_listed_chunks_in_the_box() {
        let list: ChunkList = std::sync::Arc::new(vec![(3, 3), (0, 0), (100, 100), (3, 3), (-1, 0)]);
        let c = sel(Shape::Square, Pattern::Csv("mine".into()), 160.0);
        assert_eq!(c.sequence(Some(list.clone())).collect::<Vec<_>>(), *list);
        assert_eq!(c.total(Some(&list)), 5);
        let w = sel(Shape::Square, Pattern::World, 160.0);
        assert_eq!(w.sequence(Some(list.clone())).collect::<Vec<_>>(), vec![(3, 3), (0, 0), (3, 3), (-1, 0)]);
        assert_eq!(w.total(Some(&list)), 4);
        assert!(sel(Shape::Square, Pattern::Csv("x".into()), 1.0).sequence(None).next().is_none());
    }

    #[test]
    fn pattern_names_round_trip() {
        for p in ["concentric", "loop", "spiral", "region", "world", "ring", "strip"] {
            assert_eq!(Pattern::parse(p, None).unwrap().name(), p);
        }
        assert_eq!(Pattern::parse("csv", Some("my-list")).unwrap().name(), "csv");
        assert!(Pattern::parse("csv", None).is_none() && Pattern::parse("csv", Some("../x")).is_none());
        assert!(Pattern::parse("zigzag", None).is_none());
        for s in ALL_SHAPES {
            assert_eq!(Shape::parse(s.name()), Some(s));
        }
        assert!(Shape::parse("oval").is_none());
    }

    #[test]
    fn numbers_print_like_chunky() {
        assert_eq!(number(500.0), "500");
        assert_eq!(number(1.5), "1.5");
        assert_eq!(number(-0.001), "0");
        assert_eq!(number(10.256), "10.26");
    }

    #[test]
    fn radius_parsing() {
        assert_eq!(parse_radius("500", 0.0), Ok(500.0));
        assert_eq!(parse_radius("0", 0.0), Ok(0.0));
        assert_eq!(parse_radius("10k", 0.0), Ok(10000.0));
        assert_eq!(parse_radius("625c", 0.0), Ok(10000.0));
        assert_eq!(parse_radius("2r", 0.0), Ok(1024.0));
        assert_eq!(parse_radius("+1k", 500.0), Ok(1500.0));
        assert_eq!(parse_radius("-200", 500.0), Ok(300.0));
        assert!(parse_radius("-600", 500.0).is_err());
        assert!(parse_radius("abc", 1.0).is_err());
    }
}
