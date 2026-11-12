//! Tasks, commands and the per-tick work, without any game types so it can be tested.

use crate::geom::{self, Chunk, ChunkList, Pattern, Selection, Shape};
use crate::region::{self, ExistingIndex};
use crate::task::{self, Saved};
use crate::trim::{self, TrimJob};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Chunks skipped as already generated per task and tick, at most.
const MAX_SKIPS_PER_TICK: usize = 4096;
/// Entries (held and skipped) waiting in a task's window, at most.
const MAX_QUEUE: usize = 8192;
/// How long `chunky confirm` waits for a trim, in milliseconds.
const CONFIRM_MS: u64 = 60_000;

#[derive(Clone, Debug)]
pub struct Config {
    /// Saved tasks start running again when the server starts.
    pub continue_on_restart: bool,
    /// Seconds between progress lines in the console.
    pub update_interval: u64,
    pub silent: bool,
    /// Chunks held (loading) at once per task.
    pub max_in_flight: usize,
    /// Stop asking for more chunks while the last tick of the level took longer than this.
    pub tick_budget_ms: u64,
    /// A held chunk that has not loaded after this many ticks is given up on.
    pub chunk_timeout_ticks: u64,
    /// Load chunks the save already has fully generated, instead of skipping them.
    pub force_load_existing_chunks: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            continue_on_restart: false,
            update_interval: 1,
            silent: false,
            max_in_flight: 32,
            tick_budget_ms: 35,
            chunk_timeout_ticks: 400,
            force_load_existing_chunks: false,
        }
    }
}

impl Config {
    /// Reads the TOML subset the mod's config uses (`key = value` lines); unknown keys are ignored.
    pub fn parse(text: &str) -> Config {
        let mut c = Config::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim().trim_matches('"'));
            match k {
                "continue-on-restart" => c.continue_on_restart = v == "true",
                "silent" => c.silent = v == "true",
                "update-interval" => c.update_interval = v.parse().unwrap_or(c.update_interval),
                "max-in-flight" => c.max_in_flight = v.parse::<usize>().unwrap_or(c.max_in_flight).max(1),
                "tick-budget-ms" => c.tick_budget_ms = v.parse().unwrap_or(c.tick_budget_ms),
                "chunk-timeout-ticks" => c.chunk_timeout_ticks = v.parse().unwrap_or(c.chunk_timeout_ticks),
                "force-load-existing-chunks" => c.force_load_existing_chunks = v == "true",
                _ => {}
            }
        }
        c
    }
}

/// What the glue has to do after a call: hold or release chunk tickets, print lines.
#[derive(Debug, Default, PartialEq)]
pub struct Actions {
    pub hold: Vec<Chunk>,
    pub release: Vec<Chunk>,
    pub log: Vec<String>,
}

/// Who typed a command, and what the glue read from the game for it.
#[derive(Clone, Debug, Default)]
pub struct Who {
    /// The player's block position and world (`world`, `world_nether`, `world_the_end`), none for the console.
    pub at: Option<([f64; 2], String)>,
    /// The world spawn's block x and z (level.dat), for `chunky spawn`.
    pub spawn: Option<[f64; 2]>,
}

/// One entry of a task's window, in sequence order.
struct Entry {
    chunk: Chunk,
    /// Loaded (or skipped as generated).
    ready: bool,
    /// The task's tick it was taken at.
    since: u64,
    /// A ticket is held for it (skipped chunks hold none).
    held: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Run {
    Running,
    Paused,
}

/// What `confirm` runs: a start over a saved task, a cancel (one world or all), a scheduled trim.
enum Pending {
    Start(Selection, String),
    Cancel(Option<String>),
    Trim(TrimJob),
}

struct Task {
    sel: Selection,
    run: Run,
    done: u64,
    total: u64,
    elapsed_ms: u64,
    /// The `World` / `Csv` pattern's chunk list.
    list: Option<ChunkList>,
    seq: Peekable<Box<dyn Iterator<Item = Chunk> + Send>>,
    exhausted: bool,
    /// Taken chunks in sequence order, held or skipped.
    inflight: VecDeque<Entry>,
    /// How many of `inflight` hold a ticket.
    held: usize,
    /// Which saved chunks are fully generated, read when needed (none: load everything).
    existing: Option<Arc<ExistingIndex>>,
    ticks: u64,
    last_ms: Option<u64>,
    last_msg_ms: Option<u64>,
    last_save_ms: u64,
    session_start: (u64, u64),
    current: Chunk,
}

impl Task {
    fn new(sel: Selection, list: Option<ChunkList>, done: u64, elapsed_ms: u64, run: Run) -> Task {
        let mut seq = sel.sequence(list.clone()).peekable();
        let mut skipped = 0;
        while skipped < done && seq.next().is_some() {
            skipped += 1;
        }
        Task {
            total: sel.total(list.as_ref()),
            sel,
            run,
            done,
            elapsed_ms,
            list,
            seq,
            exhausted: false,
            inflight: VecDeque::new(),
            held: 0,
            existing: None,
            ticks: 0,
            last_ms: None,
            last_msg_ms: None,
            last_save_ms: 0,
            session_start: (done, elapsed_ms),
            current: (0, 0),
        }
    }

    /// Puts the held chunks back at the start of the sequence's unprocessed part.
    fn rewind(&mut self) -> Vec<Chunk> {
        let held: Vec<Chunk> = self.inflight.drain(..).filter(|e| e.held).map(|e| e.chunk).collect();
        self.held = 0;
        self.seq = self.sel.sequence(self.list.clone()).peekable();
        for _ in 0..self.done {
            self.seq.next();
        }
        self.exhausted = false;
        held
    }

    fn percent(&self) -> f64 {
        if self.total == 0 {
            100.0
        } else {
            (self.done as f64 * 100.0 / self.total as f64).min(100.0)
        }
    }

    fn rate(&self) -> f64 {
        let ms = self.elapsed_ms.saturating_sub(self.session_start.1);
        if ms == 0 {
            0.0
        } else {
            (self.done - self.session_start.0) as f64 * 1000.0 / ms as f64
        }
    }

    fn eta_s(&self) -> Option<u64> {
        let r = self.rate();
        (r > 0.0).then(|| ((self.total.saturating_sub(self.done)) as f64 / r) as u64)
    }
}

pub fn hms(s: u64) -> String {
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// `world`, `overworld`, `minecraft:overworld`... to Chunky's world names.
pub fn canon_world(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "world" | "overworld" | "minecraft:overworld" => Some("world"),
        "world_nether" | "the_nether" | "nether" | "minecraft:the_nether" => Some("world_nether"),
        "world_the_end" | "the_end" | "end" | "minecraft:the_end" => Some("world_the_end"),
        _ => None,
    }
}

pub fn dimension_of(world: &str) -> &'static str {
    match world {
        "world_nether" => "minecraft:the_nether",
        "world_the_end" => "minecraft:the_end",
        _ => "minecraft:overworld",
    }
}

pub struct Engine {
    pub cfg: Config,
    /// The mod's config folder (`config/chunky`): config.toml, CSV files, scans, the pending trim.
    base: Option<PathBuf>,
    /// `<base>/tasks`.
    dir: Option<PathBuf>,
    /// The open world's folder, from the glue.
    world_dir: Option<PathBuf>,
    /// An action waiting for `chunky confirm`, until the time given (Chunky's pending action, one per server here).
    pending: Option<(Pending, u64)>,
    tasks: BTreeMap<String, Task>,
    selection: Selection,
    selected_world: String,
    /// Tickets to let go of, by world, applied on that level's next tick (the ticket call needs its level in place).
    release_later: BTreeMap<String, Vec<Chunk>>,
    border_request: Option<String>,
    last_level_ms: u64,
    /// Chunks that are in the world now, by dimension: a chunk loaded as a neighbour of an earlier one is done on arrival.
    loaded: HashMap<String, HashSet<Chunk>>,
}

impl Engine {
    pub fn new(cfg: Config, dir: Option<PathBuf>) -> Engine {
        Engine {
            cfg,
            base: None,
            dir,
            world_dir: None,
            pending: None,
            tasks: BTreeMap::new(),
            selection: Selection {
                shape: Shape::Square,
                pattern: Pattern::Ring,
                cx: 0.0,
                cz: 0.0,
                radius: 500.0,
                radius_z: 500.0,
            },
            selected_world: "world".into(),
            release_later: BTreeMap::new(),
            border_request: None,
            last_level_ms: 0,
            loaded: HashMap::new(),
        }
    }

    pub fn set_dir(&mut self, dir: PathBuf) {
        self.dir = Some(dir);
    }

    /// The mod's config folder; tasks live in its `tasks` folder.
    pub fn set_base(&mut self, base: PathBuf) {
        self.dir = Some(base.join("tasks"));
        self.base = Some(base);
    }

    /// The open world's folder (the glue's `native_world_dir`).
    pub fn set_world_dir(&mut self, dir: Option<PathBuf>) {
        if dir.is_some() {
            self.world_dir = dir;
        }
    }

    pub fn has_world_dir(&self) -> bool {
        self.world_dir.is_some()
    }

    /// The chunk list of a `World` or `Csv` selection (None for the other patterns). `World` scans the region files
    /// when `fresh`, else reuses the scan saved at the task's start (`scans/<world>.csv`), so a resumed task walks
    /// the same list.
    fn list_for(&self, world: &str, sel: &Selection, fresh: bool) -> Result<Option<ChunkList>, String> {
        let base = self.base.as_deref();
        match &sel.pattern {
            Pattern::Csv(name) => {
                let file = name.as_str();
                let base = base.ok_or("the config folder is not known yet")?;
                let path = base.join(format!("{file}.csv"));
                let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                Ok(Some(Arc::new(parse_csv(&text))))
            }
            Pattern::World => {
                let scan = base.map(|b| b.join("scans").join(format!("{world}.csv")));
                if !fresh {
                    if let Some(text) = scan.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
                        return Ok(Some(Arc::new(parse_csv(&text))));
                    }
                }
                let wd = self.world_dir.as_deref().ok_or("the world folder is not known yet")?;
                let chunks = region::scan_existing(&region::dimension_dir(wd, world));
                if let Some(p) = scan {
                    let _ = std::fs::create_dir_all(p.parent().unwrap_or(Path::new(".")));
                    let text: String = chunks.iter().map(|c| format!("{},{}\n", c.0, c.1)).collect();
                    let _ = std::fs::write(p, text);
                }
                Ok(Some(Arc::new(chunks)))
            }
            _ => Ok(None),
        }
    }

    /// Reads the saved tasks. With `continue_on_restart` unfinished ones run again, else they wait for `chunky continue`.
    pub fn load_tasks(&mut self) -> Vec<String> {
        let mut log = Vec::new();
        let Some(dir) = self.dir.clone() else { return log };
        let Ok(rd) = std::fs::read_dir(&dir) else { return log };
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        files.sort();
        for p in files {
            if p.extension().and_then(|e| e.to_str()) != Some("properties") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            match task::parse(&text) {
                Ok(s) if !s.cancelled => {
                    let Some(w) = canon_world(&s.world) else {
                        log.push(format!("[Chunky] Skipping saved task for unknown world {}", s.world));
                        continue;
                    };
                    let run = if self.cfg.continue_on_restart { Run::Running } else { Run::Paused };
                    let list = match self.list_for(w, &s.sel, false) {
                        Ok(l) => l,
                        Err(e) => {
                            log.push(format!("[Chunky] Skipping saved task for {w}: {e}"));
                            continue;
                        }
                    };
                    log.push(format!(
                        "[Chunky] Loaded saved task for {w}: {} chunks done, {}",
                        s.chunks,
                        if run == Run::Running { "continuing" } else { "paused" }
                    ));
                    self.tasks.insert(w.to_owned(), Task::new(s.sel, list, s.chunks, s.time_ms, run));
                }
                Ok(_) => {}
                Err(e) => log.push(format!("[Chunky] Skipping {}: {e}", p.display())),
            }
        }
        log
    }

    fn save(&self, world: &str, cancelled: bool, t: &Task) {
        let Some(dir) = &self.dir else { return };
        let _ = std::fs::create_dir_all(dir);
        let saved =
            Saved { world: world.to_owned(), cancelled, sel: t.sel.clone(), chunks: t.done, time_ms: t.elapsed_ms };
        let path = dir.join(format!("{world}.properties"));
        let tmp = dir.join(format!("{world}.properties.tmp"));
        if std::fs::write(&tmp, task::render(&saved)).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    /// Writes every live task (the server is stopping, or a periodic save).
    pub fn save_all(&self) {
        for (w, t) in &self.tasks {
            self.save(w, false, t);
        }
    }

    pub fn any_running_in(&self, dim: &str) -> bool {
        self.tasks.iter().any(|(w, _)| dimension_of(w) == dim)
    }

    pub fn level_tick_done(&mut self, took_ms: u64) {
        self.last_level_ms = took_ms;
    }

    /// A chunk came into the world (any chunk; only held ones matter).
    pub fn chunk_loaded(&mut self, dim: &str, chunk: Chunk) {
        self.loaded.entry(dim.to_owned()).or_default().insert(chunk);
        for (w, t) in &mut self.tasks {
            if dimension_of(w) == dim {
                if let Some(e) = t.inflight.iter_mut().find(|e| e.held && e.chunk == chunk) {
                    e.ready = true;
                }
            }
        }
    }

    /// A chunk left the world.
    pub fn chunk_unloaded(&mut self, dim: &str, chunk: Chunk) {
        if let Some(set) = self.loaded.get_mut(dim) {
            set.remove(&chunk);
        }
    }

    /// One tick of `dim`'s level: retire loaded chunks, hold the next ones, report progress.
    pub fn tick(&mut self, dim: &str, now_ms: u64) -> Actions {
        let mut out = Actions::default();
        for (w, t) in self.release_later.iter_mut() {
            if dimension_of(w) == dim {
                out.release.append(t);
            }
        }
        let cfg = self.cfg.clone();
        let over_budget = self.last_level_ms > cfg.tick_budget_ms;
        let mut finished = Vec::new();
        let mut save = Vec::new();
        let already = self.loaded.get(dim);
        let world_dir = self.world_dir.clone();
        for (w, t) in self.tasks.iter_mut().filter(|(w, _)| dimension_of(w) == dim) {
            if t.run != Run::Running {
                t.last_ms = None;
                continue;
            }
            t.ticks += 1;
            if let Some(last) = t.last_ms {
                t.elapsed_ms += now_ms.saturating_sub(last).min(1000);
            }
            t.last_ms = Some(now_ms);
            while let Some(e) = t.inflight.front() {
                let (c, ready, held) = (e.chunk, e.ready, e.held);
                let timed_out = t.ticks.saturating_sub(e.since) > cfg.chunk_timeout_ticks;
                if !(ready || timed_out) {
                    break;
                }
                if timed_out && !ready {
                    out.log.push(format!("[Chunky] Gave up waiting for chunk {}, {} in {w}", c.0, c.1));
                }
                t.inflight.pop_front();
                if held {
                    out.release.push(c);
                    t.held -= 1;
                }
                t.done += 1;
                t.current = c;
            }
            // Chunks the save has fully generated are skipped, unless forced or the pattern is `world` (whose job is to
            // load existing chunks).
            if t.existing.is_none() && !cfg.force_load_existing_chunks && t.sel.pattern != Pattern::World {
                if let Some(wd) = &world_dir {
                    t.existing = Some(Arc::new(ExistingIndex::new(region::dimension_dir(wd, w))));
                }
            }
            if !over_budget {
                let mut skips = 0;
                while t.held < cfg.max_in_flight && !t.exhausted && t.inflight.len() < MAX_QUEUE {
                    let Some(&c) = t.seq.peek() else {
                        t.exhausted = true;
                        break;
                    };
                    // A chunk outside the shape is walked over, as Chunky's task does (no ticket, counted as done).
                    let skip = if !t.sel.contains(c) {
                        true
                    } else {
                        match &t.existing {
                            Some(ix) if !cfg.force_load_existing_chunks => match ix.full(c) {
                                // Its region is still being read: go on next tick.
                                None => break,
                                Some(full) => full,
                            },
                            _ => false,
                        }
                    };
                    if skip && skips >= MAX_SKIPS_PER_TICK {
                        break;
                    }
                    t.seq.next();
                    if skip {
                        skips += 1;
                        t.inflight.push_back(Entry { chunk: c, ready: true, since: t.ticks, held: false });
                    } else {
                        let ready = already.is_some_and(|l| l.contains(&c));
                        t.inflight.push_back(Entry { chunk: c, ready, since: t.ticks, held: true });
                        t.held += 1;
                        out.hold.push(c);
                    }
                }
            }
            if t.exhausted && t.inflight.is_empty() {
                out.log.push(format!(
                    "[Chunky] Task finished for {w}. Processed: {} chunks ({:.2}%), Total time: {}",
                    t.done,
                    t.percent(),
                    hms(t.elapsed_ms / 1000)
                ));
                finished.push(w.clone());
                continue;
            }
            let due = t.last_msg_ms.is_none_or(|m| now_ms.saturating_sub(m) >= cfg.update_interval * 1000);
            if due && !cfg.silent {
                t.last_msg_ms = Some(now_ms);
                out.log.push(progress_line(w, t));
            }
            if now_ms.saturating_sub(t.last_save_ms) >= 10_000 {
                t.last_save_ms = now_ms;
                save.push(w.clone());
            }
        }
        for w in save {
            if let Some(t) = self.tasks.get(&w) {
                self.save(&w, false, t);
            }
        }
        for w in finished {
            if let Some(t) = self.tasks.remove(&w) {
                self.save(&w, true, &t);
            }
        }
        out
    }

    pub fn take_border_request(&mut self, dim: &str) -> bool {
        if self.border_request.as_deref().is_some_and(|w| dimension_of(w) == dim) {
            self.border_request = None;
            return true;
        }
        false
    }

    /// The border of the selected world, read by the glue: centre and edge length in blocks.
    pub fn set_border(&mut self, centre: [f64; 2], size: f64) -> String {
        self.selection.cx = centre[0];
        self.selection.cz = centre[1];
        self.selection.radius = (size / 2.0).max(1.0);
        self.selection.radius_z = self.selection.radius;
        pre(format!(
            "Center changed to {}, {}. Radius changed to {}.",
            number(self.selection.cx),
            number(self.selection.cz),
            number(self.selection.radius)
        ))
    }

    /// The progress lines for `progress`: running tasks as Chunky's update, paused ones with their count.
    pub fn progress_lines(&self) -> Vec<String> {
        self.tasks
            .iter()
            .map(|(w, t)| match t.run {
                Run::Running => progress_line(w, t),
                Run::Paused => format!("[Chunky] Task paused for {w}. Processed: {} chunks ({:.2}%)", t.done, t.percent()),
            })
            .collect()
    }

    /// Runs one `chunky ...` line (without the slash), as Chunky's command tree does. Held chunks of paused or cancelled
    /// tasks are let go on their level's next tick.
    pub fn command(&mut self, line: &str, who: &Who, now_ms: u64) -> Vec<String> {
        let mut it = line.split_whitespace();
        if it.next().map(|s| s.to_ascii_lowercase()) != Some("chunky".into()) {
            return vec![];
        }
        let args: Vec<&str> = it.collect();
        let player = who.at.is_some();
        let Some((&cmd, rest)) = args.split_first() else { return help(None, player) };
        match cmd.to_ascii_lowercase().as_str() {
            "start" => self.cmd_start(rest, now_ms),
            "pause" => self.cmd_pause(rest),
            "continue" => self.cmd_continue(rest),
            "cancel" => self.cmd_cancel(rest, now_ms),
            "progress" => match self.progress_lines() {
                lines if lines.is_empty() => vec![pre("No tasks running.")],
                lines => lines,
            },
            "world" => self.cmd_world(rest, who),
            "worldborder" => self.cmd_worldborder(rest, who),
            "center" => self.cmd_center(rest, who),
            "spawn" => self.cmd_spawn(who),
            "radius" => self.cmd_radius(rest),
            "corners" => self.cmd_corners(rest),
            "shape" => self.cmd_shape(rest),
            "pattern" => self.cmd_pattern(rest),
            "silent" => self.cmd_silent(),
            "quiet" => self.cmd_quiet(rest),
            "selection" => self.cmd_selection(),
            "trim" => self.cmd_trim(rest, now_ms),
            "confirm" => self.cmd_confirm(now_ms),
            "reload" => self.cmd_reload(rest),
            "limit" => vec![pre("Not ported yet: chunky limit (see GAPS.md).")],
            "debug" => match rest.first().map(|s| s.to_ascii_lowercase()).as_deref() {
                Some("map") => vec![pre("Not ported yet: chunky debug map (see GAPS.md).")],
                _ => vec![help_line("debug").into()],
            },
            "help" => help(rest.first().and_then(|s| s.parse::<i64>().ok()), player),
            _ => help(None, player),
        }
    }

    /// Reads `[world] [shape] [x z] [radius] [z radius]` (the order of Chunky's `start` and `trim`) into the selection,
    /// as far as it goes: like Chunky, what was read stays set when a later argument is wrong. Returns how many words
    /// were used; a wrong argument returns the lines to print (`usage` is the command's help line for a bad number).
    fn apply_area(&mut self, rest: &[&str], usage: &str) -> Result<usize, Vec<String>> {
        let mut i = 0;
        if let Some(&name) = rest.first() {
            match canon_world(name) {
                Some(w) => self.selected_world = w.into(),
                None => return Err(vec![world_not_found(name)]),
            }
            i += 1;
        }
        if let Some(&name) = rest.get(i).filter(|_| rest.len() > 1) {
            match Shape::parse(name) {
                Some(s) => self.selection.shape = s,
                None => return Err(vec![shape_not_found(name)]),
            }
            i += 1;
        }
        if rest.len() > i {
            let x = rest.get(i).and_then(|s| geom::suffixed(s)).filter(|v| !geom::past_world_limit(*v));
            let z = rest.get(i + 1).and_then(|s| geom::suffixed(s)).filter(|v| !geom::past_world_limit(*v));
            match (x, z) {
                (Some(x), Some(z)) => {
                    self.selection.cx = x;
                    self.selection.cz = z;
                }
                _ => return Err(vec![usage.into()]),
            }
            i += 2;
        }
        if rest.len() > i {
            match rest.get(i).and_then(|s| geom::suffixed(s)).filter(|r| *r >= 0.0 && !geom::past_world_limit(*r)) {
                Some(r) => {
                    self.selection.radius = r;
                    self.selection.radius_z = r;
                }
                None => return Err(vec![usage.into()]),
            }
            i += 1;
        }
        if rest.len() > i {
            match rest.get(i).and_then(|s| geom::suffixed(s)).filter(|r| *r >= 0.0 && !geom::past_world_limit(*r)) {
                Some(r) => self.selection.radius_z = r,
                None => return Err(vec![usage.into()]),
            }
            i += 1;
        }
        Ok(i)
    }

    /// `start [world] [shape] [x z] [radius] [z radius]`.
    fn cmd_start(&mut self, rest: &[&str], now_ms: u64) -> Vec<String> {
        if let Err(lines) = self.apply_area(rest, &help_line("start")) {
            return lines;
        }
        let w = self.selected_world.clone();
        if self.tasks.get(&w).is_some_and(|t| t.run == Run::Running) {
            return vec![pre(format!("Task already started for {w}!"))];
        }
        let sel = self.selection.clone();
        if self.tasks.contains_key(&w) {
            // A saved task is waiting for this world: starting over needs `confirm`, as Chunky asks.
            self.pending = Some((Pending::Start(sel, w), now_ms + CONFIRM_MS));
            return vec![pre(
                "A task was already started for this world. To continue running it, type '/chunky continue'. To start a new task, type '/chunky confirm'.",
            )];
        }
        self.start_task(&w, sel)
    }

    /// Starts a task for `w` with the selection `sel`, replacing a paused one.
    fn start_task(&mut self, w: &str, sel: Selection) -> Vec<String> {
        let list = match self.list_for(w, &sel, true) {
            Ok(l) => l,
            Err(e) => return vec![pre(format!("Cannot start the {} pattern: {e}", sel.pattern.name()))],
        };
        let t = Task::new(sel.clone(), list, 0, 0, Run::Running);
        if let Some(mut old) = self.tasks.remove(w) {
            let held = old.rewind();
            self.release_later.entry(w.to_owned()).or_default().extend(held);
        }
        self.save(w, false, &t);
        self.tasks.insert(w.to_owned(), t);
        vec![pre(format!(
            "Task started in {w} for the {} region centered at {}, {} with radius {}.",
            sel.shape.name(),
            number(sel.cx),
            number(sel.cz),
            radius_text(&sel)
        ))]
    }

    /// `pause [world]`: pauses the running tasks (or the one of `world`) and saves their progress.
    fn cmd_pause(&mut self, rest: &[&str]) -> Vec<String> {
        if !self.tasks.values().any(|t| t.run == Run::Running) {
            return vec![pre("No tasks to pause.")];
        }
        let running: Vec<String> = match rest.first() {
            Some(name) => {
                let Some(w) = canon_world(name) else { return vec![world_not_found(name)] };
                if !self.tasks.get(w).is_some_and(|t| t.run == Run::Running) {
                    return vec![help_line("pause").into()];
                }
                vec![w.to_owned()]
            }
            None => self.tasks.iter().filter(|(_, t)| t.run == Run::Running).map(|(w, _)| w.clone()).collect(),
        };
        running.iter().map(|w| self.pause_one(w)).collect()
    }

    fn pause_one(&mut self, w: &str) -> String {
        if let Some(t) = self.tasks.get_mut(w) {
            t.run = Run::Paused;
            let held = t.rewind();
            self.release_later.entry(w.to_owned()).or_default().extend(held);
        }
        if let Some(t) = self.tasks.get(w) {
            self.save(w, false, t);
        }
        pre(format!("Task paused for {w}."))
    }

    /// `continue [world]`: resumes the paused tasks (or the one of `world`).
    fn cmd_continue(&mut self, rest: &[&str]) -> Vec<String> {
        let names: Vec<String> = match rest.first() {
            Some(name) => match canon_world(name) {
                Some(w) => vec![w.to_owned()],
                None => return vec![world_not_found(name)],
            },
            None => self.tasks.keys().cloned().collect(),
        };
        let mut out = Vec::new();
        for w in names {
            match self.tasks.get_mut(&w) {
                None => {}
                Some(t) if t.run == Run::Running => out.push(pre(format!("Task already started for {w}!"))),
                Some(t) => {
                    t.run = Run::Running;
                    t.session_start = (t.done, t.elapsed_ms);
                    out.push(pre(format!("Task continuing for {w}.")));
                }
            }
        }
        if out.is_empty() {
            return vec![pre("No tasks to continue.")];
        }
        out
    }

    /// `cancel [world]`: asks for `confirm`, then drops the task(s) for good.
    fn cmd_cancel(&mut self, rest: &[&str], now_ms: u64) -> Vec<String> {
        if self.tasks.is_empty() {
            return vec![pre("No tasks to cancel.")];
        }
        let world = match rest.first() {
            Some(name) => match canon_world(name) {
                Some(w) => Some(w.to_owned()),
                None => return vec![world_not_found(name)],
            },
            None => None,
        };
        self.pending = Some((Pending::Cancel(world), now_ms + CONFIRM_MS));
        vec![pre("Cancelled tasks cannot be continued. If you are sure you want to cancel, type '/chunky confirm'.")]
    }

    fn cancel_now(&mut self, world: Option<&str>) -> Vec<String> {
        let names: Vec<String> = match world {
            Some(w) => {
                if !self.tasks.contains_key(w) {
                    return vec![pre(format!("Task cancelled for {w}."))];
                }
                vec![w.to_owned()]
            }
            None => {
                let mut out = vec![pre("Cancelling all tasks.")];
                out.extend(self.cancel_all());
                return out;
            }
        };
        let mut out = Vec::new();
        for w in names {
            out.push(pre(format!("Task cancelled for {w}.")));
            if let Some(mut t) = self.tasks.remove(&w) {
                let held = t.rewind();
                self.release_later.entry(w.clone()).or_default().extend(held);
                self.save(&w, true, &t);
            }
        }
        out
    }

    fn cancel_all(&mut self) -> Vec<String> {
        let names: Vec<String> = self.tasks.keys().cloned().collect();
        for w in names {
            if let Some(mut t) = self.tasks.remove(&w) {
                let held = t.rewind();
                self.release_later.entry(w.clone()).or_default().extend(held);
                self.save(&w, true, &t);
            }
        }
        Vec::new()
    }

    /// `world [world]`: the world the selection is in (a player's own when none is given).
    fn cmd_world(&mut self, rest: &[&str], who: &Who) -> Vec<String> {
        let target = match rest.first() {
            Some(name) => match canon_world(name) {
                Some(w) => w.to_owned(),
                None => return vec![world_not_found(name)],
            },
            None => match &who.at {
                Some((_, w)) => w.clone(),
                None => return vec![help_line("world").into()],
            },
        };
        self.selected_world = target.clone();
        vec![pre(format!("World changed to {target}."))]
    }

    /// `worldborder [world]`: the border is read on the level's next tick (the glue asks for it there).
    fn cmd_worldborder(&mut self, rest: &[&str], who: &Who) -> Vec<String> {
        let _ = who;
        let w = match rest.first() {
            Some(name) => match canon_world(name) {
                Some(w) => w.to_owned(),
                None => return vec![world_not_found(name)],
            },
            None => self.selected_world.clone(),
        };
        self.selected_world = w.clone();
        self.border_request = Some(w.clone());
        vec![pre(format!("Reading the world border of {w} on its next tick..."))]
    }

    /// `center [x z]`: the centre of the selection; with no numbers, the player's own position.
    fn cmd_center(&mut self, rest: &[&str], who: &Who) -> Vec<String> {
        let x = rest.first().and_then(|s| geom::suffixed(s));
        let z = rest.get(1).and_then(|s| geom::suffixed(s));
        let (cx, cz) = match (x, z) {
            (None, None) => match &who.at {
                Some(([px, pz], _)) => (*px, *pz),
                None => return vec![help_line("center").into()],
            },
            (Some(x), Some(z)) => (x, z),
            _ => return vec![help_line("center").into()],
        };
        if geom::past_world_limit(cx) || geom::past_world_limit(cz) {
            return vec![help_line("center").into()];
        }
        self.selection.cx = cx;
        self.selection.cz = cz;
        vec![pre(format!("Center changed to {}, {}.", number(cx), number(cz)))]
    }

    /// `spawn`: the centre on the spawn point (the Overworld's; the other worlds centre on 0, 0).
    fn cmd_spawn(&mut self, who: &Who) -> Vec<String> {
        // Only the Overworld has a spawn in the save; the other worlds centre on 0, 0.
        let [x, z] = if self.selected_world == "world" { who.spawn.unwrap_or([0.0, 0.0]) } else { [0.0, 0.0] };
        self.selection.cx = x;
        self.selection.cz = z;
        vec![pre(format!("Center changed to {}, {}.", number(x), number(z)))]
    }

    /// `radius <radius> [z radius]`: absolute, or `+`/`-` relative to the current radius.
    fn cmd_radius(&mut self, rest: &[&str]) -> Vec<String> {
        let Some(first) = rest.first() else { return vec![help_line("radius").into()] };
        let cur = self.selection.clone();
        let Ok(x) = geom::parse_radius(first, cur.radius) else { return vec![help_line("radius").into()] };
        let z = match rest.get(1) {
            Some(s) => match geom::parse_radius(s, cur.radius_z) {
                Ok(z) => z,
                Err(_) => return vec![help_line("radius").into()],
            },
            None => x,
        };
        self.selection.radius = x;
        self.selection.radius_z = z;
        let mut out = vec![if x == z {
            pre(format!("Radius changed to {}.", number(x)))
        } else {
            pre(format!("Radius changed for x to {}, and for z to {}.", number(x), number(z)))
        }];
        let shape = match cur.shape {
            Shape::Square | Shape::Rectangle => {
                if x == z { Shape::Square } else { Shape::Rectangle }
            }
            Shape::Circle | Shape::Ellipse => {
                if x == z { Shape::Circle } else { Shape::Ellipse }
            }
            other => other,
        };
        if shape != cur.shape {
            self.selection.shape = shape;
            out.push(pre(format!("Shape changed to {}.", shape.name())));
        }
        out
    }

    /// `corners <x1> <z1> <x2> <z2>`: the centre is the midpoint, the radii half the spans.
    fn cmd_corners(&mut self, rest: &[&str]) -> Vec<String> {
        let v: Vec<Option<f64>> = (0..4)
            .map(|i| rest.get(i).and_then(|s| geom::suffixed(s)).filter(|v| !geom::past_world_limit(*v)))
            .collect();
        let [Some(x1), Some(z1), Some(x2), Some(z2)] = [v[0], v[1], v[2], v[3]] else {
            return vec![help_line("corners").into()];
        };
        let (cx, cz) = ((x1 + x2) / 2.0, (z1 + z2) / 2.0);
        let (rx, rz) = ((x1 - x2).abs() / 2.0, (z1 - z2).abs() / 2.0);
        let prev = self.selection.shape;
        self.selection.cx = cx;
        self.selection.cz = cz;
        self.selection.radius = rx;
        self.selection.radius_z = rz;
        let shape = if rx == rz { Shape::Square } else { Shape::Rectangle };
        self.selection.shape = shape;
        let mut out = vec![pre(format!("Center changed to {}, {}.", number(cx), number(cz)))];
        out.push(if rx == rz {
            pre(format!("Radius changed to {}.", number(rx)))
        } else {
            pre(format!("Radius changed for x to {}, and for z to {}.", number(rx), number(rz)))
        });
        if prev != shape {
            out.push(pre(format!("Shape changed to {}.", shape.name())));
        }
        out
    }

    /// `shape <shape>`.
    fn cmd_shape(&mut self, rest: &[&str]) -> Vec<String> {
        let Some(name) = rest.first() else { return vec![help_line("shape").into()] };
        match Shape::parse(name) {
            Some(s) => {
                self.selection.shape = s;
                vec![pre(format!("Shape changed to {}.", s.name()))]
            }
            None => vec![shape_not_found(name)],
        }
    }

    /// `pattern <pattern> [value]`: `csv` takes the name of its file.
    fn cmd_pattern(&mut self, rest: &[&str]) -> Vec<String> {
        let Some(name) = rest.first() else { return vec![help_line("pattern").into()] };
        if !geom::PATTERNS.split('|').any(|p| p == name.to_ascii_lowercase()) {
            return vec![pattern_not_found(name)];
        }
        match Pattern::parse(name, rest.get(1).copied()) {
            Some(p) => {
                let n = p.name();
                self.selection.pattern = p;
                vec![pre(format!("Pattern changed to {n}."))]
            }
            None => vec![help_line("pattern").into()],
        }
    }

    fn cmd_silent(&mut self) -> Vec<String> {
        self.cfg.silent = !self.cfg.silent;
        vec![pre(format!("Silent mode {}.", if self.cfg.silent { "enabled" } else { "disabled" }))]
    }

    /// `quiet <interval>`: seconds between progress lines.
    fn cmd_quiet(&mut self, rest: &[&str]) -> Vec<String> {
        let Some(n) = rest.first().and_then(|s| s.parse::<i64>().ok()) else { return vec![help_line("quiet").into()] };
        let interval = n.max(0) as u64;
        self.cfg.update_interval = interval;
        vec![pre(format!("Quiet interval set to {interval} seconds."))]
    }

    /// `selection`: the world, shape, centre and radius (or radii) the next task would use.
    fn cmd_selection(&self) -> Vec<String> {
        let s = &self.selection;
        vec![
            pre("Current selection"),
            format!("World: {}", self.selected_world),
            format!("Shape: {}", s.shape.name()),
            format!("Center: {}, {}", number(s.cx), number(s.cz)),
            if s.radius == s.radius_z {
                format!("Radius: {}", number(s.radius))
            } else {
                format!("Radii: {}, {}", number(s.radius), number(s.radius_z))
            },
        ]
    }

    /// `trim [world] [shape] [x z] [radius] [z radius] [outside|inside] [filters]`, or `trim abort`.
    fn cmd_trim(&mut self, rest: &[&str], now_ms: u64) -> Vec<String> {
        if rest.len() == 1 && rest[0].eq_ignore_ascii_case("abort") {
            self.pending = None;
            let gone = self.base.as_ref().is_some_and(|b| std::fs::remove_file(b.join(trim::PENDING)).is_ok());
            let line = if gone { "[Chunky] The scheduled trim is called off." } else { "[Chunky] No trim is scheduled." };
            return vec![line.into()];
        }
        let usage = help_line("trim");
        let used = match self.apply_area(rest, usage) {
            Ok(n) => n,
            Err(e) => return e,
        };
        let (inside, tail): (bool, &[&str]) = if rest.len() > 6 {
            let side = rest[6].to_ascii_lowercase();
            if side != "outside" && side != "inside" {
                return vec![usage.into()];
            }
            (side == "inside", &rest[7..])
        } else {
            (false, &rest[used.min(rest.len())..])
        };
        let mut inhabited: Option<i64> = None;
        for (n, token) in tail.iter().enumerate() {
            let (key, value) = match token.split_once('=') {
                None if n == 0 => ("inhabited".to_owned(), *token),
                None => return vec![filter_invalid(token)],
                Some((k, v)) => (k.to_ascii_lowercase(), v),
            };
            match key.as_str() {
                "inhabited" if inhabited.is_none() => match parse_inhabited(value) {
                    Ok(ticks) => inhabited = Some(ticks),
                    Err(e) => return vec![e],
                },
                "inhabited" => return vec![filter_invalid(token)],
                "unmodified" => return vec![pre("Not ported yet: trim filter unmodified (see GAPS.md).")],
                _ => return vec![filter_invalid(token)],
            }
        }
        let w = self.selected_world.clone();
        let Some(wd) = self.world_dir.clone() else { return vec![pre("The world folder is not known yet.")] };
        let Ok(dir) = std::fs::canonicalize(region::dimension_dir(&wd, &w)) else {
            return vec![pre(format!("{w} has nothing saved to trim."))];
        };
        let job = TrimJob { world: w.clone(), dir, sel: self.selection.clone(), inside, inhabited };
        let p = job.preview();
        let side = if inside { "inside" } else { "outside" };
        let mut out = vec![pre(format!(
            "Warning: Chunks in {w} {side} of the {} region centered at {}, {} with radius {} will be permanently deleted. Please make sure you have taken a backup of your server before continuing to avoid potential damage. You will be required to restart your server fully after the task has finished. If you understand and wish to proceed, type '/chunky confirm'.",
            job.sel.shape.name(),
            number(job.sel.cx),
            number(job.sel.cz),
            radius_text(&job.sel)
        ))];
        if let Some(t) = inhabited {
            out.push(pre(format!(
                "Notice: Filtering chunks by a maximum inhabited time of {t} ticks. Trim may take longer than usual to complete."
            )));
        }
        out.push(pre(format!(
            "{}{} of {} saved chunks go: {} region files deleted, {} changed.",
            if inhabited.is_some() { "At most " } else { "" },
            p.chunks_deleted,
            p.chunks_deleted + p.chunks_kept,
            p.files_deleted,
            p.files_changed
        )));
        out.push(pre("This cannot be undone without a backup. It runs at the next server start, before the world loads."));
        out.push(pre(format!("Type '/chunky confirm' within {} s to schedule it.", CONFIRM_MS / 1000)));
        out.extend(p.errors.iter().map(|e| pre(format!("Could not read {e}"))));
        self.pending = Some((Pending::Trim(job), now_ms + CONFIRM_MS));
        out
    }

    /// `confirm`: runs the action a command asked for (start over, cancel, trim).
    fn cmd_confirm(&mut self, now_ms: u64) -> Vec<String> {
        let Some((action, until)) = self.pending.take() else { return vec![pre("Nothing to confirm!")] };
        if now_ms > until {
            return vec![pre("Nothing to confirm!")];
        }
        match action {
            Pending::Start(sel, w) => self.start_task(&w, sel),
            Pending::Cancel(world) => self.cancel_now(world.as_deref()),
            Pending::Trim(job) => self.schedule_trim(job),
        }
    }

    fn schedule_trim(&mut self, job: TrimJob) -> Vec<String> {
        let Some(base) = self.base.clone() else { return vec![pre("The config folder is not known yet.")] };
        let _ = std::fs::create_dir_all(&base);
        match std::fs::write(base.join(trim::PENDING), trim::render(&job)) {
            Ok(()) => vec![pre(format!(
                "Trim of {} scheduled. Back the world up, then stop and start the server to apply it. '/chunky trim abort' calls it off.",
                job.world
            ))],
            Err(e) => vec![pre(format!("Could not schedule the trim: {e}"))],
        }
    }

    /// `reload [config|tasks]`: `tasks` re-reads the task files (none may be running).
    fn cmd_reload(&mut self, rest: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        if rest.first().is_some_and(|s| s.eq_ignore_ascii_case("tasks")) {
            if self.tasks.values().any(|t| t.run == Run::Running) {
                return vec![pre("The configuration cannot be reloaded while tasks are running.")];
            }
            out.extend(self.reload_tasks());
        } else {
            out.extend(self.reload_config());
        }
        out.push(pre("Successfully reloaded configuration."));
        out
    }

    fn reload_config(&mut self) -> Vec<String> {
        let Some(base) = self.base.clone() else { return vec![pre("The config folder is not known yet.")] };
        self.cfg = std::fs::read_to_string(base.join("config.toml")).map(|t| Config::parse(&t)).unwrap_or_default();
        Vec::new()
    }

    /// Reads the task files again (after editing them); every task must be paused first, and they come back paused.
    fn reload_tasks(&mut self) -> Vec<String> {
        self.tasks.clear();
        let keep = std::mem::replace(&mut self.cfg.continue_on_restart, false);
        let mut log = self.load_tasks();
        self.cfg.continue_on_restart = keep;
        log.push(pre(format!("Tasks reloaded: {} saved, all paused.", self.tasks.len())));
        log
    }
}

/// The message prefix of Chunky's prefixed messages.
fn pre(text: impl AsRef<str>) -> String {
    format!("[Chunky] {}", text.as_ref())
}

fn world_not_found(name: &str) -> String {
    pre(format!("World {name} not found. Available worlds: world, world_nether, world_the_end"))
}

fn shape_not_found(name: &str) -> String {
    pre(format!("Shape {name} not found. Available shapes: {}", geom::SHAPES_SORTED))
}

fn pattern_not_found(name: &str) -> String {
    pre(format!("Pattern {name} not found. Available patterns: {}", geom::PATTERNS_SORTED))
}

fn filter_invalid(token: &str) -> String {
    pre(format!("Trim filter {token} is invalid. Available filters: inhabited, unmodified"))
}

/// `inhabited` ticks: a number, or a number with `s`, `m`, `h` or `d` (seconds, minutes, hours, days). A range is not
/// ported yet.
fn parse_inhabited(value: &str) -> Result<i64, String> {
    if value.contains("..") {
        return Err(pre("Not ported yet: an inhabited range (see GAPS.md)."));
    }
    let (amount, mul) = match value.chars().last().map(|c| c.to_ascii_lowercase()) {
        Some('s') => (&value[..value.len() - 1], 20.0),
        Some('m') => (&value[..value.len() - 1], 1200.0),
        Some('h') => (&value[..value.len() - 1], 72_000.0),
        Some('d') => (&value[..value.len() - 1], 1_728_000.0),
        _ => (value, 1.0),
    };
    match amount.parse::<f64>() {
        Ok(v) if v.is_finite() && v >= 0.0 => Ok((v * mul).round() as i64),
        _ => Err(filter_invalid(value)),
    }
}

/// "100 by 50" for rectangles and ellipses, one number otherwise (Chunky's `Formatting.radius`).
fn radius_text(s: &Selection) -> String {
    if s.shape.two_radii() {
        format!("{}, {}", number(s.radius), number(s.radius_z))
    } else {
        number(s.radius)
    }
}

/// Chunky's `number`: at most two decimals, none of them trailing.
fn number(v: f64) -> String {
    geom::number(v)
}

/// `task_update` in Chunky's wording: the processed count, percent, ETA, rate and the chunk being loaded.
fn progress_line(w: &str, t: &Task) -> String {
    let eta = t.eta_s().unwrap_or(0);
    format!(
        "[Chunky] Task running for {w}. Processed: {} chunks ({:.2}%), ETA: {}:{:02}:{:02}, Rate: {:.1} cps, Current: {}, {}",
        t.done,
        t.percent(),
        eta / 3600,
        eta / 60 % 60,
        eta % 60,
        t.rate(),
        t.current.0,
        t.current.1
    )
}

/// The help line of a command, as Chunky's `help_*` message (its colour codes left out).
fn help_line(name: &str) -> &'static str {
    match name {
        "cancel" => "chunky cancel - Stop and delete current or saved tasks",
        "center" => "chunky center <x> <z> - Set the center block location",
        "continue" => "chunky continue - Continue current or saved tasks",
        "corners" => "chunky corners <x1> <z1> <x2> <z2> - Set by corners",
        "debug" => "chunky debug - Debug tools",
        "limit" => "chunky limit <rate> - Set the max chunks per second",
        "pattern" => "chunky pattern <pattern> - Set the generation pattern",
        "pause" => "chunky pause - Pause current tasks and save progress",
        "progress" => "chunky progress - Displays current task progress",
        "quiet" => "chunky quiet <interval> - Set the quiet interval",
        "radius" => "chunky radius <radius> - Set the radius",
        "reload" => "chunky reload - Reload configuration",
        "selection" => "chunky selection - Display the current selection",
        "shape" => "chunky shape <shape> - Set the shape to generate",
        "silent" => "chunky silent - Toggle displaying update messages",
        "spawn" => "chunky spawn - Set the center to the spawn point",
        "start" => "chunky start - Start a new chunk generation task",
        "trim" => "chunky trim - Delete chunks outside selection",
        "world" => "chunky world <world> - Set the world target",
        "worldborder" => "chunky worldborder - Use the world border center & radius",
        _ => "chunky help - Show the commands",
    }
}

/// Chunky's help menu (`HelpCommand`): every command for the console; eight to a page for a player.
fn help(page: Option<i64>, player: bool) -> Vec<String> {
    const LISTED: [&str; 18] = [
        "start",
        "pause",
        "continue",
        "cancel",
        "world",
        "worldborder",
        "center",
        "spawn",
        "radius",
        "corners",
        "shape",
        "pattern",
        "silent",
        "quiet",
        "trim",
        "selection",
        "progress",
        "reload",
    ];
    let last = LISTED.len() / 8;
    let index = match page {
        None => 0,
        Some(p) => ((p - 1).max(0) as usize) % (last + 1),
    };
    let (first, end) = if player { (8 * index, (8 * index + 8).min(LISTED.len())) } else { (0, LISTED.len()) };
    let mut out = vec!["Chunky Commands".to_owned()];
    out.extend(LISTED[first..end].iter().map(|c| help_line(c).to_owned()));
    if player && index != last {
        out.push(format!("Type /chunky help {} to see more.", index + 2));
    }
    out
}

pub fn parse_csv(text: &str) -> Vec<Chunk> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split([',', ';', ' ', '\t']).map(str::trim).filter(|p| !p.is_empty());
            let x = it.next()?.parse().ok()?;
            let z = it.next()?.parse().ok()?;
            Some((x, z))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use super::*;

    /// A fake level: every held chunk reports loaded `delay` ticks later.
    fn run(e: &mut Engine, ticks: u64, delay: u64, per_tick_ms: u64) -> (u64, Vec<String>, HashSet<Chunk>) {
        let mut pending: Vec<(u64, Chunk)> = Vec::new();
        let mut held: HashSet<Chunk> = HashSet::new();
        let mut ever = HashSet::new();
        let mut log = Vec::new();
        let mut max_held = 0u64;
        for n in 0..ticks {
            let a = e.tick("minecraft:overworld", n * per_tick_ms);
            let quiet = a.hold.is_empty() && a.release.is_empty();
            for c in &a.release {
                assert!(held.remove(c), "released {c:?} that was not held");
            }
            for c in a.hold {
                assert!(held.insert(c), "held {c:?} twice");
                ever.insert(c);
                pending.push((n + delay, c));
            }
            max_held = max_held.max(held.len() as u64);
            let idle = quiet && pending.is_empty();
            log.extend(a.log);
            if e.tasks.is_empty() && pending.is_empty() {
                break;
            }
            if idle {
                // Waiting for a region scan on its thread, as a level's ticks would.
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let (ready, wait): (Vec<_>, Vec<_>) = pending.into_iter().partition(|p| p.0 <= n);
            pending = wait;
            for (_, c) in ready {
                e.chunk_loaded("minecraft:overworld", c);
            }
        }
        (max_held, log, ever)
    }

    fn engine(dir: Option<PathBuf>) -> Engine {
        let mut cfg = Config::default();
        cfg.update_interval = 5;
        Engine::new(cfg, dir)
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("chunky-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_task_runs_to_the_end_and_never_holds_more_than_the_window() {
        let mut e = engine(None);
        let r = e.command("chunky start world square 0 0 100", &Who::default(), 0);
        assert!(r[0].contains("Task started"), "{r:?}");
        let total = e.tasks["world"].total;
        let (max_held, log, ever) = run(&mut e, 5000, 3, 50);
        assert!(max_held <= e.cfg.max_in_flight as u64);
        assert_eq!(ever.len() as u64, total);
        assert!(log.iter().any(|l| l.contains("Task finished for world")), "{log:?}");
        assert!(e.tasks.is_empty());
    }

    #[test]
    fn pause_releases_everything_held_and_continue_resumes_without_gaps() {
        let mut e = engine(None);
        e.command("chunky start world square 0 0 200", &Who::default(), 0);
        let total = e.tasks["world"].total;
        let (_, _, first) = run(&mut e, 20, 3, 50);
        e.command("chunky pause", &Who::default(), 1000);
        let a = e.tick("minecraft:overworld", 1100);
        assert!(a.hold.is_empty());
        let done = e.tasks["world"].done;
        assert!(done > 0 && done < total);
        e.command("chunky continue world", &Who::default(), 2000);
        let (_, log, second) = run(&mut e, 5000, 2, 50);
        let all: HashSet<_> = first.union(&second).copied().collect();
        assert_eq!(all.len() as u64, total, "chunks missing after resume");
        assert!(log.iter().any(|l| l.contains("Task finished")));
    }

    #[test]
    fn a_chunk_that_loaded_earlier_counts_when_held() {
        let mut e = engine(None);
        e.command("chunky start world square 0 0 100", &Who::default(), 0);
        for c in e.tasks["world"].sel.sequence(None).take(4).collect::<Vec<_>>() {
            e.chunk_loaded("minecraft:overworld", c);
        }
        e.tick("minecraft:overworld", 0);
        let a = e.tick("minecraft:overworld", 50);
        assert_eq!(a.release.len(), 4, "pre-loaded chunks are retired at once");
        e.chunk_unloaded("minecraft:overworld", a.release[0]);
        assert!(!e.loaded["minecraft:overworld"].contains(&a.release[0]));
    }

    #[test]
    fn slow_ticks_stop_new_requests() {
        let mut e = engine(None);
        e.command("chunky start world square 0 0 500", &Who::default(), 0);
        e.level_tick_done(80);
        let a = e.tick("minecraft:overworld", 0);
        assert!(a.hold.is_empty());
        e.level_tick_done(10);
        let a = e.tick("minecraft:overworld", 50);
        assert_eq!(a.hold.len(), e.cfg.max_in_flight);
    }

    #[test]
    fn a_chunk_that_never_loads_is_given_up_on() {
        let mut e = engine(None);
        e.cfg.chunk_timeout_ticks = 10;
        e.command("chunky start world square 0 0 20", &Who::default(), 0);
        let mut gave_up = false;
        for n in 0..40 {
            gave_up |= e.tick("minecraft:overworld", n * 50).log.iter().any(|l| l.contains("Gave up"));
        }
        assert!(gave_up);
    }

    #[test]
    fn saved_task_resumes_where_it_stopped() {
        let dir = tmp("resume");
        let mut e = engine(Some(dir.clone()));
        e.command("chunky start world square 0 0 300", &Who::default(), 0);
        let total = e.tasks["world"].total;
        let (_, _, first) = run(&mut e, 30, 2, 50);
        e.command("chunky pause", &Who::default(), 2000);
        let done = e.tasks["world"].done;
        let mut e2 = engine(Some(dir.clone()));
        e2.cfg.continue_on_restart = true;
        let log = e2.load_tasks();
        assert!(log[0].contains(&format!("{done} chunks done")), "{log:?}");
        let (_, _, second) = run(&mut e2, 5000, 2, 50);
        let all: HashSet<_> = first.union(&second).copied().collect();
        assert_eq!(all.len() as u64, total);
        // Finished tasks are saved as cancelled and not loaded again.
        let mut e3 = engine(Some(dir.clone()));
        assert!(e3.load_tasks().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn without_continue_on_restart_the_task_waits() {
        let dir = tmp("wait");
        let mut e = engine(Some(dir.clone()));
        e.command("chunky start world square 0 0 100", &Who::default(), 0);
        e.save_all();
        let mut e2 = engine(Some(dir.clone()));
        e2.load_tasks();
        assert!(e2.tick("minecraft:overworld", 0).hold.is_empty());
        e2.command("chunky continue", &Who::default(), 0);
        assert!(!e2.tick("minecraft:overworld", 50).hold.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cancel_marks_the_file_and_drops_the_task() {
        let dir = tmp("cancel");
        let mut e = engine(Some(dir.clone()));
        e.command("chunky start world square 0 0 100", &Who::default(), 0);
        e.tick("minecraft:overworld", 0);
        let r = e.command("chunky cancel", &Who::default(), 0);
        assert!(r[0].contains("Cancelled tasks cannot be continued"));
        let r = e.command("chunky confirm", &Who::default(), 0);
        assert!(r[0].contains("Cancelling all tasks"), "{r:?}");
        assert!(std::fs::read_to_string(dir.join("world.properties")).unwrap().contains("cancelled=true"));
        let a = e.tick("minecraft:overworld", 50);
        assert_eq!(a.release.len(), e.cfg.max_in_flight);
        assert!(e.command("chunky progress", &Who::default(), 0)[0].contains("No tasks running"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn selection_commands() {
        let mut e = engine(None);
        let w = Who { at: Some(([10.7, -3.2], "world".into())), ..Default::default() };
        e.command("chunky shape circle", &w, 0);
        e.command("chunky center", &w, 0);
        e.command("chunky radius 2k", &w, 0);
        e.command("chunky radius +1k", &w, 0);
        assert_eq!((e.selection.cx, e.selection.cz, e.selection.radius), (10.7, -3.2, 3000.0));
        assert_eq!(e.selection.shape, Shape::Circle);
        assert!(e.command("chunky shape octagon", &w, 0)[0].contains("not found"));
        e.command("chunky worldborder", &w, 0);
        assert!(e.take_border_request("minecraft:overworld"));
        assert!(!e.take_border_request("minecraft:overworld"));
        e.set_border([0.0, 0.0], 20000.0);
        assert_eq!(e.selection.radius, 10000.0);
    }

    #[test]
    fn a_second_start_for_a_running_world_is_refused() {
        let mut e = engine(None);
        e.command("chunky start world square 0 0 100", &Who::default(), 0);
        assert!(e.command("chunky start", &Who::default(), 0)[0].contains("already started"));
    }

    #[test]
    fn config_parses_the_toml_subset() {
        let c = Config::parse("# c\ncontinue-on-restart = true\nupdate-interval = 30 # s\nsilent = false\nmax-in-flight = 9\n");
        assert!(c.continue_on_restart && c.update_interval == 30 && c.max_in_flight == 9);
    }

    fn world_with_region(name: &str, chunks: &[(usize, &str)]) -> PathBuf {
        let d = tmp(name);
        let dim = d.join("dimensions/minecraft/overworld");
        let nbts: Vec<(usize, Vec<u8>)> = chunks.iter().map(|(i, st)| (*i, crate::region::testutil::nbt(st, 0))).collect();
        crate::region::testutil::write_region(&crate::region::region_path(&dim, "region", (0, 0)), &nbts);
        d
    }

    #[test]
    fn spawn_corners_and_two_radii() {
        let mut e = engine(None);
        let w = Who { at: None, spawn: Some([120.0, -48.0]) };
        e.command("chunky spawn", &w, 0);
        assert_eq!((e.selection.cx, e.selection.cz), (120.0, -48.0));
        e.command("chunky world world_nether", &w, 0);
        e.command("chunky spawn", &w, 0);
        assert_eq!((e.selection.cx, e.selection.cz), (0.0, 0.0));
        e.command("chunky corners 0 0 15 31", &w, 0);
        let s = &e.selection;
        assert_eq!((s.shape, s.cx, s.cz, s.radius, s.radius_z), (Shape::Rectangle, 7.5, 15.5, 7.5, 15.5));
        // Chunk radii 1 by 1: the aligned shape is 3 by 3 chunks around the centre chunk (0, 0).
        assert_eq!(s.total(None), 9);
        assert!(s.contains((0, 1)) && s.contains((-1, -1)));
        e.command("chunky corners -500 -500 500 500", &w, 0);
        assert_eq!((e.selection.shape, e.selection.cx, e.selection.cz, e.selection.radius), (Shape::Square, 0.0, 0.0, 500.0));
        e.command("chunky corners -16 -16 15 15", &w, 0);
        assert_eq!((e.selection.shape, e.selection.radius), (Shape::Square, 15.5));
        e.command("chunky radius 100 2c", &w, 0);
        assert_eq!((e.selection.radius, e.selection.radius_z), (100.0, 32.0));
        let r = e.command("chunky selection", &w, 0);
        assert_eq!(r[3..], ["Center: -0.5, -0.5".to_owned(), "Radii: 100, 32".to_owned()]);
        let r = e.command("chunky start world ellipse 10 20 300 150", &w, 0);
        assert!(r[0].contains("Task started in world for the ellipse region centered at 10, 20 with radius 300, 150."), "{r:?}");
        assert!(e.command("chunky start world 1 2 3 4 5", &w, 0)[0].contains("Shape 1 not found"));
        assert!(e.command("chunky pattern spiral", &w, 0)[0].contains("spiral"));
        assert!(e.command("chunky pattern zigzag", &w, 0)[0].contains("Available patterns"));
    }

    #[test]
    fn generated_chunks_are_skipped_unless_forced() {
        // Region (0, 0) holds chunks (0,0) and (1,0) fully generated and (2,0) half generated.
        let d = world_with_region("skip", &[(0, "minecraft:full"), (1, "minecraft:full"), (2, "minecraft:noise")]);
        for force in [false, true] {
            let mut e = engine(None);
            e.cfg.force_load_existing_chunks = force;
            e.set_world_dir(Some(d.clone()));
            e.command("chunky start world square 0 0 64", &Who::default(), 0);
            let total = e.tasks["world"].total;
            let (_, log, ever) = run(&mut e, 2000, 2, 50);
            assert!(log.iter().any(|l| l.contains(&format!("Processed: {total} chunks"))), "{log:?}");
            assert_eq!(ever.contains(&(0, 0)), force);
            assert_eq!(ever.contains(&(1, 0)), force);
            assert!(ever.contains(&(2, 0)), "a half generated chunk is loaded");
            assert_eq!(ever.len() as u64, if force { total } else { total - 2 });
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_pause_while_skipping_resumes_without_gaps() {
        let full: Vec<(usize, &str)> = (0..1024).filter(|i| i % 3 == 0).map(|i| (i, "minecraft:full")).collect();
        let d = world_with_region("skippause", &full);
        let mut e = engine(None);
        e.set_world_dir(Some(d.clone()));
        e.command("chunky start world square 256 256 256", &Who::default(), 0);
        let total = e.tasks["world"].total;
        let (_, _, first) = run(&mut e, 12, 3, 50);
        e.command("chunky pause", &Who::default(), 1000);
        e.tick("minecraft:overworld", 1100);
        e.command("chunky continue", &Who::default(), 2000);
        let (_, log, second) = run(&mut e, 5000, 3, 50);
        assert!(log.iter().any(|l| l.contains("Task finished")));
        let all: HashSet<_> = first.union(&second).copied().collect();
        let skipped = full.len() as u64;
        assert_eq!(all.len() as u64, total - skipped, "every chunk not generated was loaded");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn world_pattern_loads_only_existing_chunks_and_keeps_its_scan() {
        let d = world_with_region("worldpat", &[(0, "minecraft:full"), (40, "minecraft:full"), (1023, "minecraft:full")]);
        let base = d.join("config/chunky");
        let mut e = engine(None);
        e.set_base(base.clone());
        e.set_world_dir(Some(d.clone()));
        e.command("chunky pattern world", &Who::default(), 0);
        let r = e.command("chunky start world square 0 0 160", &Who::default(), 0);
        assert!(r[0].contains("Task started"), "{r:?}");
        assert_eq!(e.tasks["world"].total, 2, "the saved chunks inside the box");
        assert!(std::fs::read_to_string(base.join("scans/world.csv")).unwrap().contains("8,1"));
        let (_, _, ever) = run(&mut e, 200, 2, 50);
        assert_eq!(ever, [(0, 0), (8, 1)].into_iter().collect());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn csv_pattern_reads_the_file_and_fails_without_it() {
        let d = tmp("csv");
        let mut e = engine(None);
        e.set_base(d.clone());
        e.command("chunky pattern csv route", &Who::default(), 0);
        assert!(e.command("chunky start world square 0 0 500", &Who::default(), 0)[0].contains("Cannot start"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("route.csv"), "x,z\n3,4\n-2, 7\n999,999\n").unwrap();
        assert!(e.command("chunky start", &Who::default(), 0)[0].contains("Task started"));
        assert_eq!(e.tasks["world"].total, 3, "the list as read, the chunk outside the shape included");
        let (_, _, ever) = run(&mut e, 100, 1, 50);
        assert_eq!(ever, [(3, 4), (-2, 7)].into_iter().collect());
        assert_eq!(parse_csv("1;2\n# c\n3\t4 extra\n"), vec![(1, 2), (3, 4)]);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn trim_needs_confirm_in_time_and_can_be_called_off() {
        let d = world_with_region("trimcmd", &[(0, "minecraft:full"), (500, "minecraft:full")]);
        let base = d.join("config/chunky");
        let mut e = engine(None);
        e.set_base(base.clone());
        let who = Who::default();
        assert!(e.command("chunky confirm", &who, 0)[0].contains("Nothing to confirm"));
        assert!(e.command("chunky trim world square 0 0 16", &who, 0)[0].contains("world folder"));
        e.set_world_dir(Some(d.clone()));
        // Chunky reads the second radius before the side word: `0 0 16 16 outside`.
        let r = e.command("chunky trim world square 0 0 16 16 outside", &who, 0);
        assert!(r[1].contains("1 of 2 saved chunks go"), "{r:?}");
        assert!(!base.join(trim::PENDING).exists(), "nothing happens before confirm");
        assert!(e.command("chunky confirm", &who, 61_000)[0].contains("Nothing to confirm"));
        e.command("chunky trim world square 0 0 16 16 inside 50", &who, 100_000);
        assert!(e.command("chunky confirm", &who, 100_500)[0].contains("scheduled"));
        let job = trim::parse(&std::fs::read_to_string(base.join(trim::PENDING)).unwrap()).unwrap();
        assert!(job.inside && job.inhabited == Some(50) && job.dir.is_absolute());
        assert!(e.command("chunky trim abort", &who, 0)[0].contains("called off"));
        assert!(!base.join(trim::PENDING).exists());
        assert!(e.command("chunky trim world square 0 0 16 16 sideways", &who, 0)[0].contains("chunky trim"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn reload_tasks_waits_for_pause_and_reload_config_reads_the_file() {
        let d = tmp("reload");
        let mut e = engine(None);
        e.set_base(d.clone());
        e.command("chunky start world square 0 0 100", &Who::default(), 0);
        assert!(e.command("chunky reload tasks", &Who::default(), 0)[0].contains("cannot be reloaded"));
        e.command("chunky pause", &Who::default(), 0);
        let r = e.command("chunky reload tasks", &Who::default(), 0);
        assert!(r.iter().any(|l| l.contains("1 saved, all paused")), "{r:?}");
        std::fs::write(d.join("config.toml"), "max-in-flight = 3\nforce-load-existing-chunks = true\n").unwrap();
        e.command("chunky reload", &Who::default(), 0);
        assert!(e.cfg.max_in_flight == 3 && e.cfg.force_load_existing_chunks);
        let _ = std::fs::remove_dir_all(d);
    }
}
