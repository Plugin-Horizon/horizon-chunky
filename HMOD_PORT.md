# hmod port: chunky
Parity: 98% now / 98% with our PRs / 98% with engine changes / 98% with new SDK (59/60 items; checklist round 100 below; the earlier 122/130 count used the audit's list and is not comparable)
Steps: no step moves an item. Chunky has no open upstream PR in HOOKS_DONE.md, and live trim has no planned engine group (its line is in GAPS_UPSTREAM.md), so the four values stay equal. Border subcommands (ChunkyBorder add-on) are not counted: Chunky 26.3's source registers `border` only when an add-on puts it into the command map, and no add-on is installed here.
Round 4 (blind, no compile)
- Added: `language` (config.toml) and the 28 message files Chunky ships (`lang/*.json`, copied from pop4959/Chunky, GPL-3.0). `src/horizon_chunky_core/lang.rs`: `localize` turns each printed line into the chosen language by matching it against `lang/en.json` (most literal text wins) and filling the same `%s`/`%n$s` slots into the translation; shape, pattern and on/off words, also inside lists, are translated; `tr` renders a message directly (the unmodified notice's time unit). `lib.rs` `log` and `legacy_text` localize every line. Unknown codes give English (Chunky's `checkLanguage`); `chunky reload` re-reads the language.
- Added: custom dimensions. A dimension the server loads (any `ns:path` other than the vanilla three) is a world Chunky can name by its id: `register_dimension` runs on each level tick and for saved tasks, `canon_world`/`dimension_of`/`display_world`/`dimension_dir` (`dimensions/<ns>/<path>`) know it, `available_worlds` feeds the tab completion and the "World not found" line.
- Added: trim filter tab completion (`chunky:filters`, a greedy argument; `engine::trim_filter_suggestions` offers `inhabited=`/`unmodified=` not used yet, with the replacement start, as `TrimFilters.suggestions`).
- Added: `pause-when-empty-seconds` handling (`lib.rs` `disable_pause_when_empty`): a value above 0 in server.properties is set to 0 (only that line rewritten, via a temp file) and the English message is logged, as Chunky's Bukkit build does at enable. The file changes at start; the server reads it on its next start.
- Fixed: messages named the canonical world (`world_nether`) instead of the Bukkit name (`<level>_nether`); now `display_world` everywhere a world name is printed (task lines, trim warning, pending trim, rate limit, debug map, not-found list).
- Fixed: `chunky spawn` centres on the selected world's spawn for every world (`Selection.spawn`), not 0, 0 outside the Overworld. The glue already reads that spawn (`lib.rs` `run_mod_command`).
- Unit tests (literal values): `lang.rs` (JSON escapes, slots and `%%`, positional slots, matching, German lines with translated words), engine: custom dimension names and paths, filter completion offsets, the unmodified notice in English.
- Left open: live trim (`LevelRef::delete_chunks`, not in the pinned SDK; GAPS_UPSTREAM.md line for chunky). The `border` subcommands (ChunkyBorder add-on): not Chunky's own, not counted.
- Unsure: the Java-to-Rust translation of the bundled JSON (surrogate pairs, `%n$s` in de/hi/bg/nl/es/ja/tr/pt/zh files), the `capture` matcher on lines that contain a slot's text as fixed text, and `server.properties` rewrite at start.

Checklist round 100 (original = Chunky b8431a9; [x] = done in the port, [ ] = open, n/a = not counted, reason in brackets)
Commands (`BrigadierCommand`, `CommandLiteral`):
- [x] 1 `chunky` alone: help
- [x] 2 help [page]
- [x] 3 start [world] [shape] [x z] [rx] [rz]
- [x] 4 pause [world]
- [x] 5 continue [world]
- [x] 6 cancel [world] (+ confirm)
- [x] 7 confirm
- [x] 8 progress
- [x] 9 world [world]
- [x] 10 worldborder [world]
- [x] 11 center [x z]
- [x] 12 spawn
- [x] 13 radius <r> [rz]
- [x] 14 corners <x1> <z1> <x2> <z2>
- [x] 15 shape <shape>
- [x] 16 pattern <pattern> [csv name]
- [x] 17 silent
- [x] 18 quiet <interval>
- [x] 19 selection
- [x] 20 limit <rate>
- [x] 21 debug map
- [x] 22 reload [config|tasks]
- [x] 23 trim [world] [shape] [x z] [rx] [rz] [inside|outside] [filters]
- [x] 24 alias `cy`
- [n/a] 25 border add/remove/wrap/list/load/bypass/help [ChunkyBorder add-on; not in Chunky, not installed]
Permissions:
- [x] 26 chunky.command root and one node per subcommand, operator level 2 (round 1); hidden from players without the node (round 3)
Tab completion (`CommandSuggestions`, `WorldSuggestions`, `*SuggestionProvider`):
- [x] 27 world names, including custom dimensions
- [x] 28 shapes
- [x] 29 patterns
- [x] 30 reload config|tasks
- [x] 31 trim side inside|outside
- [x] 32 trim filter keys (round 4)
Behaviour:
- [x] 33 inhabited filter (max, min, range; s/m/h/d)
- [x] 34 unmodified filter (region file times)
- [x] 35 trim notices and the confirm text
- [x] 36 pending actions per sender, 60 s
- [x] 37 start over a saved task needs confirm; cancel needs confirm
- [x] 38 task owner told the completion line; console "Task stopped" on pause/cancel
- [x] 39 rate limit (limit, saved with the task)
- [x] 40 radius-limit host cap (.chunky.properties)
- [n/a] 41 disk-space estimate before start (`format_start_disk`: the original never prints it)
- [x] 42 pause-when-empty-seconds disabled and logged (round 4)
- [x] 43 debug map PNG
- [x] 44 worldborder reads the border, answers centre, radius and max-world-size notice
- [x] 45 continue-on-restart, force-load-existing-chunks, saved task files, existing-chunk skipping
- [x] 46 patterns ring, concentric, spiral, loop, strip, region, world, csv (8 with the shape walk)
- [x] 47 nine shapes
- [x] 48 spawn of the selected world (round 4)
- [x] 49 world names in messages as Bukkit names (round 4)
- [x] 50 custom dimensions (round 4)
- [ ] 51 live trim (round 2: written behind cargo feature `next-engine`, needs `delete_chunks` of branch rec-world-data; default build trims at the next start)
Config keys (`config.yml`):
- [n/a] 52 version (Chunky reads it for its own migration; the port has no migration to do)
- [x] 53 language (round 4)
- [x] 54 continue-on-restart
- [x] 55 force-load-existing-chunks
- [x] 56 silent
- [x] 57 update-interval
Messages:
- [x] 58 English texts and the help menu (`help_*`, `help_menu`, `help_more`)
- [x] 59 colour codes (`&`) as text styles
- [x] 60 translations: 28 files, placeholders %s and %n$s (round 4, untested)
- [x] 61 translated words in messages (shape, pattern, enabled, disabled; time units in the unmodified notice)
- [x] 62 permission text for a player without the node (hidden command, round 3)
- [x] 63 the console lines (task progress, task stopped, gave up waiting)
Counted: 60 items (59 done, 1 open: live trim; 3 n/a: items 25, 41, 52). Port-only, not counted: deferred trim with its preview lines, tick budget and ticket keys in config.toml, `chunky trim abort`.

Round 3 (blind, no compile)
- Fixed: colours. Messages carry Chunky's `&` codes (help usage green, `&aChunky Commands&r`, trim warning `&c`, trim notices `&e`, world-border notice `&e`, debug failure `&c`); `lib.rs` `legacy_text` turns them into styled text (`#RRGGBB` colour per code), the console log strips them.
- Fixed: `chunky start` refuses a radius above `radius-limit` from `config/chunky/.chunky.properties` with `format_start_limit` (Chunky's host limit file).
- Added: `chunky debug map` (`genmap.rs`): scans the selection's region files in a background thread, counts full / unlit-or-unsaved / incomplete / invalid / not generated chunks as Chunky's `GenerationMap`, writes `config/chunky/debug/<world>.png` (8-bit palette PNG, several chunks per pixel above 4096) and reports through `Engine::take_async` at the next level tick.
- Added: trim filters `inhabited=<min>..<max>` (either end optional, units s/m/h/d), `unmodified=<amount><unit>` (region file timestamps; chunks without one stay) and the `format_trim_confirm_inhabited*` / `_unmodified` notices; filters parse like `TrimFilters.parse` (bare first word is inhabited, duplicates and unknown keys name the bad word). The pending trim file carries `inhabited-min` and `unmodified-cutoff` (older files still read).
- Fixed: a player without the node is answered like an unknown command (Chunky's Brigadier tree hides it) instead of Bukkit's permission text.
- Dropped: the disk-space estimate (`format_start_disk`, `Disk`): the message key exists in the original but nothing calls it.
- Left open: `chunky border` (the separate ChunkyBorder add-on, not Chunky), live trim (deletes at the next start, needs a region writer while the world is open), custom dimensions, `language` and translations, tab completion of trim filters.
- Unsure: PNG header/chunk writing and `&`-code styling are untested; `isLightOn` is read as a byte tag and `xPos`/`zPos` as ints by a name search in the chunk NBT.
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
