// Chunky for Horizon: the game side of the permission nodes. A support module in `game.rs` (so it reaches the game's
// `native_has_permission`); the hooks' helpers only call into it.
use super::Game;
use crate::game::native_server::NativeCommand;

/// The permission node of a subcommand, `chunky.command.<sub>` (Paper's `plugin.yml` node per command). None for the
/// bare label and for a word that is no subcommand, which only operators' level decides.
pub(super) fn node_of(sub: Option<&str>) -> Option<String> {
    const SUBS: [&str; 22] = [
        "cancel", "center", "confirm", "continue", "corners", "debug", "help", "limit", "pattern", "pause",
        "progress", "quiet", "radius", "reload", "selection", "shape", "silent", "spawn", "start", "trim", "world",
        "worldborder",
    ];
    let sub = sub?.to_ascii_lowercase();
    SUBS.contains(&sub.as_str()).then(|| format!("chunky.command.{sub}"))
}

/// Whether a player may run a `chunky ...` line: the node the line's subcommand needs, which a permissions mod may
/// grant to anyone; without one the operator level 2 decides, as before. `player_level` is the player's own level.
pub(super) fn may_run(game: &Game<'_>, uuid: u128, player_level: u8, line: &str) -> bool {
    match node_of(line.split_whitespace().nth(1)) {
        Some(node) => game.native_has_permission(uuid, &node, 2),
        None => player_level >= 2,
    }
}

/// The command for one usage, with its node when it has one (the usage's second word is the subcommand).
pub(super) fn command_for(usage: &str) -> NativeCommand {
    let command = NativeCommand::op(usage, 2);
    match node_of(usage.split_whitespace().nth(1)) {
        Some(node) => command.permission(&node),
        None => command,
    }
}
