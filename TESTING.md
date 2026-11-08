> Round 3 (1:1 port of Chunky) is not built or run yet: the unit tests in `mod/core` were updated to the new behaviour
> and not executed. See `STATUS.md`. The measurements below are of the first release.

# Testing

Engine: Horizon `69fab597a` (dedicated server, `--nogui`), fast profile, throwaway server, seed 26263, view distance 4,
`pause-when-empty-seconds=0`. Unmodded engine = the same commit built without the bundle.

## Features added blind (not yet run in an engine)

Shapes beyond square/circle, `loop`/`spiral`/`csv`/`world` patterns, two radii, `spawn`, `corners`, `reload`,
`trim`/`confirm`, and skipping generated chunks (`force-load-existing-chunks`) were written without an engine build and
are covered only by unit tests against synthetic region files. The measurements below are of the first release.
`STATUS.md` lists what to run first.

## Unit tests (`cd mod/core && cargo test`): 41 pass
New since the first release (20): every shape's area and orientation, `loop`/`spiral` order, `csv`/`world` lists,
task files with `radius-z`, `spawn`/`corners`/two radii, `reload`, skipping generated chunks (also across a pause),
the `world` pattern's scan, trim outside/inside/inhabited on synthetic region files (headers, entities, `.mcc`,
emptied files deleted, kept chunks still decode, a second run does nothing), the confirm window and `trim abort`, and
a pending trim that runs once. The first release's 21:
Pattern coverage (every selected chunk exactly once, both shapes, both patterns), ring and Hilbert order, radius syntax,
task file round trip and a Chunky-written file, config parsing, and the state machine against a fake level: window
size, pause/continue without gaps, cancel, slow-tick throttle, timeout, resume from a saved file, chunks that loaded
earlier.

## Before (unmodded engine)
```
> chunky start world square 0 0 300
Unknown or incomplete command. See below for error
chunky start world square 0 0 300<--[HERE]
```

## After (engine with the mod), console
```
> chunky start world square 0 0 300
[Chunky] Task started for world: 1444 chunks, world square centre 0, 0 radius 300 (region pattern).
[Chunky] Task running for world. Processed: 1401 chunks (97.02%), ETA: 00:00:01, Rate: 35.8 cps, Current: -128, 96
[Chunky] Task finished for world. Processed: 1444 chunks (100.00%), Total time: 00:00:39
```
All 1444 selected chunks are in the region files afterwards (checked by reading the region headers), plus a ring of
about four chunks around the area that the engine generates for decoration and light.

Pause, continue and restart (`continue-on-restart = true`), circle radius 400 at 2000,2000 (2040 chunks):
```
Task paused for world at 364 chunks.   ->  Task continued for world.   -> (server stopped at 769 chunks)
[Chunky] Loaded saved task for world: 769 chunks done, continuing
[Chunky] Task finished for world. Processed: 2040 chunks (100.00%), Total time: 00:00:46
```
All 2040 chunks present on disk. The saved file keeps Chunky's keys.

Players: a non-operator gets "Unknown or incomplete command"; an operator can run every subcommand, `center`
without arguments uses their position (`tools/player_probe.py`).
`worldborder set 1000` then `chunky worldborder` -> `Selection set to the world border: world square centre 100.5, -49.5 radius 500`.

## Speed and load, square radius 500 (3969 chunks), fresh world, same seed
| run | time | chunks/s | server thread (`tick query` while running) | process peak RSS |
|---|---|---|---|---|
| engine `/pregen start 500 square` (built in) | ~40 s | 102 | 0.3 ms | 483 MB |
| mod, `max-in-flight = 4` | 82 s | 48 | avg 1-2 ms, P99 8 ms | 442 MB |
| mod, `max-in-flight = 32` (default) | 39 s | 102 | P50 1.0 ms, P99 7.3 ms | 442 MB |
| mod, `max-in-flight = 128` | 39 s | 102 | P50 1.6 ms, P99 6.9 ms | 472 MB |

Generation runs on the engine's worker threads, so the server thread stays far below 50 ms; the throttle
(`tick-budget-ms`) never had to act in these runs.

## Not tested / known limits
- Nether and End tasks: a level only ticks when something is in it or holds a ticket, and a ticket can only be set from
  inside that level's tick, so a task for an unopened level starts but does not advance (needs an upstream hook, see
  the report). Overworld only.
- No Paper `tasks/*.properties` produced by the real plugin was loaded, only the documented format.
- Order of the `region` pattern follows the wiki's description (Hilbert curve per region), not compared with the plugin.
- Chunks that already exist were loaded again in these runs (the skip came later and is not measured).
- Light of a just-generated chunk may finish after it is released.
- Several players issuing commands share one selection.
