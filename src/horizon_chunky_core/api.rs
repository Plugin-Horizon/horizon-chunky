//! `ChunkyAPI` and `ChunkyBorderAPI` for other Horizon mods. Java plugins call these interfaces' methods; a Horizon
//! mod calls the service `chunky:api` (`host.commands().call_mod_service`) with one tab-separated line, `<op>` then its
//! arguments, and gets one text line back (`true`/`false`, a number, or tab-separated fields; `none` for nothing).
//! The listeners (`onGenerationStart` ...) become one call `subscribe <mod id>`: from then on the mod's own service
//! `<mod id>:chunky_event` is called with one event line, `<kind>` then its fields, in the order of the Java records.
//!
//! Operations: `version`, `is_running <world>`, `start_task <world> <shape> <x> <z> <rx> <rz> <pattern>`,
//! `pause_task <world>`, `continue_task <world>`, `cancel_task <world>`, `subscribe <id>`, `unsubscribe <id>`;
//! the border API: `border_version`, `border_get <world>` (`<world> <shape> <x> <z> <rx> <rz> <wrap>`),
//! `border_add <world> <shape> <x> <z> <rx> <rz>`, `border_remove <world>`, `border_set_wrap <world> <wrap>`,
//! `border_inside <world> <x> <z>`, `border_is_bypassing <uuid>`, `border_set_bypassing <uuid> <true|false>`.
//! Events: `generation_start`, `generation_progress`, `generation_complete`, `border_change`, `border_redirect`.

use super::border::{BorderData, RedirectKind, Wrap};
use super::engine::{canon_world, Engine};
use super::geom::{Chunk, Selection, Shape};

/// A player's id as the dashed UUID text.
pub fn uuid_text(id: u128) -> String {
    let h = format!("{id:032x}");
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// A UUID with or without dashes.
pub fn parse_uuid(s: &str) -> Option<u128> {
    let plain: String = s.chars().filter(|c| *c != '-').collect();
    (plain.len() == 32).then(|| u128::from_str_radix(&plain, 16).ok()).flatten()
}

/// `GenerationStartEvent`: world, shape, centre, radii, pattern, owner (empty for none), continued.
pub fn start_event(world: &str, sel: &Selection, owner: Option<&str>, continued: bool) -> String {
    format!(
        "generation_start\t{world}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{continued}",
        sel.shape.name(),
        sel.cx,
        sel.cz,
        sel.radius,
        sel.radius_z,
        sel.pattern.name(),
        owner.unwrap_or("")
    )
}

/// `GenerationProgressEvent`: world, chunks, complete, percent, hours, minutes, seconds, rate, x, z.
pub fn progress_event(world: &str, chunks: u64, complete: bool, percent: f64, elapsed_ms: u64, rate: f64, at: Chunk) -> String {
    let s = elapsed_ms / 1000;
    format!("generation_progress\t{world}\t{chunks}\t{complete}\t{percent}\t{}\t{}\t{}\t{rate}\t{}\t{}", s / 3600, s / 60 % 60, s % 60, at.0, at.1)
}

/// `GenerationCompleteEvent`: world, shape, centre, radii, pattern, owner, chunks, time in ms, reason
/// (`COMPLETED`, `PAUSED` or `CANCELLED`).
pub fn complete_event(world: &str, sel: &Selection, owner: Option<&str>, chunks: u64, time_ms: u64, reason: &str) -> String {
    format!(
        "generation_complete\t{world}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{chunks}\t{time_ms}\t{reason}",
        sel.shape.name(),
        sel.cx,
        sel.cz,
        sel.radius,
        sel.radius_z,
        sel.pattern.name(),
        owner.unwrap_or("")
    )
}

fn border_fields(b: &BorderData) -> String {
    format!("{}\t{}\t{}\t{}\t{}\t{}\t{}", b.world, b.shape.name(), b.cx, b.cz, b.rx, b.rz, b.wrap.name())
}

/// `BorderChangeEvent`: world, then the border's fields or `none` when it was removed.
pub fn border_change_event(world: &str, border: Option<&BorderData>) -> String {
    match border {
        Some(b) => format!("border_change\t{world}\t{}", border_fields(b)),
        None => format!("border_change\t{world}\tnone"),
    }
}

/// `BorderRedirectEvent`: player, world, from x y z, to x y z, type.
pub fn border_redirect_event(player: u128, world: &str, from: [f64; 3], to: [f64; 3], kind: RedirectKind) -> String {
    format!(
        "border_redirect\t{}\t{world}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        uuid_text(player),
        from[0],
        from[1],
        from[2],
        to[0],
        to[1],
        to[2],
        kind.name()
    )
}

/// The key a border is kept under: the canonical world name, else the name as given.
fn world_key(name: &str) -> String {
    canon_world(name).map_or_else(|| name.to_owned(), str::to_owned)
}

/// Answers one call of the service `chunky:api`; None for an operation or arguments it does not know.
pub fn handle(e: &mut Engine, payload: &str) -> Option<String> {
    let mut parts = payload.split('\t');
    let op = parts.next()?;
    let a: Vec<&str> = parts.collect();
    let num = |i: usize| a.get(i).and_then(|s| s.parse::<f64>().ok());
    let word = |i: usize| a.get(i).copied();
    let flag = |v: bool| v.to_string();
    Some(match op {
        "version" | "border_version" => "0".into(),
        "is_running" => flag(e.api_is_running(word(0)?)),
        "start_task" => flag(e.api_start(word(0)?, word(1)?, num(2)?, num(3)?, num(4)?, num(5)?, word(6)?)),
        "pause_task" => flag(e.api_pause(word(0)?)),
        "continue_task" => flag(e.api_continue(word(0)?)),
        "cancel_task" => flag(e.api_cancel(word(0)?)),
        "subscribe" => {
            e.subscribe(word(0)?);
            "true".into()
        }
        "unsubscribe" => {
            e.unsubscribe(word(0)?);
            "true".into()
        }
        "border_get" => e.borders.get(&world_key(word(0)?)).map_or_else(|| "none".to_owned(), border_fields),
        "border_add" => {
            let (Some(world), Some(shape)) = (canon_world(word(0)?), Shape::parse(word(1)?)) else { return Some("false".into()) };
            let (cx, cz, rx, rz) = (num(2)?, num(3)?, num(4)?, num(5)?);
            if rx < 0.0 || rz < 0.0 {
                return Some("false".into());
            }
            e.borders.add(BorderData { world: world.to_owned(), shape, cx, cz, rx, rz, wrap: Wrap::None });
            "true".into()
        }
        "border_remove" => flag(canon_world(word(0)?).is_some_and(|w| e.borders.remove(w))),
        "border_set_wrap" => {
            let name = word(1)?;
            let valid = Wrap::ALL.iter().any(|w| w.name().eq_ignore_ascii_case(name));
            flag(valid && e.borders.set_wrap(&world_key(word(0)?), Wrap::parse(name)))
        }
        "border_inside" => flag(e.borders.inside(&world_key(word(0)?), num(1)?, num(2)?)),
        "border_is_bypassing" => flag(e.borders.is_bypassing(parse_uuid(word(0)?)?)),
        "border_set_bypassing" => {
            e.borders.set_bypassing(parse_uuid(word(0)?)?, word(1)? == "true");
            "true".into()
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::horizon_chunky_core::engine::Config;

    fn engine() -> Engine {
        Engine::new(Config::default(), None)
    }

    #[test]
    fn a_uuid_round_trips_with_and_without_dashes() {
        let id = 0x0123_4567_89ab_cdef_0123_4567_89ab_cdefu128;
        assert_eq!(uuid_text(id), "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(parse_uuid(&uuid_text(id)), Some(id));
        assert_eq!(parse_uuid("0123456789abcdef0123456789abcdef"), Some(id));
        assert_eq!(parse_uuid("nope"), None);
    }

    #[test]
    fn the_task_calls_start_pause_continue_and_cancel_as_the_java_api_answers() {
        let mut e = engine();
        assert_eq!(handle(&mut e, "version").as_deref(), Some("0"));
        assert_eq!(handle(&mut e, "is_running\tworld").as_deref(), Some("false"));
        assert_eq!(handle(&mut e, "start_task\tworld\tcircle\t0\t0\t500\t500\tspiral").as_deref(), Some("true"));
        assert_eq!(handle(&mut e, "is_running\tworld").as_deref(), Some("true"));
        assert_eq!(handle(&mut e, "start_task\tworld\tcircle\t0\t0\t500\t500\tspiral").as_deref(), Some("false"), "already running");
        assert_eq!(handle(&mut e, "start_task\tmars\tcircle\t0\t0\t5\t5\tring").as_deref(), Some("false"), "unknown world");
        assert_eq!(handle(&mut e, "continue_task\tworld").as_deref(), Some("false"), "not paused");
        assert_eq!(handle(&mut e, "pause_task\tworld").as_deref(), Some("true"));
        assert_eq!(handle(&mut e, "is_running\tworld").as_deref(), Some("false"));
        assert_eq!(handle(&mut e, "pause_task\tworld").as_deref(), Some("false"));
        assert_eq!(handle(&mut e, "continue_task\tworld").as_deref(), Some("true"));
        assert_eq!(handle(&mut e, "cancel_task\tworld").as_deref(), Some("true"));
        assert_eq!(handle(&mut e, "cancel_task\tworld").as_deref(), Some("false"));
        assert_eq!(handle(&mut e, "bogus"), None);
        assert_eq!(handle(&mut e, "is_running"), None, "a missing argument is not an answer");
    }

    #[test]
    fn subscribers_get_the_start_pause_continue_and_cancel_events_in_the_records_field_order() {
        let mut e = engine();
        handle(&mut e, "subscribe\tmapper");
        handle(&mut e, "subscribe\tmapper");
        assert_eq!(e.subscribers(), ["mapper"]);
        handle(&mut e, "start_task\tworld\tcircle\t10\t-5\t500\t300\tspiral");
        handle(&mut e, "pause_task\tworld");
        handle(&mut e, "continue_task\tworld");
        handle(&mut e, "cancel_task\tworld");
        let events = e.take_api_events();
        assert_eq!(events[0], "generation_start\tworld\tcircle\t10\t-5\t500\t300\tspiral\t\tfalse");
        assert!(events[1].starts_with("generation_complete\tworld\tcircle\t10\t-5\t500\t300\tspiral\t\t0\t0\tPAUSED"), "{}", events[1]);
        assert!(events[2].ends_with("\ttrue"), "continued: {}", events[2]);
        assert!(events[3].ends_with("\tCANCELLED"), "{}", events[3]);
        assert!(e.take_api_events().is_empty());
        handle(&mut e, "unsubscribe\tmapper");
        assert!(e.subscribers().is_empty());
    }

    #[test]
    fn the_border_calls_add_read_wrap_and_remove() {
        let mut e = engine();
        assert_eq!(handle(&mut e, "border_get\tworld").as_deref(), Some("none"));
        assert_eq!(handle(&mut e, "border_inside\tworld\t99999\t0").as_deref(), Some("true"), "no border");
        assert_eq!(handle(&mut e, "border_add\tworld\tsquare\t0\t0\t100\t100").as_deref(), Some("true"));
        assert_eq!(handle(&mut e, "border_add\tworld\ttriangle2\t0\t0\t100\t100").as_deref(), Some("false"), "unknown shape");
        assert_eq!(handle(&mut e, "border_add\tworld\tsquare\t0\t0\t-1\t100").as_deref(), Some("false"));
        assert_eq!(handle(&mut e, "border_get\tworld").as_deref(), Some("world\tsquare\t0\t0\t100\t100\tnone"));
        assert_eq!(handle(&mut e, "border_inside\tworld\t99\t0").as_deref(), Some("true"));
        assert_eq!(handle(&mut e, "border_inside\tworld\t101\t0").as_deref(), Some("false"));
        assert_eq!(handle(&mut e, "border_set_wrap\tworld\tEARTH").as_deref(), Some("true"));
        assert_eq!(handle(&mut e, "border_set_wrap\tworld\ttrue").as_deref(), Some("false"), "only the enum names");
        assert_eq!(handle(&mut e, "border_get\tworld").as_deref(), Some("world\tsquare\t0\t0\t100\t100\tearth"));
        let id = uuid_text(42);
        assert_eq!(handle(&mut e, &format!("border_is_bypassing\t{id}")).as_deref(), Some("false"));
        handle(&mut e, &format!("border_set_bypassing\t{id}\ttrue"));
        assert_eq!(handle(&mut e, &format!("border_is_bypassing\t{id}")).as_deref(), Some("true"));
        assert_eq!(handle(&mut e, "border_remove\tworld").as_deref(), Some("true"));
        assert_eq!(handle(&mut e, "border_remove\tworld").as_deref(), Some("false"));
    }

    #[test]
    fn the_event_lines_follow_the_java_records() {
        let b = BorderData { world: "world".into(), shape: Shape::Circle, cx: 1.0, cz: 2.0, rx: 3.0, rz: 4.0, wrap: Wrap::X };
        assert_eq!(border_change_event("world", Some(&b)), "border_change\tworld\tworld\tcircle\t1\t2\t3\t4\tx");
        assert_eq!(border_change_event("world", None), "border_change\tworld\tnone");
        assert_eq!(
            border_redirect_event(1, "world", [1.0, 2.0, 3.0], [4.0, 5.0, 6.0], RedirectKind::Wrap),
            "border_redirect\t00000000-0000-0000-0000-000000000001\tworld\t1\t2\t3\t4\t5\t6\tWRAP"
        );
        assert_eq!(progress_event("world", 5, false, 12.5, 3_725_000, 4.5, (7, -8)), "generation_progress\tworld\t5\tfalse\t12.5\t1\t2\t5\t4.5\t7\t-8");
    }
}
