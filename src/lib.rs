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
use crate::horizon_chunky_core::api;
use crate::horizon_chunky_core::border::{BorderEvent, Redirect, RedirectKind, BYPASS_BREAK, BYPASS_MOVE, BYPASS_PLACE};
use crate::horizon_chunky_core::engine::{Who, available_worlds, canon_world, dimension_of, display_world, register_dimension, set_level_name};
use mod_api::text::TextStyle;
use mod_api::{
    BreakBlockEvent, ChunkRef, Command, EntityRef, EnderPearlTeleportDetailEvent, EntityPlaceEvent, FinalizeSpawnEvent, Heightmap,
    ItemConsumptionTeleportEvent, ModServiceCallEvent, ModServiceError, ParticleKey, ParticleOptions, PlayerLoggedOutEvent,
    PlayerRef, PlayerRespawnedEvent, PlayerTeleportEndGatewayDetailEvent, CommandArgumentError, CommandArgumentKind, CommandArgumentType,
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
    /// Ticks seen per dimension, for the border check's interval (`check-interval`) and the visualizer's phase.
    border_ticks: Mutex<HashMap<String, u64>>,
    /// The particle types the border's effect and visualizer use, by short name (`portal`, `dust`), from `register`.
    particles: Mutex<HashMap<&'static str, ParticleKey>>,
    /// The tridents a player threw, each with the last position inside the border (`prevent-trident`).
    tridents: Mutex<Vec<(EntityRef, [f64; 3])>>,
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
        "chunky border",
        "chunky border help",
        "chunky border list",
        "chunky border add",
        "chunky border add <world:chunky:world>",
        "chunky border bypass",
        "chunky border bypass <player:word>",
        "chunky border load",
        "chunky border load <world:chunky:world>",
        "chunky border remove",
        "chunky border remove <world:chunky:world>",
        "chunky border wrap",
        "chunky border wrap <wrap:chunky:wrap>",
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
    usages.push(
        "chunky border add <world:chunky:world> <shape:chunky:shape> [x:word] [z:word] [rx:word] [rz:word]".into(),
    );
    usages
        .push("chunky trim <world:chunky:world> <shape:chunky:shape> [x:word] [z:word] [rx:word] [rz:word]".into());
    usages.push(
        "chunky trim <world:chunky:world> <shape:chunky:shape> <x:word> <z:word> <rx:word> <rz:word> <side:chunky:side> [filters:chunky:filters]".into(),
    );
    usages
}

/// Every subcommand word.
const SUBS: [&str; 23] = [
    "border",
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

fn wrap_words() -> Vec<String> {
    crate::horizon_chunky_core::border::Wrap::ALL.iter().map(|w| w.name().to_owned()).collect()
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

/// The border key (`world`, `world_nether`, `world_the_end`, a custom dimension's id) and the dimension id of the
/// level a player is in.
fn player_world(host: &dyn ServerHost, player: PlayerRef) -> Option<(&'static str, String)> {
    let dim = player.dimension(host)?.to_string();
    canon_world(&dim).map(|w| (w, dim))
}

/// The surface at the column of `at` in `dim` (the first air above the ground), None when it is not loaded.
fn ground(host: &dyn ServerHost, dim: &str, at: [f64; 3]) -> Option<f64> {
    let column = (at[0].floor() as i32, at[2].floor() as i32);
    let heights = host.surface_heights(dim, &[column], Heightmap::WorldSurface).ok()?;
    heights.first().copied().flatten().map(f64::from)
}

/// The level's spawn as a position (`World.getSpawn`).
fn spawn_of(host: &dyn ServerHost, level: LevelRef) -> Option<[f64; 3]> {
    level.respawn_data(host).map(|r| [f64::from(r.pos[0]) + 0.5, f64::from(r.pos[1]), f64::from(r.pos[2]) + 0.5])
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
        r.register_command_argument("wrap", Words(wrap_words));
        // The border's effect and visualizer particles.
        let mut particles = lock(&self.particles);
        for name in ["portal", "dust", "smoke", "flame", "effect"] {
            particles.insert(name, r.particle_key(&format!("minecraft:{name}")));
        }
    }

    fn permission_gather_nodes(&self, event: &mut PermissionGatherNodesEvent<'_>) {
        // plugin.yml: the root node and one per subcommand, each defaulting to operator level 2 (`LEVEL`); holding a
        // subcommand's node grants the root (`children: [chunky.command]`).
        let level = NodeDefault::Level(PermissionLevel::from_level(i32::from(LEVEL)));
        let _ = event.nodes.add(typed(ROOT_NODE), level.clone());
        for sub in SUBS {
            let _ = event.nodes.add_with_children(typed(&format!("chunky.command.{sub}")), level.clone(), &[typed(ROOT_NODE)]);
        }
        // `chunky.chunky` (default: nobody) grants the root and every subcommand node but trim (plugin.yml).
        let children: Vec<PermissionNode> = std::iter::once(ROOT_NODE.to_string())
            .chain(SUBS.iter().filter(|s| !matches!(**s, "trim" | "border")).map(|s| format!("chunky.command.{s}")))
            .map(|n| typed(&n))
            .collect();
        let _ = event.nodes.add_with_children(typed("chunky.chunky"), NodeDefault::Nobody, &children);
        // ChunkyBorder's plugin.yml: bypass nodes nobody has by default.
        for node in [BYPASS_MOVE, BYPASS_PLACE, BYPASS_BREAK] {
            let _ = event.nodes.add(typed(node), NodeDefault::Nobody);
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
        // `border bypass <name>` finds the player among those online.
        let online: Vec<(String, u128)> = host
            .players()
            .server_players()
            .into_iter()
            .filter_map(|p| Some((p.name(&*host)?, p.uuid())))
            .collect();
        chunky::with(|e| e.borders.set_online(&online));
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
        // With `delete_chunks` a confirmed trim starts at once instead of at the next start.
        chunky::with(|e| e.set_live_trim(true));
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
        // The border: players outside it are sent back, the visualizer draws it, other mods hear what happened.
        self.border_tick(host, level, dim);
        self.deliver_events(host);
        // A trim that runs while the server is up: the chunks go a few per tick.
        if chunky::with(|e| e.live_trim_active(dim)) {
            self.diff_loaded(&*host, level, dim);
            self.live_trim_step(host, level, dim);
        }
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

    /// The services other mods call: `chunky:api` (`horizon_chunky_core::api`).
    fn mod_service_call(&self, _: &mut dyn ServerHost, event: &mut ModServiceCallEvent<'_>) -> Option<mod_api::PayloadBytes> {
        if event.service != "chunky:api" {
            return None;
        }
        let text = std::str::from_utf8(&event.payload.0).ok()?;
        chunky::with(|e| api::handle(e, text)).map(|reply| mod_api::PayloadBytes(reply.into_bytes()))
    }

    /// ChunkyBorder's `BlockBreakEvent`: no breaking outside the border without `chunkyborder.bypass.break`.
    fn break_block(&self, host: &mut dyn ServerHost, event: &mut BreakBlockEvent<'_>) -> Flow {
        let Some(player) = host.players().acting_player() else { return Flow::Continue };
        self.guard_block(host, player, event.pos, BYPASS_BREAK)
    }

    /// ChunkyBorder's `BlockPlaceEvent`: no placing outside the border without `chunkyborder.bypass.place`.
    fn entity_place(&self, host: &mut dyn ServerHost, event: &mut EntityPlaceEvent) -> Flow {
        let Some(placer) = event.placer else { return Flow::Continue };
        let Some(player) = host.players().server_players().into_iter().find(|p| p.entity() == placer) else {
            return Flow::Continue;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        self.guard_block(host, player, pos, BYPASS_PLACE)
    }

    /// `prevent-mob-spawns`: nothing spawns outside the border.
    fn finalize_spawn(&self, _: &dyn mod_api::Host, event: &mut FinalizeSpawnEvent<'_>) -> Flow {
        if !chunky::with(|e| e.borders.config.prevent_mob_spawns) {
            return Flow::Continue;
        }
        let Some(world) = canon_world(event.dimension) else { return Flow::Continue };
        if chunky::with(|e| e.borders.spawn_blocked(world, event.pos[0], event.pos[2])) {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    /// An ender pearl that lands outside the border: cancelled with `prevent-enderpearl`, else landing inside it.
    fn ender_pearl_teleport_detail(&self, host: &mut dyn ServerHost, event: &mut EnderPearlTeleportDetailEvent<'_>) -> Flow {
        let owner = event.owner;
        let Some(player) = host.players().server_players().into_iter().find(|p| p.entity() == owner) else {
            return Flow::Continue;
        };
        let prevent = chunky::with(|e| e.borders.config.prevent_enderpearl);
        self.pull_in(host, player, &mut *event.target, prevent, RedirectKind::Teleport)
    }

    /// Chorus fruit, as the ender pearl (`prevent-chorus-fruit`).
    fn item_consumption_teleport(&self, host: &mut dyn ServerHost, event: &mut ItemConsumptionTeleportEvent<'_>) -> Flow {
        let consumer = event.consumer.entity();
        let Some(player) = host.players().server_players().into_iter().find(|p| p.entity() == consumer) else {
            return Flow::Continue;
        };
        let prevent = chunky::with(|e| e.borders.config.prevent_chorus_fruit);
        self.pull_in(host, player, &mut *event.target, prevent, RedirectKind::Teleport)
    }

    /// An end gateway whose exit lies outside the border: cancelled with `prevent-end-gateway`. The exit cannot be
    /// moved inside (the landing is read-only), so without the option the player lands where the gateway says.
    fn player_teleport_end_gateway_detail(&self, host: &mut dyn ServerHost, event: &mut PlayerTeleportEndGatewayDetailEvent<'_>) -> Flow {
        if !chunky::with(|e| e.borders.config.prevent_end_gateway) {
            return Flow::Continue;
        }
        let Some(player) = host.player(event.player) else { return Flow::Continue };
        let Some((world, _)) = player_world(&*host, player) else { return Flow::Continue };
        let landing = event.landing;
        if chunky::with(|e| e.borders.inside(world, landing[0], landing[2])) {
            return Flow::Continue;
        }
        let bypass = player.permission_bool(host, &typed(BYPASS_MOVE)).unwrap_or(false);
        if bypass || chunky::with(|e| e.borders.is_bypassing(event.player)) {
            Flow::Continue
        } else {
            Flow::Cancel
        }
    }

    /// `PlayerRespawnEvent`: a respawn outside the border moves inside it.
    fn player_respawned(&self, host: &mut dyn ServerHost, event: &mut PlayerRespawnedEvent<'_>) {
        let Some(player) = host.player(event.player) else { return };
        let entity = player.entity();
        let Some(at) = entity.position(&*host) else { return };
        let mut to = at;
        let _ = self.pull_in(host, player, &mut to, false, RedirectKind::Respawn);
        if to != at {
            let _ = entity.teleport_to(host, to, None);
        }
    }

    /// `PlayerQuitEvent`: the player's border data goes.
    fn player_logged_out(&self, _: &mut dyn ServerHost, event: &mut PlayerLoggedOutEvent<'_>) {
        chunky::with(|e| e.borders.forget(event.player));
    }

    fn level_tick_post(&self, host: HostSide, _: &mut LevelTickPostEvent) {
        if matches!(host, HostSide::Server(_)) {
            chunky::level_end();
        }
    }
}

impl Mod {
    /// `BlockPlaceEvent` / `BlockBreakEvent` of ChunkyBorder: cancelled when the block lies outside the border and the
    /// player does not hold the bypass node.
    fn guard_block(&self, host: &mut dyn ServerHost, player: PlayerRef, pos: [i32; 3], node: &str) -> Flow {
        let Some((world, _)) = player_world(&*host, player) else { return Flow::Continue };
        if !chunky::with(|e| e.borders.block_blocked(world, pos, false)) {
            return Flow::Continue;
        }
        if player.permission_bool(host, &typed(node)).unwrap_or(false) {
            Flow::Continue
        } else {
            Flow::Cancel
        }
    }

    /// `ChunkyBorder`'s teleport handler for a player whose destination `target` is outside the border: cancelled when
    /// `prevent`, else moved to just inside the nearest edge (or the world spawn).
    fn pull_in(&self, host: &mut dyn ServerHost, player: PlayerRef, target: &mut [f64; 3], prevent: bool, kind: RedirectKind) -> Flow {
        let Some((world, dim)) = player_world(&*host, player) else { return Flow::Continue };
        if chunky::with(|e| e.borders.get(world).is_none()) {
            return Flow::Continue;
        }
        let bypass = player.permission_bool(host, &typed(BYPASS_MOVE)).unwrap_or(false);
        let answer = chunky::with(|e| e.borders.teleport_redirect(player.uuid(), world, *target, bypass, kind));
        match answer {
            None => Flow::Continue,
            Some(_) if prevent => Flow::Cancel,
            Some(Some(r)) => {
                let mut to = r.to;
                if let Some(y) = ground(&*host, &dim, to) {
                    to[1] = y;
                }
                *target = to;
                Flow::Continue
            }
            Some(None) => {
                if let Some(spawn) = host.level(dimension_of(world)).and_then(|l| spawn_of(&*host, l)) {
                    *target = spawn;
                }
                Flow::Continue
            }
        }
    }

    /// `BorderCheckTask`: every `check-interval` ticks each player of this level outside its border goes back (or
    /// wraps); the visualizer draws the wall every tick.
    fn border_tick(&self, host: &mut dyn ServerHost, level: LevelRef, dim: &str) {
        let Some(world) = canon_world(dim) else { return };
        let (interval, bordered, visualizer, trident) = chunky::with(|e| {
            let c = &e.borders.config;
            (c.check_interval.max(1), e.borders.get(world).is_some(), c.visualizer_enabled, c.prevent_trident)
        });
        if !bordered {
            return;
        }
        let tick = {
            let mut ticks = lock(&self.border_ticks);
            let n = ticks.entry(dim.to_owned()).or_insert(0);
            *n += 1;
            *n
        };
        if visualizer {
            self.visualize(host, level, dim, world, tick);
        }
        if trident {
            self.guard_tridents(host, level, dim, world);
        }
        if tick % interval != 0 {
            return;
        }
        for player in host.players().server_players() {
            if player.dimension(&*host).map(|d| d.to_string()).as_deref() != Some(dim) {
                continue;
            }
            let entity = player.entity();
            let Some(pos) = entity.position(&*host) else { continue };
            let rot = entity.rotation(&*host).unwrap_or([0.0, 0.0]);
            let bypass = player.permission_bool(host, &typed(BYPASS_MOVE)).unwrap_or(false);
            let spawn = spawn_of(&*host, level);
            let Some(redirect) = chunky::with(|e| e.borders.check(player.uuid(), world, pos, rot, spawn, bypass)) else {
                continue;
            };
            self.send_back(host, level, dim, player, pos, redirect);
        }
    }

    /// `prevent-trident` (`onProjectileLaunch`): a trident a player threw stops at the border. Past it the trident's
    /// velocity is zeroed and it goes back to its last position inside; one that would cross on its next tick loses its
    /// velocity. Tridents are found in a box of 128 blocks around their thrower.
    fn guard_tridents(&self, host: &mut dyn ServerHost, level: LevelRef, dim: &str, world: &str) {
        let mut seen: Vec<(EntityRef, [f64; 3])> = Vec::new();
        for player in host.players().server_players() {
            if player.dimension(&*host).map(|d| d.to_string()).as_deref() != Some(dim) {
                continue;
            }
            if chunky::with(|e| e.borders.is_bypassing(player.uuid()))
                || player.permission_bool(host, &typed(BYPASS_MOVE)).unwrap_or(false)
            {
                continue;
            }
            let owner = player.entity();
            let Some(at) = owner.position(&*host) else { continue };
            let (min, max) = ([at[0] - 128.0, at[1] - 128.0, at[2] - 128.0], [at[0] + 128.0, at[1] + 128.0, at[2] + 128.0]);
            let Some(found) = level.entities_in(&*host, min, max) else { continue };
            for thrown in found {
                let is_trident = thrown.kind(&*host).is_some_and(|k| k.id(&*host).to_string().rsplit(':').next() == Some("trident"));
                if !is_trident {
                    continue;
                }
                let Some(projectile) = host.projectile(thrown) else { continue };
                if projectile.owner(&*host) != Some(owner) {
                    continue;
                }
                let Some(pos) = thrown.position(&*host) else { continue };
                let previous = lock(&self.tridents).iter().find(|(t, _)| *t == thrown).map(|(_, p)| *p);
                let inside = |p: [f64; 3]| chunky::with(|e| e.borders.inside(world, p[0], p[2]));
                if !inside(pos) {
                    let _ = thrown.set_velocity(host, [0.0, 0.0, 0.0]);
                    if let Some(back) = previous {
                        let _ = thrown.teleport_to(host, back, None);
                        seen.push((thrown, back));
                    }
                    continue;
                }
                let v = thrown.velocity(&*host).unwrap_or([0.0, 0.0, 0.0]);
                if !inside([pos[0] + v[0], pos[1] + v[1], pos[2] + v[2]]) {
                    let _ = thrown.set_velocity(host, [0.0, 0.0, 0.0]);
                }
                seen.push((thrown, pos));
            }
        }
        *lock(&self.tridents) = seen;
    }

    /// The effect, the sound, the teleport and the message of a redirected player (`BorderCheckTask.check`).
    fn send_back(&self, host: &mut dyn ServerHost, level: LevelRef, dim: &str, player: PlayerRef, from: [f64; 3], mut redirect: Redirect) {
        if redirect.needs_ground {
            if let Some(y) = ground(&*host, dim, redirect.to) {
                redirect.to[1] = y;
                chunky::with(|e| e.borders.set_last(player.uuid(), canon_world(dim).unwrap_or("world"), redirect.to));
            }
        }
        let (effect, sound, message, action_bar) = chunky::with(|e| {
            let c = &e.borders.config;
            (c.effect.clone(), c.sound_key(), c.message.clone(), c.use_action_bar)
        });
        let entity = player.entity();
        self.play_effect(host, level, &effect, from);
        if let Some(sound) = sound {
            let _ = entity.play_sound(host, &sound, 1.0, 1.0);
        }
        let _ = entity.teleport_to(host, redirect.to, Some([redirect.yaw, redirect.pitch]));
        if !message.trim().is_empty() {
            let text = legacy_text(&message);
            if action_bar {
                let _ = player.action_bar(host, text);
            } else {
                let _ = player.tell(host, text);
            }
        }
    }

    /// The config's `effect` (a Bukkit effect name) as particles at the old position; the names this knows are the
    /// ones with a plain particle of their own.
    fn play_effect(&self, host: &mut dyn ServerHost, level: LevelRef, effect: &str, at: [f64; 3]) {
        let name = match effect.to_ascii_lowercase().as_str() {
            "ender_signal" => "portal",
            "smoke" => "smoke",
            "mobspawner_flames" => "flame",
            "potion_break" => "effect",
            _ => return,
        };
        let Some(key) = lock(&self.particles).get(name).copied() else { return };
        let _ = level.send_particles(host, &ParticleOptions::simple(key), None, [at[0], at[1] + 1.0, at[2]], [0.3, 0.5, 0.3], 0.1, 24);
    }

    /// ChunkyBorder's visualizer: dust along the wall near each player, moving a twentieth of a block a tick. The
    /// particles go to everyone in range, not only to the player the wall is drawn for.
    fn visualize(&self, host: &mut dyn ServerHost, level: LevelRef, dim: &str, world: &str, tick: u64) {
        let (range, color, border) = chunky::with(|e| {
            (e.borders.config.visualizer_range.max(1) as f64, e.borders.config.visualizer_color.clone(), e.borders.get(world).cloned())
        });
        let Some(border) = border else { return };
        let Some(dust) = lock(&self.particles).get("dust").copied() else { return };
        let rgb = u32::from_str_radix(color.trim_start_matches('#'), 16).unwrap_or(0x20A0FF);
        let mut options = ParticleOptions::simple(dust);
        options.color = [((rgb >> 16) & 0xFF) as f32 / 255.0, ((rgb >> 8) & 0xFF) as f32 / 255.0, (rgb & 0xFF) as f32 / 255.0, 1.0];
        options.scale = 1.0;
        let offset = (tick % 20) as f64 / 20.0;
        for player in host.players().server_players() {
            if player.dimension(&*host).map(|d| d.to_string()).as_deref() != Some(dim) {
                continue;
            }
            let Some(pos) = player.entity().position(&*host) else { continue };
            let points = border.visualizer_points(pos, range, offset);
            if !points.is_empty() {
                let _ = level.send_particles_at(host, &options, None, &points, [0.0, 0.0, 0.0], 0.0, 1);
            }
        }
    }

    /// Tells the mods that subscribed (`api`) what Chunky and the border did since the last tick: each gets one call
    /// of its service `<id>:chunky_event` per event. A mod that is gone is dropped from the list.
    fn deliver_events(&self, host: &mut dyn ServerHost) {
        let lines: Vec<String> = chunky::with(|e| {
            let mut lines = e.take_api_events();
            for event in e.borders.take_events() {
                lines.push(match event {
                    BorderEvent::Change { world, border } => api::border_change_event(&world, border.as_ref()),
                    BorderEvent::Redirect { player, world, from, to, kind } => {
                        api::border_redirect_event(player, &world, from, to, kind)
                    }
                });
            }
            lines
        });
        if lines.is_empty() {
            return;
        }
        for id in chunky::with(|e| e.subscribers().to_vec()) {
            let service = format!("{id}:chunky_event");
            for line in &lines {
                if let Err(ModServiceError::ProviderNotLoaded) = host.commands().call_mod_service(&service, line.as_bytes()) {
                    chunky::with(|e| e.unsubscribe(&id));
                    break;
                }
            }
        }
    }

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

    /// Erases the next chunks of the dimension's live trim (`LevelRef::delete_chunks`).
    fn live_trim_step(&self, host: &mut dyn ServerHost, level: LevelRef, dim: &str) {
        let batch = chunky::with(|e| e.take_live_trim_batch(dim, chunky::now_ms()));
        if batch.is_empty() {
            return;
        }
        let erased = level.delete_chunks(host, &batch);
        chunky::with(|e| e.live_trim_done(dim, &batch, erased));
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
