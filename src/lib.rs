//! Chunky as an arismoko native mod (trait mod on `mod_api`). The logic is the unchanged
//! `horizon-chunky-core` crate (`core/`); this file only wires it to hooks, commands and host methods.
//!
//! - Commands: Chunky's tree from `register_commands`, every usage at operator level 2. arismoko hands over the
//!   parsed invocation, not the typed line, so `run_mod_command` rebuilds the line from its literals and then its
//!   arguments, and the engine parses that line as before. Shape and side are words, not literals, because the
//!   rebuilt line cannot place a literal between arguments; tab completion therefore lists no shape names.
//! - Chunk tickets: a running task holds its chunks through the mod's own ticket controller `chunky:tasks`, from
//!   `level_tick_pre`. The engine learns about loaded chunks by diffing `level_loaded_chunks` while a task runs
//!   there, in place of Horizon's `chunk_load` and `chunk_unload` hooks. Tickets a previous run left in the save
//!   are released once per level, at its first tick.
//! - Border: read when `chunky worldborder` asks for it (`level_world_border`), as before. Nothing is set.
//! - Config: `config/chunky/config.toml` and the saved tasks are read at server start, the trim is applied before
//!   any world opens, and the tasks are saved at server stop, as in the Horizon port. No `register_configs`, so no
//!   default file is written.
use horizon_chunky_core as chunky;
use horizon_chunky_core::engine::{Who, canon_world};
use mod_api::{
    ChunkRef, Command, CommandArgumentValue, CommandSource, Flow, Hooks, HostSide, LevelRef,
    LevelTickPostEvent, LevelTickPreEvent, RegisterCommandsEvent, Registrar, RegistrationContext,
    RunModCommandEvent, ServerAboutToStartEvent, ServerHost, ServerStoppingEvent, Text,
    TicketController, TicketOwner,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The owner the engine's tickets are held under. No entity or block is behind it: validation is off, so nothing
/// checks that a block is there.
const OWNER: TicketOwner = TicketOwner::Block([0, 0, 0]);

#[derive(Default)]
struct Mod {
    /// The controller `register` makes; None when the game refused it, which turns the tick work off.
    controller: OnceLock<TicketController>,
    /// The open world's folder (`level-name` in server.properties), for the `world` pattern and trim.
    world_dir: Mutex<Option<PathBuf>>,
    /// Dimensions whose earlier tickets were released already (once per server run).
    swept: Mutex<HashSet<String>>,
    /// Chunks the engine was told are loaded, per dimension, as the diff's last state.
    seen: Mutex<HashMap<String, HashSet<(i32, i32)>>>,
}

fn log(line: &str) {
    eprintln!("[chunky] {line}");
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `level-name` from server.properties, `world` when it is not there.
fn level_name(properties: &str) -> String {
    properties
        .lines()
        .find_map(|line| line.strip_prefix("level-name="))
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "world".into())
}

/// Chunky's command tree (`BrigadierCommand.build` in Chunky's common module), one usage per line. Each
/// subcommand usage carries its node (`permission_node`, `register_commands`).
fn usages() -> Vec<String> {
    const SHAPES: [&str; 9] = [
        "square",
        "circle",
        "rectangle",
        "ellipse",
        "diamond",
        "triangle",
        "pentagon",
        "hexagon",
        "star",
    ];
    const PATTERNS: [&str; 7] = [
        "concentric",
        "loop",
        "spiral",
        "region",
        "world",
        "strip",
        "ring",
    ];
    let mut usages: Vec<String> = [
        "chunky",
        "chunky help",
        "chunky help [page:integer]",
        "chunky start",
        "chunky start <world:word>",
        "chunky pause",
        "chunky pause <world:word>",
        "chunky continue",
        "chunky continue <world:word>",
        "chunky cancel",
        "chunky cancel <world:word>",
        "chunky progress",
        "chunky world",
        "chunky world <world:word>",
        "chunky worldborder",
        "chunky worldborder <world:word>",
        "chunky center",
        "chunky center <x:word> <z:word>",
        "chunky spawn",
        "chunky corners <x1:word> <z1:word> <x2:word> <z2:word>",
        "chunky radius <radius:word>",
        "chunky radius <radius:word> <radiusz:word>",
        "chunky silent",
        "chunky selection",
        "chunky quiet <interval:integer>",
        "chunky limit <rate:integer>",
        "chunky debug",
        "chunky debug map",
        "chunky reload",
        "chunky reload config",
        "chunky reload tasks",
        "chunky trim",
        "chunky trim abort",
        "chunky trim <world:word>",
        "chunky confirm",
        "chunky pattern csv <name:word>",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    for pattern in PATTERNS {
        usages.push(format!("chunky pattern {pattern}"));
    }
    for shape in SHAPES {
        usages.push(format!("chunky shape {shape}"));
    }
    // Shape and side are arguments here (see the module doc); the engine checks the words.
    usages.push(
        "chunky start <world:word> <shape:word> [x:word] [z:word] [rx:word] [rz:word]".into(),
    );
    usages
        .push("chunky trim <world:word> <shape:word> [x:word] [z:word] [rx:word] [rz:word]".into());
    usages.push(
        "chunky trim <world:word> <shape:word> <x:word> <z:word> <rx:word> <rz:word> <side:word> [filters:text]".into(),
    );
    usages
}

/// The permission node of a subcommand, `chunky.command.<sub>` (Paper's `plugin.yml`).
fn permission_node(sub: &str) -> Option<String> {
    const SUBS: [&str; 22] = [
        "cancel",
        "center",
        "confirm",
        "continue",
        "corners",
        "debug",
        "help",
        "limit",
        "pattern",
        "pause",
        "progress",
        "quiet",
        "radius",
        "reload",
        "selection",
        "shape",
        "silent",
        "spawn",
        "start",
        "trim",
        "world",
        "worldborder",
    ];
    let sub = sub.to_ascii_lowercase();
    SUBS.contains(&sub.as_str())
        .then(|| format!("chunky.command.{sub}"))
}

/// Holds (`forced`) or releases one chunk of `level` through the mod's controller.
fn force(
    host: &mut dyn ServerHost,
    level: LevelRef,
    controller: TicketController,
    chunk: (i32, i32),
    forced: bool,
) {
    let _ = ChunkRef::new(level, [chunk.0, chunk.1])
        .set_mod_forced(host, controller, OWNER, forced, false);
}

#[mod_api::horizon_mod]
impl Hooks for Mod {
    fn register(&self, r: &mut dyn Registrar, _: &RegistrationContext) {
        // Named `tasks` in the mod's namespace: `chunky:tasks`.
        let _ = self
            .controller
            .set(r.register_ticket_controller("tasks", false));
    }

    fn register_commands(&self, _: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        // Each subcommand path carries Chunky's node (`chunky.command.start`), as Brigadier's requires on that
        // literal: a permissions mod can hand one subcommand to a non-operator; level 2 decides when none answers.
        // The bare `/chunky` (help) has no node, so it does not gate the subcommands.
        for usage in usages() {
            let command = Command::op(&usage, 2);
            let sub = usage.split(' ').nth(1).filter(|w| !w.starts_with('<') && !w.starts_with('['));
            event.commands.push(match sub.and_then(permission_node) {
                Some(node) => command.with_node(&node),
                None => command,
            });
        }
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        if event.command.literals.first().map(String::as_str) != Some("chunky") {
            return Flow::Continue;
        }
        // The line as typed: literals, then the arguments in parse order. `text` is the rest of the line, last.
        let line = event
            .command
            .literals
            .iter()
            .cloned()
            .chain(
                event
                    .command
                    .arguments
                    .iter()
                    .filter_map(|a| match &a.value {
                        CommandArgumentValue::String(s) => Some(s.clone()),
                        CommandArgumentValue::Integer(i) => Some(i.to_string()),
                        _ => None,
                    }),
            )
            .collect::<Vec<_>>()
            .join(" ");
        let at = match &event.source {
            CommandSource::Player { player, .. } => {
                let entity = player.entity();
                let dimension = entity
                    .level(&*host)
                    .map(|l| l.dimension_id())
                    .unwrap_or("minecraft:overworld");
                entity.position(&*host).map(|p| {
                    (
                        [p[0], p[2]],
                        canon_world(dimension).unwrap_or("world").to_string(),
                    )
                })
            }
            _ => None,
        };
        let spawn = host
            .level("minecraft:overworld")
            .and_then(|l| l.respawn_data(&*host))
            .map(|r| [f64::from(r.pos[0]), f64::from(r.pos[2])]);
        let who = Who { at, spawn };
        let world_dir = lock(&self.world_dir).clone();
        let lines = chunky::with(|e| {
            e.set_world_dir(world_dir);
            e.command(&line, &who, chunky::now_ms())
        });
        for l in lines {
            let _ = host.commands().feedback(Text::literal(l));
        }
        Flow::Cancel
    }

    fn server_about_to_start(&self, _: &mut dyn ServerHost, _: &mut ServerAboutToStartEvent) {
        let config = Path::new("config");
        let world_dir = PathBuf::from(
            std::fs::read_to_string("server.properties")
                .map(|t| level_name(&t))
                .unwrap_or_else(|_| "world".into()),
        );
        *lock(&self.world_dir) = Some(world_dir.clone());
        // A trim confirmed during the last run, applied before any world (and so any region file) is open.
        for l in chunky::server_about_to_start(config) {
            log(&l);
        }
        for l in chunky::init_once(config, Some(world_dir)) {
            log(&l);
        }
    }

    fn server_stopping(&self, _: &mut dyn ServerHost, _: &mut ServerStoppingEvent) {
        chunky::with(|e| e.save_all());
    }

    fn level_tick_pre(&self, host: HostSide, event: &mut LevelTickPreEvent) {
        let HostSide::Server(host) = host else {
            return;
        };
        let Some(controller) = self.controller.get().copied() else {
            return;
        };
        let level = event.level;
        let dim = level.dimension_id();
        self.sweep_once(host, level, dim, controller);
        if chunky::with(|e| e.take_border_request(dim)) {
            self.border(&*host, level, dim);
        }
        if !chunky::with(|e| e.any_running_in(dim)) {
            // Nothing runs here: the engine forgets the chunks it was told of, so the next run starts from the truth.
            if let Some(prev) = lock(&self.seen).remove(dim) {
                chunky::with(|e| {
                    for c in prev {
                        e.chunk_unloaded(dim, c);
                    }
                });
            }
            return;
        }
        self.diff_loaded(&*host, level, dim);
        chunky::level_begin();
        let actions = chunky::with(|e| e.tick(dim, chunky::now_ms()));
        for &c in &actions.release {
            force(host, level, controller, c, false);
        }
        for &c in &actions.hold {
            force(host, level, controller, c, true);
        }
        for l in &actions.log {
            log(l);
        }
    }

    fn level_tick_post(&self, host: HostSide, _: &mut LevelTickPostEvent) {
        if matches!(host, HostSide::Server(_)) {
            chunky::level_end();
        }
    }
}

impl Mod {
    /// Releases the tickets of `controller` that an earlier run left in `level`'s save, once per dimension. They would
    /// keep their chunks loaded for good: the engine does not know them, and the controller is registered, so the world
    /// keeps them across restarts.
    fn sweep_once(
        &self,
        host: &mut dyn ServerHost,
        level: LevelRef,
        dim: &str,
        controller: TicketController,
    ) {
        if !lock(&self.swept).insert(dim.to_owned()) {
            return;
        }
        let Some(tickets) = level.mod_forced_chunks(&*host, controller) else {
            return;
        };
        let count = tickets.len();
        for (owner, [x, z], natural) in tickets {
            let _ = ChunkRef::new(level, [x, z])
                .set_mod_forced(host, controller, owner, false, natural);
        }
        if count > 0 {
            log(&format!(
                "released {count} chunk ticket(s) an earlier run left in {dim}"
            ));
        }
    }

    /// Tells the engine which chunks came and went since the last tick (its `chunk_load` and `chunk_unload`).
    fn diff_loaded(&self, host: &dyn ServerHost, level: LevelRef, dim: &str) {
        let Some(list) = level.loaded_chunks(host) else {
            return;
        };
        let now: HashSet<(i32, i32)> = list
            .into_iter()
            .map(|c| {
                let [x, z] = c.pos();
                (x, z)
            })
            .collect();
        let mut seen = lock(&self.seen);
        let prev = seen.remove(dim).unwrap_or_default();
        chunky::with(|e| {
            for &c in now.difference(&prev) {
                e.chunk_loaded(dim, c);
            }
            for &c in prev.difference(&now) {
                e.chunk_unloaded(dim, c);
            }
        });
        seen.insert(dim.to_owned(), now);
    }

    /// The border's centre and size go to the engine's selection (`chunky worldborder`), as the Horizon port did.
    fn border(&self, host: &dyn ServerHost, level: LevelRef, dim: &str) {
        let Some([x0, z0, x1, z1]) = level.world_border(host) else {
            log(&format!("no world border to read in {dim}"));
            return;
        };
        let centre = [(x0 + x1) / 2.0, (z0 + z1) / 2.0];
        let line = chunky::with(|e| e.set_border(centre, x1 - x0));
        log(&line);
    }
}
