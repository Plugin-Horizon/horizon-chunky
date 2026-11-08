fn chunky_console(game: &mut crate::game::Game<'_>, command: &str, ran: &mut bool) {
    if !command.split_whitespace().next().is_some_and(|w| w.eq_ignore_ascii_case("chunky")) {
        return;
    }
    let who = horizon_chunky_core::engine::Who {
        at: None,
        spawn: game.level.world_spawn.map(|p| [f64::from(p[0]), f64::from(p[2])]),
    };
    let world_dir = game.native_world_dir();
    let lines = horizon_chunky_core::with(|e| {
        e.set_world_dir(world_dir);
        e.command(command, &who, horizon_chunky_core::now_ms())
    });
    for l in lines {
        game.native_feedback(horizon_client::text::Text::literal(l));
    }
    *ran = true;
}
