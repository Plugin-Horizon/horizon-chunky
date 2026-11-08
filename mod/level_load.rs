fn chunky_level_load(game: &mut crate::game::Game<'_>, dimension: &str) {
    let _ = dimension;
    if let Ok(dir) = crate::game::Game::native_config_dir() {
        for l in horizon_chunky_core::init_once(&dir, game.native_world_dir()) {
            crate::game::dedicated::log("INFO", &l);
        }
    }
}
