//! `chunky trim`: deleting saved chunks outside (or inside) a selection.
//!
//! The engine keeps region files open and caches what it read, so a running server's region files are never edited.
//! A confirmed trim is written to `trim-pending.properties` in the mod's config folder and applied by
//! `apply_pending` when the server next starts, before any world loads. A deleted chunk loses its terrain, its
//! entities and its points of interest (`region/`, `entities/` and `poi/`), and generates again from the seed when
//! someone comes near. Nothing else in the world is touched.
//!
//! Region files are edited as vanilla deletes a chunk: its location and timestamp entries are set to 0, the file is not
//! compacted. Each edited file is written as a copy and renamed over the old one; a file with no chunk left is deleted,
//! and so is the `.mcc` file of a deleted chunk that had its data outside the region file.

use crate::geom::{Chunk, Pattern, Selection, Shape};
use crate::region::{self, KINDS};
use std::collections::{BTreeSet, HashMap};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const PENDING: &str = "trim-pending.properties";

#[derive(Clone, Debug, PartialEq)]
pub struct TrimJob {
    pub world: String,
    /// The dimension's folder (`<world>/dimensions/minecraft/overworld`), absolute.
    pub dir: PathBuf,
    pub sel: Selection,
    /// Delete inside the selection instead of outside it.
    pub inside: bool,
    /// Only chunks whose `InhabitedTime` is at most this many ticks go (chunks whose time can't be read stay).
    pub inhabited: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrimReport {
    pub chunks_deleted: u64,
    pub chunks_kept: u64,
    pub files_deleted: u64,
    pub files_changed: u64,
    pub errors: Vec<String>,
}

impl TrimJob {
    fn targeted(&self, c: Chunk) -> bool {
        self.sel.contains(c) == self.inside
    }

    /// Delete flags for the chunks of region `r`, from the terrain file (`region/`): targeted, and inhabited little
    /// enough when the job asks. A chunk with no terrain record goes when it is targeted.
    fn decide(&self, r: (i32, i32)) -> Vec<bool> {
        let mut del: Vec<bool> = (0..1024).map(|i| self.targeted(region::chunk_at(r, i))).collect();
        let Some(limit) = self.inhabited else { return del };
        let path = region::region_path(&self.dir, "region", r);
        let Ok(mut f) = std::fs::File::open(&path) else { return del };
        let Ok(loc) = region::read_locations(&mut f) else { return vec![false; 1024] };
        for i in 0..1024 {
            if del[i] && loc[i] != 0 {
                let time = region::read_record(&mut f, loc[i]).and_then(|n| region::nbt_long(&n, "InhabitedTime"));
                del[i] = time.is_some_and(|t| t <= limit);
            }
        }
        del
    }

    fn regions(&self) -> BTreeSet<(i32, i32)> {
        KINDS.iter().flat_map(|k| region::region_files(&self.dir, k)).map(|(r, _)| r).collect()
    }

    /// What `apply` would delete, from the region headers only (the inhabited filter is not read here: with it the
    /// count is an upper bound).
    pub fn preview(&self) -> TrimReport {
        let mut rep = TrimReport::default();
        for (r, p) in region::region_files(&self.dir, "region") {
            let Ok(chunks) = region::existing_in(&p, r) else {
                rep.errors.push(format!("{} unreadable", p.display()));
                continue;
            };
            let gone = chunks.iter().filter(|c| self.targeted(**c)).count() as u64;
            rep.chunks_deleted += gone;
            rep.chunks_kept += chunks.len() as u64 - gone;
            if gone > 0 && gone == chunks.len() as u64 {
                rep.files_deleted += 1;
            } else if gone > 0 {
                rep.files_changed += 1;
            }
        }
        rep
    }

    /// Deletes the chunks. Only call it while no server has the world open.
    pub fn apply(&self) -> TrimReport {
        let mut rep = TrimReport::default();
        for r in self.regions() {
            let del = self.decide(r);
            for kind in KINDS {
                let path = region::region_path(&self.dir, kind, r);
                if !path.exists() {
                    continue;
                }
                match trim_file(&path, r, &del) {
                    Ok((gone, kept, removed)) => {
                        if kind == "region" {
                            rep.chunks_deleted += gone;
                            rep.chunks_kept += kept;
                        }
                        if removed {
                            rep.files_deleted += 1;
                        } else if gone > 0 {
                            rep.files_changed += 1;
                        }
                    }
                    Err(e) => rep.errors.push(format!("{}: {e}", path.display())),
                }
            }
        }
        rep
    }
}

/// Removes the chunks flagged in `del` from one region file: (deleted, kept, file removed).
fn trim_file(path: &Path, r: (i32, i32), del: &[bool]) -> std::io::Result<(u64, u64, bool)> {
    let mut f = std::fs::File::open(path)?;
    let loc = region::read_locations(&mut f)?;
    drop(f);
    let gone: Vec<usize> = (0..1024).filter(|&i| loc[i] != 0 && del[i]).collect();
    let kept = (0..1024).filter(|&i| loc[i] != 0 && !del[i]).count() as u64;
    if gone.is_empty() {
        return Ok((0, kept, false));
    }
    let folder = path.parent().unwrap_or(Path::new("."));
    let removed = kept == 0;
    if removed {
        std::fs::remove_file(path)?;
    } else {
        let tmp = path.with_extension("mca.chunky-trim");
        std::fs::copy(path, &tmp)?;
        let mut t = std::fs::OpenOptions::new().write(true).open(&tmp)?;
        for &i in &gone {
            t.seek(SeekFrom::Start(i as u64 * 4))?;
            t.write_all(&[0; 4])?;
            t.seek(SeekFrom::Start(4096 + i as u64 * 4))?;
            t.write_all(&[0; 4])?;
        }
        t.sync_all()?;
        drop(t);
        std::fs::rename(&tmp, path)?;
    }
    for &i in &gone {
        let c = region::chunk_at(r, i);
        let _ = std::fs::remove_file(folder.join(format!("c.{}.{}.mcc", c.0, c.1)));
    }
    Ok((gone.len() as u64, kept, removed))
}

pub fn render(job: &TrimJob) -> String {
    format!(
        "# A confirmed chunky trim, applied when the server next starts. Delete this file to call it off.\n\
         world={}\ndimension-dir={}\ncenter-x={:?}\ncenter-z={:?}\nradius={:?}\nradius-z={:?}\nshape={}\ninside={}\ninhabited={}\n",
        job.world,
        job.dir.display(),
        job.sel.cx,
        job.sel.cz,
        job.sel.radius,
        job.sel.radius_z,
        job.sel.shape.name(),
        job.inside,
        job.inhabited.map_or(-1, |v| v),
    )
}

pub fn parse(text: &str) -> Result<TrimJob, String> {
    let m: HashMap<&str, &str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim(), v.trim())))
        .collect();
    let get = |k: &str| m.get(k).copied().ok_or_else(|| format!("missing {k}"));
    let num = |k: &str| -> Result<f64, String> {
        get(k)?.parse::<f64>().ok().filter(|v| v.is_finite()).ok_or_else(|| format!("bad {k}"))
    };
    let dir = PathBuf::from(get("dimension-dir")?);
    if !dir.is_absolute() {
        return Err("dimension-dir must be absolute".into());
    }
    let (radius, radius_z) = (num("radius")?, num("radius-z")?);
    if radius <= 0.0 || radius_z <= 0.0 {
        return Err("bad radius".into());
    }
    let inhabited: i64 = get("inhabited")?.parse().map_err(|_| "bad inhabited")?;
    Ok(TrimJob {
        world: get("world")?.to_owned(),
        dir,
        sel: Selection {
            shape: Shape::parse(get("shape")?).ok_or("unknown shape")?,
            pattern: Pattern::Region,
            cx: num("center-x")?,
            cz: num("center-z")?,
            radius,
            radius_z,
        },
        inside: get("inside")? == "true",
        inhabited: (inhabited >= 0).then_some(inhabited),
    })
}

/// Runs a confirmed trim, if one waits in `base` (the mod's config folder). Call it before any world loads. The job
/// file is renamed to `trim-done-<unix seconds>.properties` (or `trim-failed-...`) with the result appended, so it runs
/// once.
pub fn apply_pending(base: &Path) -> Vec<String> {
    let path = base.join(PENDING);
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (lines, ok) = match parse(&text) {
        Err(e) => (vec![format!("[Chunky] Not trimming: {} is invalid ({e}).", path.display())], false),
        Ok(job) if !job.dir.join("region").is_dir() => (
            vec![format!("[Chunky] Not trimming {}: {} has no region folder.", job.world, job.dir.display())],
            false,
        ),
        Ok(job) => {
            let rep = job.apply();
            let mut v = vec![format!(
                "[Chunky] Trimmed {}: {} chunks deleted, {} kept, {} region files deleted, {} changed.",
                job.world, rep.chunks_deleted, rep.chunks_kept, rep.files_deleted, rep.files_changed
            )];
            v.extend(rep.errors.iter().map(|e| format!("[Chunky] Trim error: {e}")));
            (v, true)
        }
    };
    let done = base.join(format!("trim-{}-{stamp}.properties", if ok { "done" } else { "failed" }));
    let report: String = lines.iter().map(|l| format!("# {l}\n")).collect();
    if std::fs::write(&done, format!("{text}{report}")).is_ok() {
        let _ = std::fs::remove_file(&path);
    } else {
        // Never leave the job to run again on every start.
        let _ = std::fs::rename(&path, &done);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::region::testutil::{nbt, write_region};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("chunky-trim-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn job(dir: PathBuf, inside: bool, inhabited: Option<i64>) -> TrimJob {
        TrimJob {
            world: "world".into(),
            dir,
            sel: Selection { shape: Shape::Square, pattern: Pattern::Region, cx: 0.0, cz: 0.0, radius: 32.0, radius_z: 32.0 },
            inside,
            inhabited,
        }
    }

    /// Region (0, 0): chunks (0,0), (1,0), (5,5); region (3, 3): one chunk; entities of (5,5) and (0,0).
    fn world(d: &Path) {
        let full = || nbt("minecraft:full", 10);
        write_region(&region::region_path(d, "region", (0, 0)), &[(0, full()), (1, nbt("minecraft:full", 5000)), (5 + 5 * 32, full())]);
        write_region(&region::region_path(d, "region", (3, 3)), &[(0, full())]);
        write_region(&region::region_path(d, "entities", (0, 0)), &[(0, full()), (5 + 5 * 32, full())]);
        std::fs::write(d.join("region/c.5.5.mcc"), b"x").unwrap();
    }

    #[test]
    fn outside_trim_deletes_outside_chunks_their_entities_and_empty_files() {
        let d = tmp("outside");
        world(&d);
        let j = job(d.clone(), false, None);
        let p = j.preview();
        assert_eq!((p.chunks_deleted, p.chunks_kept, p.files_deleted, p.files_changed), (2, 2, 1, 1));
        let rep = j.apply();
        assert!(rep.errors.is_empty(), "{:?}", rep.errors);
        assert_eq!((rep.chunks_deleted, rep.chunks_kept), (2, 2));
        assert!(!region::region_path(&d, "region", (3, 3)).exists());
        let left = region::existing_in(&region::region_path(&d, "region", (0, 0)), (0, 0)).unwrap();
        assert_eq!(left, vec![(0, 0), (1, 0)]);
        let ents = region::existing_in(&region::region_path(&d, "entities", (0, 0)), (0, 0)).unwrap();
        assert_eq!(ents, vec![(0, 0)]);
        assert!(!d.join("region/c.5.5.mcc").exists());
        // The kept chunk still decodes after the edit.
        let mut f = std::fs::File::open(region::region_path(&d, "region", (0, 0))).unwrap();
        let loc = region::read_locations(&mut f).unwrap();
        assert!(region::is_full(&region::read_record(&mut f, loc[1]).unwrap()).unwrap());
        // A second run finds nothing more to do.
        assert_eq!(j.apply().chunks_deleted, 0);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn inside_and_inhabited_filters() {
        let d = tmp("inside");
        world(&d);
        let rep = job(d.clone(), true, Some(100)).apply();
        // Inside the 4x4 chunk square: (0,0) inhabited 10 goes, (1,0) inhabited 5000 stays.
        assert_eq!(rep.chunks_deleted, 1);
        let left = region::existing_in(&region::region_path(&d, "region", (0, 0)), (0, 0)).unwrap();
        assert_eq!(left, vec![(1, 0), (5, 5)]);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn pending_job_runs_once_and_bad_jobs_do_nothing() {
        let d = tmp("pending");
        let dim = d.join("w/dimensions/minecraft/overworld");
        world(&dim);
        let base = d.join("chunky");
        std::fs::create_dir_all(&base).unwrap();
        let j = job(dim.clone(), false, None);
        assert_eq!(parse(&render(&j)), Ok(j.clone()));
        std::fs::write(base.join(PENDING), render(&j)).unwrap();
        let log = apply_pending(&base);
        assert!(log[0].contains("2 chunks deleted"), "{log:?}");
        assert!(!base.join(PENDING).exists());
        assert!(apply_pending(&base).is_empty());
        let relative = render(&j).replace(&dim.display().to_string(), "relative/dir");
        std::fs::write(base.join(PENDING), relative).unwrap();
        assert!(apply_pending(&base)[0].contains("Not trimming"));
        assert!(!base.join(PENDING).exists());
        let _ = std::fs::remove_dir_all(d);
    }
}
