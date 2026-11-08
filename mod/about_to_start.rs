fn chunky_about_to_start() {
    // A trim confirmed during the last run, applied before any world (and so any region file) is open.
    if let Ok(dir) = crate::game::Game::native_config_dir() {
        for l in horizon_chunky_core::server_about_to_start(&dir) {
            crate::game::dedicated::log("INFO", &l);
        }
    }
}
