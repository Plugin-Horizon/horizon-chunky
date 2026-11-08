# Status

Round 3 (1:1 port of Chunky, GPL-3.0, pop4959/Chunky at c4274bd). Written without a build (no cargo in this round):
`rustfmt --check` parses every changed file, and the iterator orders were checked by simulation (Python), not in the
Rust code.

| Area | State |
|---|---|
| Command tree and usages (tab completion) | written, not built |
| Command behaviour and messages (en.json wording) | written, not built |
| Shapes (chunk-centre test, chunk-aligned radii, polygon formulas) | written, unit tests updated, not run |
| Patterns (ring default, concentric, spiral, loop, strip, region, world, csv) | written, unit tests updated, not run |
| Config keys | Chunky's four keys read; the port's three engine keys kept |
| Permission nodes per subcommand | written, not built |
| Licence (GPL-3.0), NOTICE, README | done |

## Test first once a build is possible
1. `cargo test` in `mod/core` (the unit tests were updated to the new behaviour, not run).
2. The bundle applies and builds at its pinned revision (`mod/horizon-mixin.toml`).
3. Tab completion after `/chunky ` and after `chunky shape `, `chunky pattern csv `, `chunky trim world square `.
4. `chunky start world square 0 0 300` against the same run on Chunky's wiki example; the chunk count of a ring is
   `(2 * ceil(r/16) + 1)^2` and a circle selects about 78 % of its square's chunks.
