# horizon-chunky

## Port status

Compared with Chunky b8431a9 (pop4959): **93 %** of the original's commands, permissions, config keys, messages and features work today; **93 %** once our open Horizon PRs are merged; **93 %** with the engine changes we are writing; **93 %** with the newest Horizon SDK. Counted on 26/28 items (see audit item list; the 2 open ones are engine follow-ups). Status: not yet compiled after the 2026-11-20 round.

> **Unofficial Rust port of Chunky for the Horizon server (https://github.com/arismoko/horizon). Not affiliated with or endorsed by the Chunky authors.**

Derived from Chunky (https://github.com/pop4959/Chunky, GPL-3.0). Copyright of the original: pop4959 and the Chunky
contributors. Licence: GNU GPL version 3, see `LICENSE`; attribution in `NOTICE`. Not affiliated with Chunky or its
author.

> **Built for and working only with [arismoko/horizon](https://github.com/arismoko/horizon)** (the Horizon fork that carries the mod API). [horizon-rust/horizon](https://github.com/horizon-rust/horizon) is experimental for mods and not supported: the earlier version for its native hooks / horizon-mixins is frozen on the `horizon-plain` branch (work stopped on 2026-11-12).

## Which Horizon

This is a native trait mod: `horizon-mod.toml`, a `Cargo.toml` that depends on `horizon-mod-api`, `src/lib.rs` with the hooks, and the plugin logic in the `core/` crate (plain Rust, no engine needed for its tests).

- Pin: `[native] horizon = "54090ee36c4e2c7b43b66db864220c577b94b7e0"` in `horizon-mod.toml` (game `26.3`). That is a development build of arismoko/horizon `main` plus mod API additions that are not all merged upstream yet, so a stock `main` may lack some host methods this mod calls until they land.
- It compiled and loaded on that build and was smoke-tested there; it has not seen a long run with real players. Treat it as experimental.
- Feature descriptions below were written for the earlier horizon-rust version; where they name engine calls (`native_*`) or old file paths, read them as descriptions only. The Known gaps list is current.

## Commands

Chunky's syntax, with the `chunky` root (`/chunky` in the plugin, and the `cy` alias). A
player needs `chunky.command.<sub>` from a permissions mod, else operator level 2; the console runs everything.

```
chunky start [world] [shape] [x z] [radius] [z radius]     chunky pause|continue|cancel [world]
chunky progress                                            chunky selection
chunky world [world]        chunky shape <shape>           chunky pattern <pattern> [csv file]
chunky center [x z]         chunky spawn                   chunky corners <x1> <z1> <x2> <z2>
chunky radius <r> [z r]     (+/-: relative, 10k, 625c, 2r)  chunky worldborder [world]
chunky silent               chunky quiet <seconds>         chunky reload [config|tasks]
chunky trim [world] [shape] [x z] [radius] [z radius] [outside|inside] [inhabited=a..b] [unmodified=2d]
chunky trim abort           chunky confirm                 chunky help [page]
chunky limit <rate>         chunky debug map
```

`start` and `trim` take the words in that order: a shape needs a world before it, the centre two numbers, the radii
one or two; the side word of `trim` comes after both radii. `cancel`, `start` over a saved task and `trim` ask for
`chunky confirm` within 60 seconds.

`limit <rate>` caps a task at that many chunks per second (0 for none; saved with the task). A player who starts or continues a task is told when it finishes, and `confirm` is kept per sender. Tab completion offers world (custom dimensions too), shape, pattern, trim-side and trim-filter words.

`debug map` writes `config/chunky/debug/<world>.png`. Trim filters: `inhabited=<min>..<max>` and `unmodified=<amount><unit>`.

### Border (ChunkyBorder)

`chunky border <add|remove|wrap|list|load|bypass|help>` is ChunkyBorder's command (GPL-3.0, pop4959/ChunkyBorder), node
`chunky.command.border`. `add [world] [shape] [x z] [radius] [z radius]` reads the words as `start` does and writes the
selection; `load [world]` copies a border into the selection; `wrap [none|default|both|radial|x|z|earth]` sets what
happens at the edge. Borders are kept in `config/chunkyborder/borders.json` (Gson's layout, so the Java file can be
copied in), the options in `config/chunkyborder/config.yml` or `config.toml` (`border-options` keys: `check-interval`,
`message`, `use-action-bar`, `effect`, `sound`, `prevent-mob-spawns`, `-enderpearl`, `-chorus-fruit`, `-end-gateway`,
`-trident`, `visualizer-enabled`, `-range`, `-color`). Players outside a border are sent back (or wrapped) every
`check-interval` ticks; nodes `chunkyborder.bypass.move`, `.place`, `.break` (default nobody) or `border bypass` let one through.

### API for other mods

`chunky:api` is a mod service (`host.commands().call_mod_service("chunky:api", bytes)`): one tab-separated line in, one
text line out. Chunky's calls: `version`, `is_running <world>`, `start_task <world> <shape> <x> <z> <rx> <rz> <pattern>`,
`pause_task`, `continue_task`, `cancel_task`; ChunkyBorder's: `border_get`, `border_add`, `border_remove`,
`border_set_wrap`, `border_inside`, `border_is_bypassing`, `border_set_bypassing`. `subscribe <mod id>` makes Chunky
call `<mod id>:chunky_event` with `generation_start`, `generation_progress`, `generation_complete`, `border_change` or
`border_redirect` lines (fields in the order of the Java records). The layout is in `src/horizon_chunky_core/api.rs`.

## Shapes and patterns

Shapes: `square`, `circle`, `rectangle`, `ellipse` (two radii), `diamond`, `triangle`, `pentagon`, `hexagon`, `star`.
A chunk is selected when its centre (`x * 16 + 8`) lies in the shape, which is aligned to the chunk grid, as in
Chunky. Rectangles and ellipses walk strips unless the pattern is `loop` or `world`, as Chunky chooses them.

Patterns: `ring` (the default: square bands 64 chunks wide), `concentric`, `spiral`, `loop`, `strip`, `region` (regions
of 32 chunks in rings, a Hilbert curve in each whole region), `world` (chunks the save already has), `csv <name>` (the
chunks listed in `config/chunky/<name>.csv`, one `x,z` per line).

## Config

`config/chunky/config.toml` (all optional):

```toml
continue-on-restart = false         # saved, unfinished tasks run again at start (else: `chunky continue`)
force-load-existing-chunks = false  # load chunks the save has fully generated instead of skipping them
update-interval = 1                 # seconds between progress lines (Chunky's `update-interval`, `quiet`)
silent = false
language = "en"                   # message file: any of the 28 in lang/ (de, fr, ja, zh_CN...); unknown: en
max-in-flight = 128                 # this port: chunks held at once per task (32 -> 180 cps, 128 -> 260+ at radius 1000)
tick-budget-ms = 50                 # this port: stop requesting while a level tick takes longer
chunk-timeout-ticks = 400           # this port: give up on a chunk that never loads
```

Chunky's Bukkit build sets `pause-when-empty-seconds` to 0 in server.properties at enable (and logs that a restart is needed); this port does the same at server start.

Tasks are saved to `config/chunky/tasks/<world>.properties` with Chunky's keys (`world`, `cancelled`, `center-x`,
`center-z`, `radius`, `shape`, `pattern` as `name` or `csv=<name>`, `chunks`, `time`); a rectangle or ellipse also saves
`radius-z`.

## Trim

Chunky deletes on the live server after `confirm`. This port writes the trim to `config/chunky/trim-pending.properties`
and applies it at the next server start, before any world opens (`chunky trim abort` calls it off). Back the world up
first: deleted chunks cannot be restored.

## Build

Build it as a native trait mod with arismoko's launcher (see the "native mods" how-to in the arismoko/horizon docs):

```sh
horizon-launcher native-build <path to this folder> --server-output <dir> --version <tag> --trust-native-code
```

`--trust-native-code` is your consent to compile and run this code inside the server. The logic crate tests run without the engine: `(cd core && cargo test)`.

## Known gaps

What does not work yet:

- The disk-space message (`format_start_disk`) is not ported: the original defines it but no command prints it. `start` checks `radius-limit` from `config/chunky/.chunky.properties`.
- Border: the squaremap mod draws the marker layer from the border events (dynmap, BlueMap and Pl3xMap markers are Java-only). An end gateway whose exit lies outside the border is cancelled with `prevent-end-gateway`, otherwise the exit is moved inside (the hook's writable `landing`). The visualizer's dust goes to everyone in range, not only to the player it is drawn for; the border effect `ender_signal` is drawn as portal particles.
- Live trim runs while the server is up (`LevelRef::delete_chunks`): a confirmed trim erases chunks a few per tick; chunks that are loaded or held wait and are retried every 5 s.
- The aggregate node `chunky.chunky` (default nobody, children: the root and every subcommand node but trim and border) and the `children: [chunky.command]` of each subcommand node follow plugin.yml (`add_with_children`).
- A Java `config.yml` copied into `config/chunky/` is read when `config.toml` is absent (same keys; its `version` key is ignored).
- Messages are translated by matching each printed line against `lang/en.json` (see `src/horizon_chunky_core/lang.rs`); a line whose wording differs from that file is printed in English.
