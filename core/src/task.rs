//! Saved tasks: one `<world>.properties` file per world, `key=value` lines.

use crate::geom::{Pattern, Selection, Shape};

#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub world: String,
    pub cancelled: bool,
    pub sel: Selection,
    pub chunks: u64,
    pub time_ms: u64,
}

/// The file's keys are the ones Chunky's wiki documents. The z radius of a rectangle or ellipse is not documented
/// there: this mod writes it as `radius-z`, only for those two shapes, and a file without it uses `radius` for both.
pub fn render(s: &Saved) -> String {
    let z = if s.sel.shape.two_radii() { format!("radius-z={:?}\n", s.sel.radius_z) } else { String::new() };
    format!(
        "world={}\ncancelled={}\ncenter-x={:?}\ncenter-z={:?}\nradius={:?}\n{z}shape={}\npattern={}\nchunks={}\ntime={}\n",
        s.world,
        s.cancelled,
        s.sel.cx,
        s.sel.cz,
        s.sel.radius,
        s.sel.shape.name(),
        pattern_text(&s.sel.pattern),
        s.chunks,
        s.time_ms
    )
}

/// Chunky's task-file form of a pattern: its name, with the file name after `=` for `csv`.
fn pattern_text(p: &Pattern) -> String {
    match p {
        Pattern::Csv(value) => format!("csv={value}"),
        other => other.name().to_owned(),
    }
}

pub fn parse(text: &str) -> Result<Saved, String> {
    let mut m = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            m.insert(k.trim().to_owned(), v.trim().to_owned());
        }
    }
    let get = |k: &str| m.get(k).cloned().ok_or_else(|| format!("missing {k}"));
    let num = |k: &str| -> Result<f64, String> { get(k)?.parse().map_err(|_| format!("bad {k}")) };
    let radius = num("radius")?;
    if !(radius.is_finite() && radius > 0.0) {
        return Err("bad radius".into());
    }
    let radius_z = match m.get("radius-z") {
        Some(_) => num("radius-z")?,
        None => radius,
    };
    if !(radius_z.is_finite() && radius_z > 0.0) {
        return Err("bad radius-z".into());
    }
    Ok(Saved {
        world: get("world")?,
        cancelled: get("cancelled")? == "true",
        sel: Selection {
            cx: num("center-x")?,
            cz: num("center-z")?,
            radius,
            radius_z,
            shape: Shape::parse(&get("shape")?).ok_or("unknown shape")?,
            // A pattern this mod does not know falls back to the default order; the count is what resumes.
            pattern: {
                let text = get("pattern")?;
                let (name, value) = match text.split_once('=') {
                    Some((name, value)) => (name, Some(value)),
                    None => (text.as_str(), None),
                };
                Pattern::parse(name, value).unwrap_or(Pattern::Ring)
            },
        },
        chunks: get("chunks")?.parse().map_err(|_| "bad chunks")?,
        time_ms: get("time")?.parse().map_err(|_| "bad time")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = Saved {
            world: "world".into(),
            cancelled: false,
            sel: Selection { shape: Shape::Circle, pattern: Pattern::Region, cx: -32.5, cz: 0.0, radius: 5000.0, radius_z: 5000.0 },
            chunks: 1234,
            time_ms: 99,
        };
        assert_eq!(parse(&render(&s)), Ok(s));
    }

    #[test]
    fn two_radii_and_list_patterns_round_trip() {
        for (shape, pattern) in [(Shape::Ellipse, Pattern::Csv("mine".into())), (Shape::Star, Pattern::World), (Shape::Rectangle, Pattern::Spiral)] {
            let s = Saved {
                world: "world".into(),
                cancelled: false,
                sel: Selection { shape, pattern, cx: 1.0, cz: 2.0, radius: 300.0, radius_z: if shape.two_radii() { 700.0 } else { 300.0 } },
                chunks: 5,
                time_ms: 6,
            };
            let text = render(&s);
            assert_eq!(text.contains("radius-z="), shape.two_radii(), "{text}");
            assert_eq!(parse(&text), Ok(s));
        }
        let t = "world=world\ncancelled=false\ncenter-x=0\ncenter-z=0\nradius=50\nshape=rectangle\npattern=nonsense\nchunks=0\ntime=0\n";
        let s = parse(t).unwrap();
        assert_eq!((s.sel.radius_z, s.sel.pattern), (50.0, Pattern::Ring));
    }

    #[test]
    fn reads_the_documented_file() {
        let t = "world=world_nether\ncancelled=true\ncenter-x=0.0\ncenter-z=0.0\nradius=5000.0\nshape=square\npattern=region\nchunks=393079\ntime=4609587\n";
        let s = parse(t).unwrap();
        assert!(s.cancelled && s.chunks == 393079 && s.world == "world_nether");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("world=world\n").is_err());
        assert!(parse("world=w\ncancelled=false\ncenter-x=0\ncenter-z=0\nradius=-4\nshape=square\npattern=region\nchunks=0\ntime=0\n").is_err());
    }
}
