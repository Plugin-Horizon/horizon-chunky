//! Chunky for Horizon: pre-generation tasks as a pure state machine.
//! The mod's helpers feed it commands, ticks and chunk loads and apply the chunk tickets it asks for.

pub mod engine;
pub mod geom;
pub mod region;
pub mod task;
pub mod trim;

pub use engine::{Actions, Config, Engine};

use std::sync::Mutex;

static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

/// Runs `f` on the process-wide engine, creating it on first use.
pub fn with<R>(f: impl FnOnce(&mut Engine) -> R) -> R {
    let mut g = ENGINE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(|| Engine::new(Config::default(), None)))
}

use std::path::Path;
use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static LEVEL_BEGIN: Mutex<Option<Instant>> = Mutex::new(None);
static INIT: OnceLock<()> = OnceLock::new();

/// Milliseconds on a clock that only moves forward.
pub fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Marks the start of a level's tick work, for the budget check.
pub fn level_begin() {
    *LEVEL_BEGIN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

/// The level's tick ended: tells the engine how long it took since `level_begin`.
pub fn level_end() {
    let began = LEVEL_BEGIN.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(b) = began {
        let ms = b.elapsed().as_millis() as u64;
        with(|e| e.level_tick_done(ms));
    }
}

/// First call only: reads `<config>/chunky/config.toml` and the saved tasks in `<config>/chunky/tasks`. `world_dir`
/// is the open world's folder (for the `world` pattern, skipping generated chunks and trim).
pub fn init_once(config_dir: &Path, world_dir: Option<std::path::PathBuf>) -> Vec<String> {
    with(|e| e.set_world_dir(world_dir));
    if INIT.set(()).is_err() {
        return Vec::new();
    }
    let base = config_dir.join("chunky");
    let cfg = std::fs::read_to_string(base.join("config.toml")).map(|t| Config::parse(&t)).unwrap_or_default();
    with(|e| {
        e.cfg = cfg;
        e.set_base(base);
        e.load_tasks()
    })
}

/// Before any world loads: runs a trim confirmed during the last run (`<config>/chunky/trim-pending.properties`).
pub fn server_about_to_start(config_dir: &Path) -> Vec<String> {
    trim::apply_pending(&config_dir.join("chunky"))
}
