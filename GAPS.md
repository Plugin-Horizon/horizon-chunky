# Gaps

What still differs from the Paper plugin Chunky (checked against its source in round 3, GPL-3.0, commit c4274bd).

## Not ported

1. **`chunky limit <rate>`**: Chunky's per-task chunk rate limit. Answers "Not ported yet".
2. **`chunky debug map`**: Chunky's generation map as a PNG. Answers "Not ported yet".
3. **`border` subcommands** (`border add|remove|wrap|list|load|bypass`): they need Chunky's ChunkyBorder integration,
   which Horizon does not have. Not in the command tree, not in `help`.
4. **Trim filter `unmodified=<n><s|m|h|d>`** (region-file timestamps) and **inhabited ranges** `inhabited=<min>..<max>`.
   The bare `inhabited` maximum and `inhabited=<max>` work, with `s`, `m`, `h`, `d` units.
5. **`world` pattern details.** Chunky's iterator treats partly generated chunks specially (a flood fill from the
   unfinished edge over the region files). The port takes the chunks the save lists, inside the selection's box, and
   does not follow Chunky's flood fill. Read Chunky's `WorldChunkIterator` before relying on the exact set.
6. **`language` config key.** Messages are English only.
7. **Colours.** Chunky's `&` codes are left out (plain text). Nothing is coloured.
8. **`quiet 0`.** Chunky prints a line for every generated chunk; the port prints at most one line per level tick.

## Differences by design

9. **Trim runs at the next start**, not on the live server (see README). `chunky trim abort` is a port addition; Chunky's
   `cancel` also stops a trim, here it does not touch a scheduled trim file.
10. **One pending action per server**, not per sender (`confirm` answers the last request).
11. **Config keys** `max-in-flight`, `tick-budget-ms`, `chunk-timeout-ticks` are the port's own engine tuning.
12. **Pausing a task you started in this session** asks for `confirm` before a new start, as for a saved task; Chunky
    says "already started" for a paused task still in its map.
13. **`csv <name>` file folder**: Chunky reads `<name>.csv` from its data folder first, then its config folder; the port
    reads `config/chunky/<name>.csv` only.
14. **`spawn` in the Nether and End** centres on 0, 0 (Chunky uses that world's spawn; the save does not give one).
15. **`worldborder`** reads the border on the level's next tick and prints one line (centre and radius).
16. **Hilbert and ring orders** are checked by simulating Chunky's iterators, not in an engine.

## Permissions

Each subcommand has its node `chunky.command.<sub>` (from Chunky's `plugin.yml`, including `confirm` and `help`), wired
through `native_has_permission` (`mod/glue.rs`); operators (level 2) pass when no permissions mod answers. Not yet checked
in an engine.
