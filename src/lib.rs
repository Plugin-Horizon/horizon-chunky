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

pub mod horizon_chunky_core;
use horizon_chunky_core as chunky;
use crate::horizon_chunky_core::engine::{Who, canon_world, dimension_of, display_world, set_level_name};
use mod_api::text::TextStyle;
use mod_api::{
    ChunkRef, Command, CommandArgumentError, CommandArgumentKind, CommandArgumentType,
    CommandArgumentValue, CommandSender, CommandSource, CommandSuggestion, Flow, Hooks, HostSide, LevelRef,
    LevelTickPostEvent, LevelTickPreEvent, PlayerCommandEvent, RegisterCommandsEvent, Registrar,
    NodeDefault, PermissionGatherNodesEvent, PermissionLevel, PermissionNode, PermissionValueType,
    RegistrationContext, RunModCommandEvent, PlayerCommandsSentEvent, ServerAboutToStartEvent, ServerHost, ServerStoppingEvent,
    Text, TicketController, TicketOwner,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The owner the engine's tickets are held under. No entity or block is behind it: validation is off, so nothing
/// checks that a block is there.
const OWNER: TicketOwner = TicketOwner::Block([0, 0, 0]);
/// plugin.yml: `chunky.command` (the root, default op) and the operator level the usages are registered at.
const ROOT_NODE: &str = "chunky.command";
const LEVEL: u8 = 2;
/// Bukkit's permission message, which plugin.yml gives every permission-gated command (no `permission-message`).
const NO_PERMISSION: &str = "I'm sorry, but you do not have permission to perform this command. Please contact the server administrators if you believe that this is in error.";

/// The red permission message for a player without the node.
fn no_permission() -> Text {
    let mut style = TextStyle::default();
    style.color = Some("#FF5555".to_string());
    Text::literal(NO_PERMISSION).with_style(style)
}

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

/// Tells the online player called `name` one line, as Chunky tells a task's owner.
fn tell_named(host: &mut dyn ServerHost, name: &str, line: &str) {
    let players = host.players().server_players();
    for player in players {
        if player.name(&*host).is_some_and(|n| n.eq_ignore_ascii_case(name)) {
            let _ = player.tell(host, Text::literal(line.to_owned()));
        }
    }
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

/// Chunky's command tree (`BrigadierCommand.build` in Chunky's common module), one usage per line. Each
/// subcommand usage carries its node (`permission_node`, `register_commands`).
fn usages() -> Vec<String> {
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
        "chunky start <world:chunky:world>",
        "chunky pause",
        "chunky pause <world:chunky:world>",
        "chunky continue",
        "chunky continue <world:chunky:world>",
        "chunky cancel",
        "chunky cancel <world:chunky:world>",
        "chunky progress",
        "chunky world",
        "chunky world <world:chunky:world>",
        "chunky worldborder",
        "chunky worldborder <world:chunky:world>",
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
        "chunky trim <world:chunky:world>",
        "chunky confirm",
        "chunky pattern csv",
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
        "chunky start <world:chunky:world> <shape:chunky:shape> [x:word] [z:word] [rx:word] [rz:word]".into(),
    );
    usages
        .push("chunky trim <world:chunky:world> <shape:chunky:shape> [x:word] [z:word] [rx:word] [rz:word]".into());
    usages.push(
        "chunky trim <world:chunky:world> <shape:chunky:shape> <x:word> <z:word> <rx:word> <rz:word> <side:chunky:side> [filters:text]".into(),
    );
    usages
}

/// Every subcommand word.
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

/// The permission node of a subcommand, `chunky.command.<sub>` (Paper's `plugin.yml`).
fn permission_node(sub: &str) -> Option<String> {
    let sub = sub.to_ascii_lowercase();
    SUBS.contains(&sub.as_str())
        .then(|| format!("chunky.command.{sub}"))
}

/// A typed Boolean node by name; every name used here is declared in `permission_gather_nodes`.
fn typed(name: &str) -> PermissionNode {
    PermissionNode::new(name, PermissionValueType::Boolean)
}

/// A word argument that accepts anything (the engine answers a wrong world or shape with Chunky's own line) and
/// completes from a list, as Chunky's `WorldSuggestions` and shape/pattern suggestions do.
struct Words(fn() -> Vec<String>);

impl CommandArgumentType for Words {
    fn kind(&self) -> CommandArgumentKind {
        CommandArgumentKind::Word
    }

    fn parse(&self, input: &str) -> Result<String, CommandArgumentError> {
        Ok(input.to_owned())
    }

    fn suggest(&self, _: Option<&CommandSender>, prefix: &str) -> Vec<CommandSuggestion> {
        let prefix = prefix.to_ascii_lowercase();
        (self.0)()
            .into_iter()
            .filter(|w| w.to_ascii_lowercase().starts_with(&prefix))
            .map(CommandSuggestion::new)
            .collect()
    }
}

fn world_words() -> Vec<String> {
    ["world", "world_nether", "world_the_end"].iter().map(|w| display_world(w)).collect()
}

fn shape_words() -> Vec<String> {
    SHAPES.iter().map(|s| (*s).to_owned()).collect()
}

fn side_words() -> Vec<String> {
    vec!["outside".to_owned(), "inside".to_owned()]
}

/// `max-world-size` from server.properties, Minecraft's default when it is not there.
fn max_world_size() -> f64 {
    std::fs::read_to_string("server.properties")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("max-world-size="))
                .and_then(|v| v.trim().parse::<f64>().ok())
        })
        .unwrap_or(29_999_984.0)
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
        r.register_command_argument("world", Words(world_words));
        r.register_command_argument("shape", Words(shape_words));
        r.register_command_argument("side", Words(side_words));
    }

    fn permission_gather_nodes(&self, event: &mut PermissionGatherNodesEvent<'_>) {
        // The root node and one per subcommand, each defaulting to operator level 2 (`LEVEL`).
        let level = PermissionLevel::from_level(i32::from(LEVEL));
        let names = std::iter::once(ROOT_NODE.to_string()).chain(SUBS.iter().map(|s| format!("chunky.command.{s}")));
        for name in names {
            let _ = event.nodes.add(typed(&name), NodeDefault::Level(level));
        }
    }

    fn register_commands(&self, _: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        // Each subcommand path carries Chunky's node (`chunky.command.start`), as Brigadier's requires on that
        // literal: a permissions mod can hand one subcommand to a non-operator; level 2 decides when none answers.
        // The bare `/chunky` (help) has no node, so it does not gate the subcommands.
        for usage in usages() {
            let sub = usage.split(' ').nth(1).filter(|w| !w.starts_with('<') && !w.starts_with('['));
            let node = sub.and_then(permission_node);
            // plugin.yml gives Chunky the alias `cy`: the same usages under that label.
            for label in ["chunky", "cy"] {
                let command = Command::op(&usage.replacen("chunky", label, 1), 2);
                event.commands.push(match &node {
                    Some(node) => command.with_node(&typed(node)),
                    None => command,
                });
            }
        }
    }

    /// Tab completion: a player with no chunky node at all (and below the operator level) is not offered
    /// `chunky` or `cy` (Bukkit hides a command the sender may not run).
    fn player_commands_sent(&self, host: &mut dyn ServerHost, event: &mut PlayerCommandsSentEvent<'_>) {
        let may = match host.player(event.player) {
            Some(p) => {
                p.permission_level(&*host).unwrap_or(0) >= LEVEL
                    || p.permission_bool(host, &typed(ROOT_NODE)).unwrap_or(false)
                    || SUBS.iter().any(|s| {
                        p.permission_bool(host, &typed(&format!("chunky.command.{s}"))).unwrap_or(false)
                    })
            }
            None => false,
        };
        if !may {
            event.names.retain(|n| !matches!(n.to_ascii_lowercase().as_str(), "chunky" | "cy"));
        }
    }

    /// A player without the node gets Bukkit's permission message, as the original. The tree already leaves those
    /// usages out for them (`with_node`); this line answers what the tree no longer shows.
    fn player_command(&self, host: &mut dyn ServerHost, event: &mut PlayerCommandEvent<'_>) -> Flow {
        let mut words = event.command.split_whitespace();
        let root = words.next().unwrap_or("").to_ascii_lowercase();
        if root != "chunky" && root != "cy" {
            return Flow::Continue;
        }
        // The subcommand's node as `register_commands` gives it; the bare command and unknown words use the root's.
        let node = words
            .next()
            .and_then(permission_node)
            .unwrap_or_else(|| ROOT_NODE.to_string());
        // Any refused query is a denial; the node's default (operator level 2) is what an operator gets.
        let allowed = match host.player(event.player) {
            Some(p) => p.permission_bool(host, &typed(&node)).unwrap_or(false),
            None => false,
        };
        if allowed {
            return Flow::Continue;
        }
        event.replies.push(no_permission());
        event.cancelled = true;
        Flow::Cancel
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        if !matches!(event.command.literals.first().map(String::as_str), Some("chunky" | "cy")) {
            return Flow::Continue;
        }
        // The line as typed: literals, then the arguments in parse order. `text` is the rest of the line, last.
        // The alias `cy` is parsed as `chunky`.
        let line = std::iter::once("chunky".to_owned())
            .chain(event.command.literals.iter().skip(1).cloned())
            .chain(
                event
                    .command
                    .arguments
                    .iter()
                    .filter_map(|a| match &a.value {
                        CommandArgumentValue::String(s) => Some(s.clone()),
                        CommandArgumentValue::Integer(i) => Some(i.to_string()),
                        CommandArgumentValue::Custom { value, .. } => Some(value.clone()),
                        _ => None,
                    }),
            )
            .collect::<Vec<_>>()
            .join(" ");
        let sender = match &event.source {
            CommandSource::Player { player, .. } => player.name(&*host).unwrap_or_default(),
            _ => String::new(),
        };
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
        // `chunky spawn` centres on the selected world's own spawn.
        let selected = chunky::with(|e| e.selected_world().to_owned());
        let spawn = host
            .level(dimension_of(&selected))
            .and_then(|l| l.respawn_data(&*host))
            .map(|r| [f64::from(r.pos[0]), f64::from(r.pos[2])]);
        let who = Who { at, spawn, sender };
        let world_dir = lock(&self.world_dir).clone();
        let mut lines = chunky::with(|e| {
            e.set_world_dir(world_dir);
            e.command(&line, &who, chunky::now_ms())
        });
        // `chunky worldborder` answers the sender with what it read (Chunky's `WorldBorderCommand`).
        if let Some(world) = chunky::with(|e| e.take_border_world()) {
            lines.extend(self.border_lines(host, &world));
        }
        for l in chunky::with(|e| e.take_console()) {
            log(&l);
        }
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
        set_level_name(&world_dir.to_string_lossy());
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
        for (owner, line) in &actions.notify {
            tell_named(host, owner, line);
        }
        for l in chunky::with(|e| e.take_console()) {
            log(&l);
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

    /// The selected world's border becomes the selection's centre and radius; the answer is Chunky's lines.
    fn border_lines(&self, host: &mut dyn ServerHost, world: &str) -> Vec<String> {
        let border = host
            .level(dimension_of(world))
            .and_then(|level| level.world_border(&*host));
        let Some([x0, z0, x1, z1]) = border else {
            return vec![format!(
                "[Chunky] No world border to read in {}.",
                display_world(world)
            )];
        };
        let centre = [(x0 + x1) / 2.0, (z0 + z1) / 2.0];
        chunky::with(|e| e.set_border(centre, x1 - x0, max_world_size()))
    }
}
