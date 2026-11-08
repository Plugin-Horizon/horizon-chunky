fn chunky_tick(game: &mut crate::game::Game<'_>, dimension: &str) {
    use horizon_chunky_core as c;
    if c::with(|e| e.take_border_request(dimension)) {
        // hooks-all `native_world_border(dimension)`: [min x, min z, max x, max z] of the
        // level's border; the engine's one border when the helper knows no such level.
        let (centre, size) = match game.native_world_border(dimension) {
            Some([x0, z0, x1, z1]) => ([(x0 + x1) / 2.0, (z0 + z1) / 2.0], x1 - x0),
            None => (game.border.center(), game.border.size()),
        };
        let line = c::with(|e| e.set_border(centre, size));
        crate::game::dedicated::log("INFO", &line);
    }
    if !c::with(|e| e.any_running_in(dimension)) {
        return;
    }
    c::level_begin();
    let a = c::with(|e| e.tick(dimension, c::now_ms()));
    for ch in &a.release {
        game.native_force_chunk_in("chunky:task", dimension, [ch.0, ch.1], false);
    }
    for ch in &a.hold {
        game.native_force_chunk_in("chunky:task", dimension, [ch.0, ch.1], true);
    }
    for l in &a.log {
        crate::game::dedicated::log("INFO", l);
    }
}
