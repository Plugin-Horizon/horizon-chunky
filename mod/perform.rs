fn chunky_perform(
    game: &mut crate::game::Game<'_>,
    player: &crate::game::native_server::NativeServerPlayer,
    command: &str,
    ran: &mut bool,
) {
    if !command.split_whitespace().next().is_some_and(|w| w.eq_ignore_ascii_case("chunky")) {
        return;
    }
    *ran = true;
    if !crate::game::chunky_glue::may_run(&*game, player.uuid, player.permission, command) {
        game.native_feedback(horizon_client::text::Text::literal(
            "[Chunky] You do not have permission for this command.".to_string(),
        ));
        return;
    }
    let world = horizon_chunky_core::engine::canon_world(&player.dimension).unwrap_or("world").to_string();
    let who = horizon_chunky_core::engine::Who {
        at: Some(([player.position[0], player.position[2]], world)),
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
}
