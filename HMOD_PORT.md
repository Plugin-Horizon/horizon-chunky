# hmod port: chunky
Round 1
- Fixed: src/lib.rs `register_commands`: `with_node` takes `&PermissionNode`; the node name is wrapped with the new `typed()` helper.
- Fixed: src/lib.rs `player_commands_sent` (tab completion): `permission_value` == true replaced by typed `permission_bool` queries on `chunky.command` and each `chunky.command.<sub>`. The query needs `&mut` host, so the hook's `host` is used directly. Any error is a denial.
- Fixed: src/lib.rs `player_command`: `permission_value` replaced by `permission_bool` on the subcommand's node (or the root node); any error is a denial.
- Fixed: src/lib.rs `Mod::permission_gather_nodes` (new): declares `chunky.command` and `chunky.command.<sub>` for each of the 22 entries of `SUBS`.
- Permission nodes: `chunky.command` and `chunky.command.<sub>` (22 subcommands) -> `Level(2)`, declared in `Mod::permission_gather_nodes` (src/lib.rs).
- Gaps: none
- Unsure:
  - `player_command` no longer falls back to the operator level when the query fails; the declared default (level 2) gives operators the same answer. The spec's "any error is a denial" was applied.
  - `SUBS` names are assumed to pass the node name rules (lowercase letters only).

Audit fixes
- Fixed: `chunky worldborder` answers the sender at once with Chunky's centre/radius lines and the max-world-size notice (read in `run_mod_command`, no log-only result).
- Fixed: a task's owner (the player who started or continued it) is told the completion line; the console gets `Task stopped for X.` on pause and cancel; `confirm` actions are kept per sender.
- Fixed: `chunky limit <rate>` (rate limiter, saved as `rate-limit` with `owner` in the task file), `limit` row in the help menu, bare `chunky pattern csv` prints the pattern help, `GAPS.md` references now point to the README.
- Fixed: tab completion for world, shape and trim-side arguments through registered argument types `chunky:world`, `chunky:shape`, `chunky:side` (they accept any word; the engine keeps Chunky's own "not found" lines); `chunky spawn` uses the selected world's spawn.
- Gaps: see the README's Known gaps (colours, radius/disk checks on start, `debug map`, trim filter `unmodified`, live trim).
- Unsure: the argument kind ids in usages (`<world:chunky:world>`) and `CommandArgumentValue::Custom { value, .. }` are written from the docs, not compiled.
