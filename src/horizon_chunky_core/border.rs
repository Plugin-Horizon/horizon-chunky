//! ChunkyBorder (pop4959/ChunkyBorder, GPL-3.0): custom world borders next to Chunky. The logic is the add-on's:
//! `BorderData` and its shapes (Chunky's shape classes, not chunk aligned), `borders.json`, the wrap modes and the
//! redirect maths of `BorderCheckTask` and `ChunkyBorder`, the `chunkyborder.bypass.*` rules and the config keys.
//! Nothing here touches the game: the glue asks `check`, `teleport_redirect` and `blocked` and does the teleports.

use super::geom::{line_intersection, Pattern, Selection, Shape};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// `BorderWrapType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrap {
    None,
    Default,
    Both,
    Radial,
    X,
    Z,
    Earth,
}

impl Wrap {
    pub const ALL: [Wrap; 7] = [Wrap::None, Wrap::Default, Wrap::Both, Wrap::Radial, Wrap::X, Wrap::Z, Wrap::Earth];

    /// `BorderWrapType.fromString`: `true` is the default wrap, `false` and anything unknown are none.
    pub fn parse(s: &str) -> Wrap {
        let lower = s.to_ascii_lowercase();
        match lower.as_str() {
            "true" => Wrap::Default,
            "false" => Wrap::None,
            other => Wrap::ALL.into_iter().find(|w| w.name() == other).unwrap_or(Wrap::None),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Wrap::None => "none",
            Wrap::Default => "default",
            Wrap::Both => "both",
            Wrap::Radial => "radial",
            Wrap::X => "x",
            Wrap::Z => "z",
            Wrap::Earth => "earth",
        }
    }
}

/// `RedirectType` of the API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedirectKind {
    Bounce,
    Wrap,
    Teleport,
    Respawn,
}

impl RedirectKind {
    pub fn name(self) -> &'static str {
        match self {
            RedirectKind::Bounce => "BOUNCE",
            RedirectKind::Wrap => "WRAP",
            RedirectKind::Teleport => "TELEPORT",
            RedirectKind::Respawn => "RESPAWN",
        }
    }
}

/// One border: `BorderData`.
#[derive(Clone, Debug, PartialEq)]
pub struct BorderData {
    pub world: String,
    pub shape: Shape,
    pub cx: f64,
    pub cz: f64,
    pub rx: f64,
    pub rz: f64,
    pub wrap: Wrap,
}

/// Where a player goes instead, as the glue teleports them.
#[derive(Clone, Debug, PartialEq)]
pub struct Redirect {
    pub to: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub kind: RedirectKind,
    /// A wrap lands on a new column: the glue sets `to[1]` to the surface there (`BorderCheckTask.wrap`).
    pub needs_ground: bool,
}

/// What the glue tells other mods: `BorderChangeEvent` and `BorderRedirectEvent`.
#[derive(Clone, Debug, PartialEq)]
pub enum BorderEvent {
    Change { world: String, border: Option<BorderData> },
    Redirect { player: u128, world: String, from: [f64; 3], to: [f64; 3], kind: RedirectKind },
}

/// `chunkyborder.bypass.move`, `.place` and `.break` (plugin.yml; default nobody).
pub const BYPASS_MOVE: &str = "chunkyborder.bypass.move";
pub const BYPASS_PLACE: &str = "chunkyborder.bypass.place";
pub const BYPASS_BREAK: &str = "chunkyborder.bypass.break";

impl BorderData {
    pub fn selection(&self) -> Selection {
        Selection { shape: self.shape, pattern: Pattern::Ring, cx: self.cx, cz: self.cz, radius: self.rx, radius_z: self.rz }
    }

    /// `Shape.isBounding` of the border's shape.
    pub fn is_bounding(&self, x: f64, z: f64) -> bool {
        self.selection().contains_point(x, z)
    }

    fn rectangle(&self) -> bool {
        matches!(self.shape, Shape::Square | Shape::Rectangle)
    }

    /// The points where the line through `centre` and `p` meets the border (`ShapeUtil.intersection` against each
    /// edge, or `pointOnEllipse`); `angle` is the ellipse's angle for the caller's direction.
    fn crossings(&self, p: (f64, f64), ellipse_angle: f64) -> Vec<(f64, f64)> {
        let sel = self.selection();
        let centre = (self.cx, self.cz);
        if let Some(points) = sel.polygon_points() {
            let n = points.len();
            (0..n).filter_map(|i| line_intersection(centre, p, points[i], points[(i + 1) % n])).collect()
        } else if let Some((rx, rz)) = sel.ellipse_radii() {
            vec![(self.cx + rx * ellipse_angle.cos(), self.cz + rz * ellipse_angle.sin())]
        } else {
            Vec::new()
        }
    }

    /// `BorderCheckTask.wrap`: the wrapped position (x, z and a direction to face when the mode sets one), None when
    /// the mode does not wrap here.
    fn wrapped(&self, mode: Wrap, x: f64, z: f64) -> Option<Wrapped> {
        let mut at = Wrapped { x, z, yaw: None, dir: None };
        let done = match mode {
            Wrap::None => false,
            Wrap::Default => {
                if self.rectangle() {
                    self.wrap_both(&mut at)
                } else {
                    self.wrap_radial(&mut at)
                }
            }
            Wrap::Both => self.wrap_both(&mut at),
            Wrap::Radial => self.wrap_radial(&mut at),
            Wrap::X => self.wrap_x(&mut at),
            Wrap::Z => self.wrap_z(&mut at),
            Wrap::Earth => self.rectangle() && self.wrap_earth(&mut at),
        };
        done.then_some(at)
    }

    fn wrap_both(&self, at: &mut Wrapped) -> bool {
        self.wrap_x(at);
        self.wrap_z(at);
        true
    }

    fn wrap_x(&self, at: &mut Wrapped) -> bool {
        let (min_x, max_x) = (self.cx - self.rx, self.cx + self.rx);
        if at.x <= min_x {
            at.x = max_x - 3.0;
        } else if at.x >= max_x {
            at.x = min_x + 3.0;
        }
        let (min_z, max_z) = (self.cz - self.rz, self.cz + self.rz);
        at.z > min_z && at.z < max_z
    }

    fn wrap_z(&self, at: &mut Wrapped) -> bool {
        let (min_z, max_z) = (self.cz - self.rz, self.cz + self.rz);
        if at.z <= min_z {
            at.z = max_z - 3.0;
        } else if at.z >= max_z {
            at.z = min_z + 3.0;
        }
        let (min_x, max_x) = (self.cx - self.rx, self.cx + self.rx);
        at.x > min_x && at.x < max_x
    }

    fn wrap_earth(&self, at: &mut Wrapped) -> bool {
        self.wrap_x(at);
        let (min_z, max_z) = (self.cz - self.rz, self.cz + self.rz);
        let meridian = self.cx - at.x;
        if at.z <= min_z {
            at.x = self.cx + meridian;
            at.z = min_z + 3.0;
            at.yaw = Some(0.0);
        } else if at.z >= max_z {
            at.x = self.cx + meridian;
            at.z = max_z - 3.0;
            at.yaw = Some(180.0);
        }
        true
    }

    /// `BorderCheckTask.wrapRadial`: across the middle to the far edge, three blocks inside it.
    fn wrap_radial(&self, at: &mut Wrapped) -> bool {
        let from = (at.x, at.z);
        // Chunky's ellipse angle reads x against the centre's z and the other way round; kept as it is.
        let angle = std::f64::consts::PI + (from.1 - self.cx).atan2(from.0 - self.cz);
        let crossings = self.crossings(from, angle);
        if crossings.is_empty() {
            return false;
        }
        let dir = unit3((from.0 - self.cx, from.1 - self.cz));
        let mut closest = crossings[0];
        let mut longest = 0.0_f64;
        let mut found = false;
        for c in crossings {
            let d = (from.0 - c.0).powi(2) + (from.1 - c.1).powi(2);
            if d > longest && self.is_bounding(c.0 + dir.0, c.1 + dir.1) {
                closest = c;
                longest = d;
                found = true;
            }
        }
        if !found {
            return false;
        }
        at.x = closest.0 + dir.0;
        at.z = closest.1 + dir.1;
        at.dir = Some(dir);
        true
    }

    /// `ChunkyBorder`'s teleport handler: where a destination outside the border moves to, the nearest edge point
    /// seen from the destination, three blocks towards the centre. None when the destination is inside; the
    /// inner None is the world spawn (no edge point found).
    fn pull_in(&self, to: (f64, f64)) -> Option<Option<Wrapped>> {
        if self.is_bounding(to.0, to.1) {
            return None;
        }
        let angle = (to.1 - self.cx).atan2(to.0 - self.cz);
        let crossings = self.crossings(to, angle);
        if crossings.is_empty() {
            return Some(None);
        }
        let dir = unit3((self.cx - to.0, self.cz - to.1));
        let mut closest = crossings[0];
        let mut shortest = f64::MAX;
        for c in crossings {
            let d = (to.0 - c.0).powi(2) + (to.1 - c.1).powi(2);
            if d < shortest && self.is_bounding(c.0 + dir.0, c.1 + dir.1) {
                closest = c;
                shortest = d;
            }
        }
        if shortest == f64::MAX {
            return Some(None);
        }
        Some(Some(Wrapped { x: closest.0 + dir.0, z: closest.1 + dir.1, yaw: None, dir: Some(dir) }))
    }
}

impl BorderData {
    /// ChunkyBorder's visualizer (`Particles.at`): the points of the border's wall within `max` blocks of `pos`, a
    /// column of them every block along the edge, shifted by `offset` (0 to 1) of a block so the wall seems to move.
    pub fn visualizer_points(&self, pos: [f64; 3], max: f64, offset: f64) -> Vec<[f64; 3]> {
        let max2 = max * max;
        let sel = self.selection();
        let mut out = Vec::new();
        if let Some(points) = sel.polygon_points() {
            let n = points.len();
            for i in 0..n {
                let (p1, p2) = (points[i], points[(i + 1) % n]);
                let Some(closest) = closest_on_line((pos[0], pos[2]), p1, p2) else { continue };
                if (pos[0] - closest.0).hypot(pos[2] - closest.1) > max {
                    continue;
                }
                let len = (p2.0 - p1.0).hypot(p2.1 - p1.1);
                if len == 0.0 {
                    continue;
                }
                let (ux, uz) = ((p2.0 - p1.0) / len, (p2.1 - p1.1) / len);
                let (ax, az) = (ux.abs(), uz.abs());
                let from_x = (closest.0 - p1.0).abs();
                let from_z = (closest.1 - p1.1).abs();
                let steps_x = if ax == 0.0 { 0.0 } else { (from_x / ax).trunc() };
                let steps_z = if az == 0.0 { 0.0 } else { (from_z / az).trunc() };
                let start = (p1.0 + steps_x * ux, pos[1].floor(), p1.1 + steps_z * uz);
                let bounds = (p1.0.min(p2.0), p1.1.min(p2.1), p1.0.max(p2.0), p1.1.max(p2.1));
                for dir in [-1.0, 1.0] {
                    // Backwards from the start (first step -1) and forwards (first step 0), until out of range.
                    let first = if dir < 0.0 { -1.0 } else { 0.0 };
                    for k in 0..MAX_VISUALIZER_STEPS {
                        let d = first + dir * f64::from(k);
                        let at = [start.0 + d * ux, start.1, start.2 + d * uz];
                        if dist2(pos, at) > max2 {
                            break;
                        }
                        out.extend(vertical_points(pos, at, offset, (ux, uz), bounds, max2));
                    }
                }
            }
        } else if let Some((rx, rz)) = sel.ellipse_radii() {
            let r = rx.min(rz);
            let angle = ((2.0 * r * r - 1.0) / (2.0 * r * r)).acos();
            if !angle.is_finite() || angle <= 0.0 {
                return out;
            }
            let camera = (rx * (pos[2] - self.cz)).atan2(rz * (pos[0] - self.cx));
            let start_y = pos[1].floor();
            let forward_start = (camera / angle).floor() * angle;
            let on = |a: f64| (self.cx + rx * a.cos(), self.cz + rz * a.sin());
            let segment = |a: f64| {
                let (s, e) = (on(a), on(a + angle));
                let bounds = (s.0.min(e.0), s.1.min(e.1), s.0.max(e.0), s.1.max(e.1));
                vertical_points(pos, [s.0, start_y, s.1], offset, (e.0 - s.0, e.1 - s.1), bounds, max2)
            };
            let backward_start = forward_start - angle;
            let mut da = backward_start;
            for _ in 0..MAX_VISUALIZER_STEPS {
                if da <= backward_start - std::f64::consts::PI {
                    break;
                }
                let pts = segment(da);
                if pts.is_empty() {
                    break;
                }
                out.extend(pts);
                da -= angle;
            }
            let mut da = forward_start;
            for _ in 0..MAX_VISUALIZER_STEPS {
                if da >= forward_start + std::f64::consts::PI {
                    break;
                }
                let pts = segment(da);
                if pts.is_empty() {
                    break;
                }
                out.extend(pts);
                da += angle;
            }
        }
        out
    }
}

/// A guard for the visualizer's walks, which Chunky ends by distance alone.
const MAX_VISUALIZER_STEPS: u32 = 4096;

fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// `ShapeUtil.closestPointOnLine`: where the perpendicular from `pos` meets the line through `p1` and `p2`.
fn closest_on_line(pos: (f64, f64), p1: (f64, f64), p2: (f64, f64)) -> Option<(f64, f64)> {
    let (dx, dz) = (p2.0 - p1.0, p2.1 - p1.1);
    let p3 = if dz == 0.0 { (pos.0, pos.1 + 1.0) } else { (pos.0 + 1.0, pos.1 + (-dx / dz)) };
    line_intersection(p1, p2, pos, p3)
}

/// `Particles.verticalPoints`: the column of points at one spot of the wall, up and down from the player's height.
fn vertical_points(pos: [f64; 3], start: [f64; 3], offset: f64, unit: (f64, f64), bounds: (f64, f64, f64, f64), max2: f64) -> Vec<[f64; 3]> {
    let mut out = Vec::new();
    let (ox, oz) = (start[0] + unit.0 * offset, start[2] + unit.1 * offset);
    let first = [ox, start[1] - offset, oz];
    if !(ox >= bounds.0 && ox <= bounds.2 && oz >= bounds.1 && oz <= bounds.3) || dist2(pos, first) > max2 {
        return out;
    }
    out.push(first);
    for dy in 0..MAX_VISUALIZER_STEPS {
        let dy = f64::from(dy);
        let up = [ox, start[1] + dy - offset, oz];
        let down = [ox, start[1] - dy - offset, oz];
        let (near_up, near_down) = (dist2(pos, up) <= max2, dist2(pos, down) <= max2);
        if !near_up && !near_down {
            break;
        }
        if near_up {
            out.push(up);
        }
        if near_down {
            out.push(down);
        }
    }
    out
}

/// A position a wrap or pull-in gives, and the way to face there.
#[derive(Clone, Debug, PartialEq)]
struct Wrapped {
    x: f64,
    z: f64,
    yaw: Option<f32>,
    dir: Option<(f64, f64)>,
}

/// `new Vector3(dx, 0, dz).normalize().multiply(3)`; the zero vector stays zero (Bukkit would give NaN, the
/// player is then at the centre, which no border excludes).
fn unit3(v: (f64, f64)) -> (f64, f64) {
    let len = v.0.hypot(v.1);
    if len == 0.0 { (0.0, 0.0) } else { (v.0 / len * 3.0, v.1 / len * 3.0) }
}

/// `Location.setDirection` with a flat vector: the yaw that faces `(dx, dz)`.
pub fn yaw_of(dir: (f64, f64)) -> f32 {
    let theta = (-dir.0).atan2(dir.1);
    let two_pi = std::f64::consts::TAU;
    (((theta + two_pi) % two_pi).to_degrees()) as f32
}

/// The player data `ChunkyBorder` keeps: the last position inside, the bypass switch.
#[derive(Clone, Debug)]
struct Last {
    world: String,
    pos: [f64; 3],
}

/// ChunkyBorder's `config.yml`: the keys under `border-options`.
#[derive(Clone, Debug, PartialEq)]
pub struct BorderConfig {
    pub check_interval: u64,
    pub message: String,
    pub use_action_bar: bool,
    pub effect: String,
    pub sound: String,
    pub prevent_mob_spawns: bool,
    pub prevent_enderpearl: bool,
    pub prevent_chorus_fruit: bool,
    pub prevent_end_gateway: bool,
    pub prevent_trident: bool,
    pub visualizer_enabled: bool,
    pub visualizer_range: i64,
    pub visualizer_color: String,
}

impl Default for BorderConfig {
    fn default() -> Self {
        BorderConfig {
            check_interval: 20,
            message: "&cYou have reached the edge of this world.".into(),
            use_action_bar: true,
            effect: "ender_signal".into(),
            sound: "entity_enderman_teleport".into(),
            prevent_mob_spawns: false,
            prevent_enderpearl: false,
            prevent_chorus_fruit: false,
            prevent_end_gateway: false,
            prevent_trident: false,
            visualizer_enabled: false,
            visualizer_range: 8,
            visualizer_color: "20A0FF".into(),
        }
    }
}

impl BorderConfig {
    /// ChunkyBorder's `config.yml` (`key: value` under `border-options:`) or the same keys as `key = value`; unknown
    /// keys (`version`, `map-options`) are ignored.
    pub fn parse(text: &str) -> BorderConfig {
        let mut c = BorderConfig::default();
        for line in text.lines() {
            let line = line.trim_start();
            if line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=').or_else(|| line.split_once(':')) else { continue };
            let k = k.trim();
            let mut v = v.trim();
            if !v.starts_with('"') && !v.starts_with('\'') {
                v = v.split('#').next().unwrap_or("").trim();
            }
            let v = v.trim_matches('"').trim_matches('\'');
            let on = v.eq_ignore_ascii_case("true");
            match k {
                "check-interval" => c.check_interval = v.parse().unwrap_or(c.check_interval),
                "message" => c.message = v.to_owned(),
                "use-action-bar" => c.use_action_bar = on,
                "effect" => c.effect = v.to_owned(),
                "sound" => c.sound = v.to_owned(),
                "prevent-mob-spawns" => c.prevent_mob_spawns = on,
                "prevent-enderpearl" => c.prevent_enderpearl = on,
                "prevent-chorus-fruit" => c.prevent_chorus_fruit = on,
                "prevent-end-gateway" => c.prevent_end_gateway = on,
                "prevent-trident" => c.prevent_trident = on,
                "visualizer-enabled" => c.visualizer_enabled = on,
                "visualizer-range" => c.visualizer_range = v.parse().unwrap_or(c.visualizer_range),
                "visualizer-color" => c.visualizer_color = v.to_owned(),
                _ => {}
            }
        }
        c
    }

    /// The sound's resource name: Bukkit's enum name `ENTITY_ENDERMAN_TELEPORT` is `entity.enderman.teleport`; a
    /// name that already has a `.` or a namespace is used as given.
    pub fn sound_key(&self) -> Option<String> {
        let s = self.sound.trim();
        if s.is_empty() {
            return None;
        }
        let lower = s.to_ascii_lowercase();
        Some(if lower.contains('.') || lower.contains(':') { lower } else { lower.replace('_', ".") })
    }
}

/// The borders, the players' state and the events waiting for the glue.
#[derive(Default)]
pub struct Borders {
    map: BTreeMap<String, BorderData>,
    dir: Option<PathBuf>,
    pub config: BorderConfig,
    bypass: HashSet<u128>,
    last: HashMap<u128, Last>,
    /// Online players by lower-case name, refreshed by the glue (`bypass` takes a name).
    online: HashMap<String, (String, u128)>,
    events: Vec<BorderEvent>,
}

impl Borders {
    /// Reads `config.toml` / `config.yml` and `borders.json` of the add-on's folder.
    pub fn load(dir: &Path) -> Borders {
        let mut b = Borders { dir: Some(dir.to_owned()), ..Borders::default() };
        b.reload_config();
        b.map = read_borders(dir);
        b
    }

    pub fn reload_config(&mut self) {
        let Some(dir) = &self.dir else { return };
        let text = std::fs::read_to_string(dir.join("config.toml")).or_else(|_| std::fs::read_to_string(dir.join("config.yml")));
        self.config = text.map(|t| BorderConfig::parse(&t)).unwrap_or_default();
    }

    /// `ReloadCommandEvent`: the config and the borders again, announcing each border as changed.
    pub fn reload(&mut self) {
        self.reload_config();
        if let Some(dir) = self.dir.clone() {
            self.map = read_borders(&dir);
        }
        for b in self.map.values() {
            self.events.push(BorderEvent::Change { world: b.world.clone(), border: Some(b.clone()) });
        }
    }

    fn save(&self) {
        let Some(dir) = &self.dir else { return };
        let _ = std::fs::create_dir_all(dir);
        let tmp = dir.join("borders.json.tmp");
        if std::fs::write(&tmp, borders_json(&self.map)).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join("borders.json"));
        }
    }

    pub fn get(&self, world: &str) -> Option<&BorderData> {
        self.map.get(world)
    }

    pub fn all(&self) -> impl Iterator<Item = &BorderData> {
        self.map.values()
    }

    /// `addBorder`: replaces the world's border, keeping its wrap mode.
    pub fn add(&mut self, mut data: BorderData) {
        if let Some(old) = self.map.get(&data.world) {
            data.wrap = old.wrap;
        }
        self.events.push(BorderEvent::Change { world: data.world.clone(), border: Some(data.clone()) });
        self.map.insert(data.world.clone(), data);
        self.save();
    }

    /// `removeBorder`: false when the world had none.
    pub fn remove(&mut self, world: &str) -> bool {
        if self.map.remove(world).is_none() {
            return false;
        }
        self.events.push(BorderEvent::Change { world: world.to_owned(), border: None });
        self.save();
        true
    }

    /// `setWrap`: false when the world has no border.
    pub fn set_wrap(&mut self, world: &str, wrap: Wrap) -> bool {
        let Some(b) = self.map.get_mut(world) else { return false };
        b.wrap = wrap;
        let copy = b.clone();
        self.events.push(BorderEvent::Change { world: world.to_owned(), border: Some(copy) });
        self.save();
        true
    }

    /// `isInsideBorder` of the API: true when the world has no border.
    pub fn inside(&self, world: &str, x: f64, z: f64) -> bool {
        self.map.get(world).is_none_or(|b| b.is_bounding(x, z))
    }

    pub fn is_bypassing(&self, player: u128) -> bool {
        self.bypass.contains(&player)
    }

    pub fn set_bypassing(&mut self, player: u128, on: bool) {
        if on {
            self.bypass.insert(player);
        } else {
            self.bypass.remove(&player);
        }
    }

    /// The online players the glue saw (name, uuid), for `border bypass <name>` and the sender's own switch.
    pub fn set_online(&mut self, players: &[(String, u128)]) {
        self.online = players.iter().map(|(n, u)| (n.to_ascii_lowercase(), (n.clone(), *u))).collect();
    }

    /// The online player called `name`: their name as the game spells it, and their id.
    pub fn player_named(&self, name: &str) -> Option<(String, u128)> {
        self.online.get(&name.to_ascii_lowercase()).cloned()
    }

    /// `PlayerQuitEvent`: the player's data goes, the bypass switch with it.
    pub fn forget(&mut self, player: u128) {
        self.last.remove(&player);
        self.bypass.remove(&player);
    }

    pub fn take_events(&mut self) -> Vec<BorderEvent> {
        std::mem::take(&mut self.events)
    }

    /// `BorderCheckTask.check` for one player: remembers the position while inside, and outside (unless the player
    /// bypasses) says where to send them: the wrapped position, else back to the last inside position, else the spawn.
    pub fn check(
        &mut self,
        player: u128,
        world: &str,
        pos: [f64; 3],
        rot: [f32; 2],
        spawn: Option<[f64; 3]>,
        permission_bypass: bool,
    ) -> Option<Redirect> {
        let border = self.map.get(world)?;
        if border.is_bounding(pos[0], pos[2]) {
            self.last.insert(player, Last { world: world.to_owned(), pos });
            return None;
        }
        if self.bypass.contains(&player) || permission_bypass {
            return None;
        }
        let redirect = if let Some(w) = border.wrapped(border.wrap, pos[0], pos[2]) {
            let yaw = match (w.yaw, w.dir) {
                (Some(y), _) => y,
                (None, Some(d)) => yaw_of(d),
                (None, None) => rot[0],
            };
            let pitch = if w.dir.is_some() { 0.0 } else { rot[1] };
            Redirect { to: [w.x, pos[1], w.z], yaw, pitch, kind: RedirectKind::Wrap, needs_ground: true }
        } else {
            let back = self.last.get(&player).filter(|l| l.world == world).map(|l| l.pos).or(spawn)?;
            Redirect { to: back, yaw: rot[0], pitch: rot[1], kind: RedirectKind::Bounce, needs_ground: false }
        };
        self.last.insert(player, Last { world: world.to_owned(), pos: redirect.to });
        self.events.push(BorderEvent::Redirect { player, world: world.to_owned(), from: pos, to: redirect.to, kind: redirect.kind });
        Some(redirect)
    }

    /// The player's last position inside, for the glue after a wrap landed on a surface.
    pub fn set_last(&mut self, player: u128, world: &str, pos: [f64; 3]) {
        self.last.insert(player, Last { world: world.to_owned(), pos });
    }

    /// `PlayerTeleportEvent` (ender pearl, chorus fruit, gateway, respawn): the destination moves inside when it is
    /// outside the border and the player does not bypass. `Some(None)` is the world spawn. The glue sets the height.
    pub fn teleport_redirect(
        &mut self,
        player: u128,
        world: &str,
        to: [f64; 3],
        permission_bypass: bool,
        kind: RedirectKind,
    ) -> Option<Option<Redirect>> {
        let border = self.map.get(world)?;
        if self.bypass.contains(&player) || permission_bypass {
            return None;
        }
        let moved = border.pull_in((to[0], to[2]))?;
        let Some(w) = moved else { return Some(None) };
        let r = Redirect { to: [w.x, to[1], w.z], yaw: w.dir.map_or(0.0, yaw_of), pitch: 0.0, kind, needs_ground: true };
        self.last.insert(player, Last { world: world.to_owned(), pos: r.to });
        // `callRedirectEvent(player, e.getLocation(), insideBorder, type)`: the destination counts as where it came from.
        self.events.push(BorderEvent::Redirect { player, world: world.to_owned(), from: to, to: r.to, kind });
        Some(Some(r))
    }

    /// `CreatureSpawnEvent`: a spawn outside the border is cancelled.
    pub fn spawn_blocked(&self, world: &str, x: f64, z: f64) -> bool {
        self.map.get(world).is_some_and(|b| !b.is_bounding(x, z))
    }

    /// `BlockPlaceEvent` / `BlockBreakEvent`: the block's centre is outside the border and the player has no bypass.
    pub fn block_blocked(&self, world: &str, pos: [i32; 3], permission_bypass: bool) -> bool {
        self.map
            .get(world)
            .is_some_and(|b| !b.is_bounding(f64::from(pos[0]) + 0.5, f64::from(pos[2]) + 0.5) && !permission_bypass)
    }
}

// ---- borders.json (Gson's layout) --------------------------------------------------------------------------------

fn borders_json(map: &BTreeMap<String, BorderData>) -> String {
    if map.is_empty() {
        return "{}".into();
    }
    let mut out = String::from("{\n");
    let last = map.len() - 1;
    for (i, (key, b)) in map.iter().enumerate() {
        out.push_str(&format!(
            "  {}: {{\n    \"world\": {},\n    \"centerX\": {:?},\n    \"centerZ\": {:?},\n    \"radiusX\": {:?},\n    \"radiusZ\": {:?},\n    \"shape\": {},\n    \"wrap\": {}\n  }}{}\n",
            json_str(key),
            json_str(&b.world),
            b.cx,
            b.cz,
            b.rx,
            b.rz,
            json_str(b.shape.name()),
            json_str(b.wrap.name()),
            if i == last { "" } else { "," }
        ));
    }
    out.push('}');
    out
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON value, as far as `borders.json` needs.
#[derive(Clone, Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

struct JsonReader<'a> {
    s: &'a [u8],
    i: usize,
}

impl JsonReader<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        self.ws();
        (self.s.get(self.i) == Some(&c)).then(|| self.i += 1)
    }

    fn value(&mut self) -> Option<Json> {
        self.ws();
        match *self.s.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut out = Vec::new();
                if self.eat(b'}').is_some() {
                    return Some(Json::Obj(out));
                }
                loop {
                    self.ws();
                    let Json::Str(k) = self.value()? else { return None };
                    self.eat(b':')?;
                    out.push((k, self.value()?));
                    if self.eat(b',').is_none() {
                        self.eat(b'}')?;
                        return Some(Json::Obj(out));
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut out = Vec::new();
                if self.eat(b']').is_some() {
                    return Some(Json::Arr(out));
                }
                loop {
                    out.push(self.value()?);
                    if self.eat(b',').is_none() {
                        self.eat(b']')?;
                        return Some(Json::Arr(out));
                    }
                }
            }
            b'"' => {
                self.i += 1;
                let mut bytes = Vec::new();
                loop {
                    let c = *self.s.get(self.i)?;
                    self.i += 1;
                    match c {
                        b'"' => break,
                        b'\\' => {
                            let e = *self.s.get(self.i)?;
                            self.i += 1;
                            match e {
                                b'n' => bytes.push(b'\n'),
                                b't' => bytes.push(b'\t'),
                                b'r' => bytes.push(b'\r'),
                                b'u' => {
                                    let hex = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
                                    self.i += 4;
                                    let ch = char::from_u32(u32::from_str_radix(hex, 16).ok()?).unwrap_or('?');
                                    let mut buf = [0u8; 4];
                                    bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                                }
                                other => bytes.push(other),
                            }
                        }
                        other => bytes.push(other),
                    }
                }
                Some(Json::Str(String::from_utf8_lossy(&bytes).into_owned()))
            }
            b't' if self.s[self.i..].starts_with(b"true") => {
                self.i += 4;
                Some(Json::Bool(true))
            }
            b'f' if self.s[self.i..].starts_with(b"false") => {
                self.i += 5;
                Some(Json::Bool(false))
            }
            b'n' if self.s[self.i..].starts_with(b"null") => {
                self.i += 4;
                Some(Json::Null)
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok().map(Json::Num)
            }
        }
    }
}

fn field<'a>(obj: &'a [(String, Json)], key: &str) -> Option<&'a Json> {
    obj.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn read_borders(dir: &Path) -> BTreeMap<String, BorderData> {
    let mut out = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(dir.join("borders.json")) else { return out };
    let mut reader = JsonReader { s: text.as_bytes(), i: 0 };
    let Some(Json::Obj(entries)) = reader.value() else { return out };
    for (key, v) in entries {
        let Json::Obj(o) = v else { continue };
        let num = |k: &str| match field(&o, k) {
            Some(Json::Num(n)) => Some(*n),
            _ => None,
        };
        let text_of = |k: &str| match field(&o, k) {
            Some(Json::Str(s)) => Some(s.clone()),
            _ => None,
        };
        let world = text_of("world").unwrap_or_else(|| key.clone());
        let shape = text_of("shape").and_then(|s| Shape::parse(&s)).unwrap_or(Shape::Square);
        let data = BorderData {
            world: world.clone(),
            shape,
            cx: num("centerX").unwrap_or(0.0),
            cz: num("centerZ").unwrap_or(0.0),
            rx: num("radiusX").unwrap_or(0.0),
            rz: num("radiusZ").unwrap_or(0.0),
            wrap: text_of("wrap").map_or(Wrap::None, |w| Wrap::parse(&w)),
        };
        out.insert(world, data);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(r: f64) -> BorderData {
        BorderData { world: "world".into(), shape: Shape::Square, cx: 0.0, cz: 0.0, rx: r, rz: r, wrap: Wrap::None }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("chunkyborder-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_shapes_bound_as_chunkys_shapes_do() {
        let sq = square(100.0);
        assert!(sq.is_bounding(100.0, -100.0) && !sq.is_bounding(100.1, 0.0));
        let circle = BorderData { shape: Shape::Circle, ..square(100.0) };
        assert!(circle.is_bounding(60.0, 80.0) && !circle.is_bounding(80.0, 80.0));
        let oval = BorderData { shape: Shape::Ellipse, rx: 100.0, rz: 50.0, ..square(0.0) };
        assert!(oval.is_bounding(0.0, 49.0) && !oval.is_bounding(0.0, 51.0));
        let diamond = BorderData { shape: Shape::Diamond, ..square(100.0) };
        assert!(diamond.is_bounding(49.0, 49.0) && !diamond.is_bounding(60.0, 60.0));
        // A star reaches its points but not the dents between them.
        let star = BorderData { shape: Shape::Star, ..square(100.0) };
        assert!(star.is_bounding(0.0, 0.0) && !star.is_bounding(0.0, 99.0));
    }

    #[test]
    fn wrap_names_read_as_chunkys_wrap_type() {
        assert_eq!(Wrap::parse("true"), Wrap::Default);
        assert_eq!(Wrap::parse("FALSE"), Wrap::None);
        assert_eq!(Wrap::parse("Earth"), Wrap::Earth);
        assert_eq!(Wrap::parse("nonsense"), Wrap::None);
        assert_eq!(Wrap::parse("x").name(), "x");
    }

    #[test]
    fn a_player_outside_goes_back_to_the_last_inside_position() {
        let mut b = Borders::default();
        b.add(square(100.0));
        assert!(b.check(7, "world", [90.0, 70.0, 0.0], [0.0, 0.0], None, false).is_none());
        let r = b.check(7, "world", [101.0, 70.0, 0.0], [45.0, 10.0], None, false).unwrap();
        assert_eq!((r.to, r.yaw, r.pitch, r.kind), ([90.0, 70.0, 0.0], 45.0, 10.0, RedirectKind::Bounce));
        // The spawn is the way back for a player who was never inside.
        let r = b.check(8, "world", [150.0, 70.0, 0.0], [0.0, 0.0], Some([0.5, 64.0, 0.5]), false).unwrap();
        assert_eq!(r.to, [0.5, 64.0, 0.5]);
        assert!(b.check(9, "world", [150.0, 70.0, 0.0], [0.0, 0.0], None, false).is_none(), "no way back, nothing to do");
        // No border in another world.
        assert!(b.check(7, "world_nether", [900.0, 70.0, 0.0], [0.0, 0.0], None, false).is_none());
    }

    #[test]
    fn bypass_by_switch_or_permission_leaves_the_player_alone() {
        let mut b = Borders::default();
        b.add(square(100.0));
        assert!(b.check(1, "world", [500.0, 70.0, 0.0], [0.0, 0.0], Some([0.0; 3]), true).is_none());
        b.set_bypassing(2, true);
        assert!(b.check(2, "world", [500.0, 70.0, 0.0], [0.0, 0.0], Some([0.0; 3]), false).is_none());
        assert!(b.is_bypassing(2));
        b.set_bypassing(2, false);
        assert!(b.check(2, "world", [500.0, 70.0, 0.0], [0.0, 0.0], Some([0.0; 3]), false).is_some());
        assert!(!b.block_blocked("world", [500, 64, 0], true));
        assert!(b.block_blocked("world", [500, 64, 0], false));
        assert!(!b.block_blocked("world", [99, 64, 0], false), "the block's centre is x = 99.5");
        assert!(b.block_blocked("world", [100, 64, 0], false), "the block's centre is x = 100.5");
    }

    #[test]
    fn both_wrap_puts_the_player_three_blocks_inside_the_far_edge() {
        let mut b = Borders::default();
        b.add(BorderData { wrap: Wrap::Both, ..square(100.0) });
        let r = b.check(1, "world", [100.5, 70.0, 20.0], [0.0, 0.0], None, false).unwrap();
        assert_eq!((r.to[0], r.to[2], r.kind, r.needs_ground), (-97.0, 20.0, RedirectKind::Wrap, true));
        let r = b.check(1, "world", [-100.5, 70.0, 20.0], [0.0, 0.0], None, false).unwrap();
        assert_eq!(r.to[0], 97.0);
        let r = b.check(1, "world", [120.0, 70.0, 130.0], [0.0, 0.0], None, false).unwrap();
        assert_eq!((r.to[0], r.to[2]), (-97.0, -97.0), "both axes wrap in the corner");
    }

    #[test]
    fn x_and_z_wrap_only_when_the_other_axis_is_inside() {
        let mut b = Borders::default();
        b.add(BorderData { wrap: Wrap::X, ..square(100.0) });
        // wrapX answers whether z is inside: outside on z too, the player is bounced instead.
        assert!(b.check(1, "world", [101.0, 70.0, 150.0], [0.0, 0.0], Some([1.0, 64.0, 1.0]), false).unwrap().kind == RedirectKind::Bounce);
        assert_eq!(b.check(1, "world", [101.0, 70.0, 50.0], [0.0, 0.0], None, false).unwrap().to[0], -97.0);
        // x wrap does nothing for a z crossing: bounce.
        assert!(b.check(1, "world", [0.0, 70.0, 101.0], [0.0, 0.0], Some([1.0, 64.0, 1.0]), false).unwrap().kind == RedirectKind::Bounce);
        b.set_wrap("world", Wrap::Z);
        assert_eq!(b.check(1, "world", [0.0, 70.0, 101.0], [0.0, 0.0], None, false).unwrap().to[2], -97.0);
    }

    #[test]
    fn earth_wrap_mirrors_across_the_pole_and_faces_back() {
        let mut b = Borders::default();
        b.add(BorderData { wrap: Wrap::Earth, ..square(100.0) });
        let r = b.check(1, "world", [30.0, 70.0, 101.0], [0.0, 0.0], None, false).unwrap();
        assert_eq!((r.to[0], r.to[2], r.yaw), (-30.0, 97.0, 180.0));
        let r = b.check(1, "world", [30.0, 70.0, -101.0], [0.0, 0.0], None, false).unwrap();
        assert_eq!((r.to[0], r.to[2], r.yaw), (-30.0, -97.0, 0.0));
    }

    #[test]
    fn radial_wrap_crosses_the_centre_to_the_other_edge() {
        let mut b = Borders::default();
        b.add(BorderData { shape: Shape::Circle, wrap: Wrap::Default, ..square(100.0) });
        let r = b.check(1, "world", [101.0, 70.0, 0.0], [0.0, 0.0], None, false).unwrap();
        assert_eq!(r.kind, RedirectKind::Wrap);
        assert!((r.to[0] + 97.0).abs() < 1e-6 && r.to[2].abs() < 1e-6, "{:?}", r.to);
        // Facing the way the player was going: east, which Minecraft calls yaw 270.
        assert!((r.yaw - 270.0).abs() < 1e-3, "{}", r.yaw);
    }

    #[test]
    fn a_teleport_outside_is_pulled_in_three_blocks_from_the_edge() {
        let mut b = Borders::default();
        b.add(square(100.0));
        let r = b.teleport_redirect(1, "world", [150.0, 70.0, 0.0], false, RedirectKind::Teleport).unwrap().unwrap();
        assert!((r.to[0] - 97.0).abs() < 1e-6 && r.to[2].abs() < 1e-6);
        assert!(b.teleport_redirect(1, "world", [50.0, 70.0, 0.0], false, RedirectKind::Teleport).is_none());
        assert!(b.teleport_redirect(1, "world", [150.0, 70.0, 0.0], true, RedirectKind::Teleport).is_none());
        assert!(b.teleport_redirect(1, "world_nether", [150.0, 70.0, 0.0], false, RedirectKind::Teleport).is_none());
    }

    #[test]
    fn the_border_file_round_trips_in_gsons_layout() {
        let dir = tmp("file");
        let mut b = Borders::load(&dir);
        b.add(BorderData { shape: Shape::Rectangle, cx: 10.0, cz: -5.5, rx: 300.0, rz: 120.0, wrap: Wrap::Earth, ..square(0.0) });
        b.add(BorderData { world: "world_nether".into(), ..square(75.0) });
        let text = std::fs::read_to_string(dir.join("borders.json")).unwrap();
        assert!(text.contains("\"centerZ\": -5.5") && text.contains("\"shape\": \"rectangle\""), "{text}");
        let again = Borders::load(&dir);
        assert_eq!(again.get("world"), b.get("world"));
        assert_eq!(again.get("world_nether").unwrap().rx, 75.0);
        // A file Gson wrote (a `wrap` the add-on spells `true`, a field we do not know).
        std::fs::write(
            dir.join("borders.json"),
            r#"{"world":{"world":"world","centerX":1.5,"centerZ":0.0,"radiusX":50.0,"radiusZ":50.0,"shape":"circle","wrap":"true","x":[1,2]}}"#,
        )
        .unwrap();
        let g = Borders::load(&dir);
        assert_eq!((g.get("world").unwrap().shape, g.get("world").unwrap().wrap, g.get("world").unwrap().cx), (Shape::Circle, Wrap::Default, 1.5));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_keeps_the_wrap_and_remove_and_wrap_report_what_they_did() {
        let mut b = Borders::default();
        b.add(square(100.0));
        assert!(b.set_wrap("world", Wrap::Radial) && !b.set_wrap("nowhere", Wrap::Radial));
        b.add(square(200.0));
        assert_eq!(b.get("world").unwrap().wrap, Wrap::Radial);
        assert!(b.remove("world") && !b.remove("world"));
        assert!(b.inside("world", 1e9, 1e9), "no border: everything is inside");
        let events = b.take_events();
        assert_eq!(events.len(), 4);
        assert!(matches!(events.last(), Some(BorderEvent::Change { border: None, .. })));
    }

    #[test]
    fn the_visualizer_draws_columns_along_the_near_edge_only() {
        let b = square(100.0);
        let pts = b.visualizer_points([95.0, 64.0, 0.0], 8.0, 0.0);
        assert!(!pts.is_empty());
        // Every point is on the east wall (x = 100), within 8 blocks of the player, in the range of the wall.
        assert!(pts.iter().all(|p| (p[0] - 100.0).abs() < 1e-9 && dist2([95.0, 64.0, 0.0], *p) <= 64.0 + 1e-9), "{pts:?}");
        assert!(pts.iter().any(|p| p[1] > 64.0) && pts.iter().any(|p| p[1] < 64.0), "columns go up and down");
        // Far from every edge: nothing.
        assert!(b.visualizer_points([0.0, 64.0, 0.0], 8.0, 0.0).is_empty());
        // The offset lowers the wall by that part of a block and moves it along the edge.
        let moved = b.visualizer_points([95.0, 64.0, 0.0], 8.0, 0.5);
        assert!(moved.iter().any(|p| (p[1] - 63.5).abs() < 1e-9), "{moved:?}");
        let circle = BorderData { shape: Shape::Circle, ..square(100.0) };
        let ring = circle.visualizer_points([99.0, 64.0, 0.0], 8.0, 0.0);
        assert!(!ring.is_empty());
        assert!(ring.iter().all(|p| (p[0].hypot(p[2]) - 100.0).abs() < 1.0), "points sit on the circle");
        assert!(circle.visualizer_points([0.0, 64.0, 0.0], 8.0, 0.0).is_empty());
    }

    #[test]
    fn the_config_reads_chunkyborders_yml_and_a_toml_with_the_same_keys() {
        let yml = "version: 1\nborder-options:\n  check-interval: 5\n  message: \"&eNo further.\"\n  use-action-bar: false\n  sound: entity.enderman.teleport # comment\n  prevent-enderpearl: true\n  prevent-mob-spawns: true\nmap-options:\n  color: FF0000\n";
        let c = BorderConfig::parse(yml);
        assert_eq!((c.check_interval, c.use_action_bar, c.prevent_enderpearl, c.prevent_mob_spawns), (5, false, true, true));
        assert_eq!(c.message, "&eNo further.");
        assert_eq!(c.sound_key().as_deref(), Some("entity.enderman.teleport"));
        assert_eq!(BorderConfig::default().sound_key().as_deref(), Some("entity.enderman.teleport"));
        let toml = BorderConfig::parse("check-interval = 40\nprevent-trident = true\n");
        assert_eq!((toml.check_interval, toml.prevent_trident, toml.use_action_bar), (40, true, true));
    }
}
