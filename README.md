# horizon-chunky

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

Chunky's syntax, with the `chunky` root (`/chunky` in the plugin; the `cy` alias is not registered on Horizon). A
player needs `chunky.command.<sub>` from a permissions mod, else operator level 2; the console runs everything.

```
chunky start [world] [shape] [x z] [radius] [z radius]     chunky pause|continue|cancel [world]
chunky progress                                            chunky selection
chunky world [world]        chunky shape <shape>           chunky pattern <pattern> [csv file]
chunky center [x z]         chunky spawn                   chunky corners <x1> <z1> <x2> <z2>
chunky radius <r> [z r]     (+/-: relative, 10k, 625c, 2r)  chunky worldborder [world]
chunky silent               chunky quiet <seconds>         chunky reload [config|tasks]
chunky trim [world] [shape] [x z] [radius] [z radius] [outside|inside] [inhabited ticks]
chunky trim abort           chunky confirm                 chunky help [page]
```

`start` and `trim` take the words in that order: a shape needs a world before it, the centre two numbers, the radii
one or two; the side word of `trim` comes after both radii. `cancel`, `start` over a saved task and `trim` ask for
`chunky confirm` within 60 seconds.

Not ported: `limit`, `debug map`, the `border` subcommands (ChunkyBorder is not on Horizon), the trim filter
`unmodified` and inhabited ranges. See the Known gaps section.

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
max-in-flight = 32                  # this port: chunks held at once per task
tick-budget-ms = 35                 # this port: stop requesting while a level tick takes longer
chunk-timeout-ticks = 400           # this port: give up on a chunk that never loads
```

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

What does not work yet on arismoko/horizon (each is marked `TODO(arismoko-api: ...)` in `src/lib.rs`):

- `chunky.command.<sub>` permission nodes are not checked: every sub-command needs operator level 2, so a permissions mod cannot grant a single sub-command.
