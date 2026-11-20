//! Chunky as an arismoko native mod (trait mod on `mod_api`). The logic is the unchanged
//! `horizon-chunky-core` crate (`core/`); this file only wires it to hooks, commands and host methods.
//!
//! - Commands: Chunky's tree from `register_commands`, every usage at operator level 2. arismoko hands over the
//!   parsed invocation, not the typed line, so `run_mod_command` rebuilds the line from its literals and then its
//!   arguments, and the engine parses that line as before. World, shape, side and the trim filters are registered
//!   argument types (`chunky:world`, `chunky:shape`, `chunky:side`, `chunky:filters`): they accept what the engine
//!   checks and complete from Chunky's lists, as its suggestion providers do.
//! - Chunk tickets: a running task holds its chunks through the mod's own ticket controller `chunky:tasks`, from
//!   `level_tick_pre`. The engine learns about loaded chunks by diffing `level_loaded_chunks` while a task runs
//!   there, in place of Horizon's `chunk_load` and `chunk_unload` hooks. Tickets a previous run left in the save
//!   are released once per level, at its first tick.
//! - Border: read when `chunky worldborder` asks for it (`level_world_border`), as before. Nothing is set.
//! - Config: `config/chunky/config.toml` and the saved tasks are read at server start, the trim is applied before
//!   any world opens, and the tasks are saved at server stop, as in the Horizon port. No `register_configs`, so no
//!   default file is written. `language` picks the message file (`lang/<code>.json`); every line the mod prints goes
//!   through `lang::localize` on its way out.

pub mod horizon_chunky_core;
use horizon_chunky_core as chunky;
use crate::horizon_chunky_core::engine::{Who, available_worlds, canon_world, dimension_of, display_world, register_dimension, set_level_name};
use mod_api::text::TextStyle;
use mod_api::{
    ChunkRef, Command, CommandArgumentError, CommandArgumentKind, CommandArgumentType,
    CommandArgumentValue, CommandSender, CommandSource, CommandSuggestion, Flow, Hooks, HostSide, LevelRef,
    LevelTickPostEvent, LevelTickPreEvent, PlayerCommandEvent, RegisterCommandsEvent, RegisterNodeCommandsEvent, Registrar,
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

/// Brigadier's answer for a literal the sender may not use. Chunky's Paper build puts its permission on the command
/// tree, so a player without the node never sees the command (the plain-Bukkit fallback would print a permission message).
const UNKNOWN_COMMAND: &str = "Unknown or incomplete command, see below for error";

/// The answer for a player without the node: the command is hidden, as Chunky's Brigadier tree hides it.
fn no_permission() -> Text {
    let mut style = TextStyle::default();
    style.color = Some("#FF5555".to_string());
    Text::literal(UNKNOWN_COMMAND).with_style(style)
}

/// One run of a message with one formatting, from Chunky's `&` colour codes.
#[derive(Clone, Debug, Default, PartialEq)]
struct Segment {
    text: String,
    color: Option<&'static str>,
    bold: bool,
    italic: bool,
    underlined: bool,
    strikethrough: bool,
    obfuscated: bool,
}

/// What `&<code>` gives (Bukkit's `ChatColor`): `0`-`9`, `a`-`f` are colours.
fn legacy_color(c: char) -> Option<&'static str> {
    Some(match c.to_ascii_lowercase() {
        '0' => "#000000",
        '1' => "#0000AA",
        '2' => "#00AA00",
        '3' => "#00AAAA",
        '4' => "#AA0000",
        '5' => "#AA00AA",
        '6' => "#FFAA00",
        '7' => "#AAAAAA",
        '8' => "#555555",
        '9' => "#5555FF",
        'a' => "#55FF55",
        'b' => "#55FFFF",
        'c' => "#FF5555",
        'd' => "#FF55FF",
        'e' => "#FFFF55",
        'f' => "#FFFFFF",
        _ => return None,
    })
}

/// Splits a message at its `&` codes (`translateAlternateColorCodes('&', ...)`): a colour code sets the colour and
/// clears the formats, `&r` resets, `&k &l &m &n &o` add a format. An `&` before anything else stays as typed.
fn legacy_segments(line: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut cur = Segment::default();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        let code = if c == '&' { chars.peek().copied() } else { None };
        let Some(code) = code else {
            cur.text.push(c);
            continue;
        };
        let lower = code.to_ascii_lowercase();
        let known = legacy_color(code).is_some() || matches!(lower, 'r' | 'k' | 'l' | 'm' | 'n' | 'o');
        if !known {
            cur.text.push(c);
            continue;
        }
        chars.next();
        if !cur.text.is_empty() {
            out.push(cur.clone());
        }
        cur.text.clear();
        match (legacy_color(code), lower) {
            (Some(color), _) => cur = Segment { color: Some(color), ..Segment::default() },
            (None, 'r') => cur = Segment::default(),
            (None, 'k') => cur.obfuscated = true,
            (None, 'l') => cur.bold = true,
            (None, 'm') => cur.strikethrough = true,
            (None, 'n') => cur.underlined = true,
            _ => cur.italic = true,
        }
    }
    if !cur.text.is_empty() {
        out.push(cur);
    }
    out
}

/// A message with Chunky's `&` colour codes as Horizon text.
fn legacy_text(line: &str) -> Text {
    let mut text = Text::literal("");
    for seg in legacy_segments(&chunky::lang::localize(line)) {
        let mut style = TextStyle::default();
        style.color = seg.color.map(str::to_owned);
        style.bold = seg.bold.then_some(true);
        style.italic = seg.italic.then_some(true);
        style.underlined = seg.underlined.then_some(true);
        style.strikethrough = seg.strikethrough.then_some(true);
        style.obfuscated = seg.obfuscated.then_some(true);
        text = text.append(Text::literal(seg.text).with_style(style));
    }
    text
}

/// A message without its colour codes, for the console log.
fn strip_legacy(line: &str) -> String {
    legacy_segments(line).into_iter().map(|s| s.text).collect()
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
    eprintln!("[chunky] {}", strip_legacy(&chunky::lang::localize(line)));
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Tells the online player called `name` one line, as Chunky tells a task's owner.
fn tell_named(host: &mut dyn ServerHost, name: &str, line: &str) {
    let players = host.players().server_players();
    for player in players {
        if player.name(&*host).is_some_and(|n| n.eq_ignore_ascii_case(name)) {
            let _ = player.tell(host, legacy_text(line));
        }
    }
}

/// Chunky's Bukkit build sets `pause-when-empty-seconds` to 0 in server.properties (a paused server stops the tasks)
/// and asks for a restart. True when the file was changed; only that one line is rewritten.
fn disable_pause_when_empty() -> bool {
    let Ok(text) = std::fs::read_to_string("server.properties") else { return false };
    let mut changed = false;
    let mut out = String::new();
    for line in text.lines() {
        let is_setting = line.split_once('=').is_some_and(|(k, v)| {
            k.trim() == "pause-when-empty-seconds" && v.trim().parse::<i64>().is_ok_and(|n| n > 0)
        });
        if is_setting {
            out.push_str("pause-when-empty-seconds=0\n");
            changed = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !changed {
        return false;
    }
    let tmp = "server.properties.tmp";
    std::fs::write(tmp, out).is_ok() && std::fs::rename(tmp, "server.properties").is_ok()
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
        "chunky trim <world:chunky:world> <shape:chunky:shape> <x:word> <z:word> <rx:word> <rz:word> <side:chunky:side> [filters:chunky:filters]".into(),
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

/// The worlds tab completion offers: the three and the custom dimensions seen so far.
fn world_words() -> Vec<String> {
    available_worlds()
}

/// The trim filter words after the side: a greedy text that completes the filter keys not used yet (`inhabited=`,
/// `unmodified=`), as Chunky's `TrimFilters.suggestions` does. A bare first word counts as `inhabited`.
struct Filters;

impl CommandArgumentType for Filters {
    fn kind(&self) -> CommandArgumentKind {
        CommandArgumentKind::Greedy
    }

    fn parse(&self, input: &str) -> Result<String, CommandArgumentError> {
        Ok(input.to_owned())
    }

    fn suggest(&self, _: Option<&CommandSender>, prefix: &str) -> Vec<CommandSuggestion> {
        chunky::engine::trim_filter_suggestions(prefix)
            .into_iter()
            .map(|(text, start)| CommandSuggestion::new(text).replacing_from(start))
            .collect()
    }
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
        r.register_command_argument("filters", Filters);
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
        for usage in usages() {
            let sub = usage.split(' ').nth(1).filter(|w| !w.starts_with('<') && !w.starts_with('['));
            if sub.and_then(permission_node).is_some() {
                continue;
            }
            for label in ["chunky", "cy"] {
                event.commands.push(Command::op(&usage.replacen("chunky", label, 1), 2));
            }
        }
    }

    fn register_node_commands(&self, _: &mut dyn ServerHost, event: &mut RegisterNodeCommandsEvent<'_>) {
        // Each subcommand path carries Chunky's node (`chunky.command.start`), as Brigadier's requires on that
        // literal: a permissions mod can hand one subcommand to a non-operator; level 2 decides when none answers.
        // The bare `/chunky` (help) has no node, so it does not gate the subcommands.
        for usage in usages() {
            let sub = usage.split(' ').nth(1).filter(|w| !w.starts_with('<') && !w.starts_with('['));
            let Some(node) = sub.and_then(permission_node) else {
                continue;
            };
            // plugin.yml gives Chunky the alias `cy`: the same usages under that label.
            for label in ["chunky", "cy"] {
                event.commands.push(Command::op(&usage.replacen("chunky", label, 1), 2).with_node(&typed(&node)));
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

    /// A player without the node is answered as for an unknown command (Chunky's Brigadier tree hides it). The tree
    /// already leaves those usages out for them (`with_node`); this line answers what the tree no longer shows.
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
            let _ = host.commands().feedback(legacy_text(&l));
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
        if disable_pause_when_empty() {
            log(&chunky::lang::tr("error_pause_when_empty_seconds", &[]));
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
        register_dimension(dim);
        self.sweep_once(host, level, dim, controller);
        // Lines a background job (`debug map`) finished with, for whoever asked.
        for (sender, line) in chunky::with(|e| e.take_async()) {
            if sender.is_empty() {
                log(&line);
            } else {
                tell_named(host, &sender, &line);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_codes_split_a_message_into_styled_runs() {
        let segs = legacy_segments("&2chunky start&r - Start a task");
        assert_eq!(segs.len(), 2);
        assert_eq!((segs[0].text.as_str(), segs[0].color), ("chunky start", Some("#00AA00")));
        assert_eq!((segs[1].text.as_str(), segs[1].color), (" - Start a task", None));
        let warn = legacy_segments("[Chunky] &cWarning: a & b &z");
        assert_eq!(warn[0].text, "[Chunky] ");
        assert_eq!((warn[1].text.as_str(), warn[1].color), ("Warning: a & b &z", Some("#FF5555")));
        assert_eq!(strip_legacy("&aChunky Commands&r"), "Chunky Commands");
        let bold = legacy_segments("&c&lHi");
        assert!(bold[0].bold && bold[0].color == Some("#FF5555"));
    }
}
