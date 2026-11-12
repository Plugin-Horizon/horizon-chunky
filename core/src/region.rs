//! Reading the world's region files (Anvil `r.<x>.<z>.mca`) without the engine: which chunks exist, which are fully
//! generated, and how long players spent in them. Used by the `world` pattern, by skipping existing chunks and by trim.
//!
//! A region file starts with 1024 four-byte locations (sector offset << 8 | sector count, 0 = no chunk) and 1024
//! four-byte timestamps; a chunk's record is a big-endian length, a compression byte (1 gzip, 2 zlib, 3 none, 4 lz4;
//! +128 = the data lives in `c.<x>.<z>.mcc` beside the file) and the compressed NBT.

use crate::geom::Chunk;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

pub const SECTOR: u64 = 4096;
pub const KINDS: [&str; 3] = ["region", "entities", "poi"];

/// The folder of one dimension inside a world folder: `dimensions/minecraft/<name>` (the layout of 26.x saves); a
/// world in the older layout (`region/` at the top, `DIM-1/`, `DIM1/`) is used when the new folder is missing.
pub fn dimension_dir(world_dir: &Path, world: &str) -> PathBuf {
    let (new, old) = match world {
        "world_nether" => ("the_nether", Some("DIM-1")),
        "world_the_end" => ("the_end", Some("DIM1")),
        _ => ("overworld", None),
    };
    let modern = world_dir.join("dimensions").join("minecraft").join(new);
    if modern.is_dir() {
        return modern;
    }
    match old {
        Some(o) if world_dir.join(o).join("region").is_dir() => world_dir.join(o),
        None if world_dir.join("region").is_dir() => world_dir.to_path_buf(),
        _ => modern,
    }
}

/// `r.-1.2.mca` to `(-1, 2)`.
pub fn region_of_name(name: &str) -> Option<(i32, i32)> {
    let rest = name.strip_prefix("r.")?.strip_suffix(".mca")?;
    let (x, z) = rest.split_once('.')?;
    Some((x.parse().ok()?, z.parse().ok()?))
}

pub fn region_path(dir: &Path, kind: &str, r: (i32, i32)) -> PathBuf {
    dir.join(kind).join(format!("r.{}.{}.mca", r.0, r.1))
}

/// Every region file of `kind` in a dimension folder, sorted.
pub fn region_files(dir: &Path, kind: &str) -> Vec<((i32, i32), PathBuf)> {
    let mut v: Vec<_> = std::fs::read_dir(dir.join(kind))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let r = region_of_name(p.file_name()?.to_str()?)?;
            Some((r, p))
        })
        .collect();
    v.sort();
    v
}

/// The location table of a region file; a file shorter than its header holds no chunk.
pub fn read_locations(file: &mut std::fs::File) -> std::io::Result<[u32; 1024]> {
    let mut head = vec![0u8; 4096];
    let len = file.metadata()?.len();
    let mut out = [0u32; 1024];
    if len < 8192 {
        return Ok(out);
    }
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut head)?;
    for (i, o) in out.iter_mut().enumerate() {
        *o = u32::from_be_bytes([head[i * 4], head[i * 4 + 1], head[i * 4 + 2], head[i * 4 + 3]]);
    }
    Ok(out)
}

/// The chunk at header index `i` of region `r`.
pub fn chunk_at(r: (i32, i32), i: usize) -> Chunk {
    (r.0 * 32 + (i % 32) as i32, r.1 * 32 + (i / 32) as i32)
}

pub fn index_of(c: Chunk) -> usize {
    (c.0.rem_euclid(32) + c.1.rem_euclid(32) * 32) as usize
}

/// The chunks a region file has an entry for.
pub fn existing_in(path: &Path, r: (i32, i32)) -> std::io::Result<Vec<Chunk>> {
    let mut f = std::fs::File::open(path)?;
    let loc = read_locations(&mut f)?;
    Ok((0..1024).filter(|&i| loc[i] != 0).map(|i| chunk_at(r, i)).collect())
}

/// Every chunk the dimension's region files have, region by region (sorted), each region in header order.
pub fn scan_existing(dir: &Path) -> Vec<Chunk> {
    region_files(dir, "region").into_iter().flat_map(|(r, p)| existing_in(&p, r).unwrap_or_default()).collect()
}

/// The decoded NBT of the record at `location`, None when it is unreadable, lz4 (not read here) or external.
pub fn read_record(file: &mut std::fs::File, location: u32) -> Option<Vec<u8>> {
    let (start, count) = (u64::from(location >> 8), u64::from(location & 255));
    if start < 2 || count == 0 {
        return None;
    }
    file.seek(SeekFrom::Start(start * SECTOR)).ok()?;
    let mut head = [0u8; 5];
    file.read_exact(&mut head).ok()?;
    let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
    if len < 1 || len > count * SECTOR {
        return None;
    }
    let mut data = vec![0u8; (len - 1) as usize];
    file.read_exact(&mut data).ok()?;
    let mut out = Vec::new();
    // At most 8 MiB decoded: a chunk's NBT is far smaller.
    let limit = 8 << 20;
    match head[4] {
        1 => flate2::read::GzDecoder::new(&data[..]).take(limit).read_to_end(&mut out).ok()?,
        2 => flate2::read::ZlibDecoder::new(&data[..]).take(limit).read_to_end(&mut out).ok()?,
        3 => return Some(data),
        _ => return None,
    };
    Some(out)
}

/// The first named tag `name` of NBT type `kind` in `nbt`, as the bytes after its name (a byte search: chunk NBT keeps
/// `Status` and `InhabitedTime` at its top level, and the names are long enough not to appear by chance).
fn find_tag<'a>(nbt: &'a [u8], kind: u8, name: &str) -> Option<&'a [u8]> {
    let mut pat = vec![kind];
    pat.extend_from_slice(&(name.len() as u16).to_be_bytes());
    pat.extend_from_slice(name.as_bytes());
    let at = nbt.windows(pat.len()).position(|w| w == pat.as_slice())?;
    Some(&nbt[at + pat.len()..])
}

pub fn nbt_string(nbt: &[u8], name: &str) -> Option<String> {
    let rest = find_tag(nbt, 8, name)?;
    let len = u16::from_be_bytes([*rest.first()?, *rest.get(1)?]) as usize;
    String::from_utf8(rest.get(2..2 + len)?.to_vec()).ok()
}

pub fn nbt_long(nbt: &[u8], name: &str) -> Option<i64> {
    let b = find_tag(nbt, 4, name)?.get(..8)?;
    Some(i64::from_be_bytes(b.try_into().ok()?))
}

/// Is the chunk's record fully generated (`Status` `minecraft:full`)? None when it can't be told.
pub fn is_full(nbt: &[u8]) -> Option<bool> {
    let s = nbt_string(nbt, "Status")?;
    Some(s == "minecraft:full" || s == "full")
}

/// Which chunks of a region are fully generated, one flag per header index; unreadable chunks count as not full, so
/// they are loaded.
pub fn full_chunks(path: &Path) -> Vec<bool> {
    let mut out = vec![false; 1024];
    let Ok(mut f) = std::fs::File::open(path) else { return out };
    let Ok(loc) = read_locations(&mut f) else { return out };
    for i in 0..1024 {
        if loc[i] != 0 {
            out[i] = read_record(&mut f, loc[i]).and_then(|n| is_full(&n)).unwrap_or(false);
        }
    }
    out
}

/// Region scans on a thread of their own, so the level's tick never waits on the disk: `full(c)` answers None until
/// the chunk's region is read, and asks for it.
pub struct ExistingIndex {
    dir: PathBuf,
    known: Arc<Mutex<HashMap<(i32, i32), Option<Vec<bool>>>>>,
    tx: Mutex<Option<Sender<(i32, i32)>>>,
}

impl ExistingIndex {
    pub fn new(dimension_dir: PathBuf) -> ExistingIndex {
        ExistingIndex { dir: dimension_dir, known: Arc::new(Mutex::new(HashMap::new())), tx: Mutex::new(None) }
    }

    /// Some(true) for a fully generated chunk, Some(false) for one to load, None while its region is being read.
    pub fn full(&self, c: Chunk) -> Option<bool> {
        let r = (c.0 >> 5, c.1 >> 5);
        let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        match known.get(&r) {
            Some(Some(flags)) => return Some(flags[index_of(c)]),
            Some(None) => return None,
            None => {}
        }
        let path = region_path(&self.dir, "region", r);
        if !path.exists() {
            known.insert(r, Some(vec![false; 1024]));
            return Some(false);
        }
        known.insert(r, None);
        drop(known);
        let mut tx = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        if tx.is_none() {
            let (s, rx): (Sender<(i32, i32)>, Receiver<(i32, i32)>) = std::sync::mpsc::channel();
            let (dir, known) = (self.dir.clone(), self.known.clone());
            let spawned = std::thread::Builder::new().name("chunky-scan".into()).spawn(move || {
                // Ends once the index (the sender) is dropped.
                while let Ok(r) = rx.recv() {
                    let flags = full_chunks(&region_path(&dir, "region", r));
                    known.lock().unwrap_or_else(|e| e.into_inner()).insert(r, Some(flags));
                }
            });
            if spawned.is_err() {
                // No thread: every chunk is loaded, as with force-load-existing-chunks.
                self.known.lock().unwrap_or_else(|e| e.into_inner()).insert(r, Some(vec![false; 1024]));
                return Some(false);
            }
            *tx = Some(s);
        }
        if let Some(s) = tx.as_ref() {
            let _ = s.send(r);
        }
        None
    }
}

#[cfg(test)]
pub mod testutil {
    //! Synthetic region files for the tests.
    use std::io::Write;
    use std::path::Path;

    /// Chunk NBT with only the tags the readers look at.
    pub fn nbt(status: &str, inhabited: i64) -> Vec<u8> {
        let mut v = vec![10, 0, 0];
        v.extend([4, 0, 13]);
        v.extend(b"InhabitedTime");
        v.extend(inhabited.to_be_bytes());
        v.extend([8, 0, 6]);
        v.extend(b"Status");
        v.extend((status.len() as u16).to_be_bytes());
        v.extend(status.as_bytes());
        v.push(0);
        v
    }

    /// Writes a region file holding `chunks` (header index, NBT), zlib compressed, one chunk per sector run.
    pub fn write_region(path: &Path, chunks: &[(usize, Vec<u8>)]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut head = vec![0u8; 8192];
        let mut body = Vec::new();
        let mut sector = 2u32;
        for (i, n) in chunks {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(n).unwrap();
            let data = z.finish().unwrap();
            let mut rec = ((data.len() + 1) as u32).to_be_bytes().to_vec();
            rec.push(2);
            rec.extend(data);
            let sectors = rec.len().div_ceil(4096) as u32;
            rec.resize(sectors as usize * 4096, 0);
            head[i * 4..i * 4 + 4].copy_from_slice(&((sector << 8) | sectors).to_be_bytes());
            head[4096 + i * 4..4096 + i * 4 + 4].copy_from_slice(&1234u32.to_be_bytes());
            sector += sectors;
            body.extend(rec);
        }
        head.extend(body);
        std::fs::write(path, head).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("chunky-region-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn reads_status_inhabited_time_and_existing_chunks() {
        let d = tmp("read");
        let dim = d.join("dimensions/minecraft/overworld");
        write_region(
            &region_path(&dim, "region", (-1, 0)),
            &[(0, nbt("minecraft:full", 99)), (33, nbt("minecraft:features", 0))],
        );
        assert_eq!(dimension_dir(&d, "world"), dim);
        assert_eq!(scan_existing(&dim), vec![(-32, 0), (-31, 1)]);
        let flags = full_chunks(&region_path(&dim, "region", (-1, 0)));
        assert!(flags[0] && !flags[33] && !flags[1]);
        let n = nbt("minecraft:full", 99);
        assert_eq!(nbt_long(&n, "InhabitedTime"), Some(99));
        assert_eq!(index_of((-31, 1)), 33);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_index_answers_after_its_thread_read_the_region() {
        let d = tmp("index");
        write_region(&region_path(&d, "region", (0, 0)), &[(5, nbt("minecraft:full", 0))]);
        let idx = ExistingIndex::new(d.clone());
        assert_eq!(idx.full((40, 40)), Some(false), "no file: nothing exists");
        let mut got = idx.full((5, 0));
        for _ in 0..200 {
            if got.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            got = idx.full((5, 0));
        }
        assert_eq!(got, Some(true));
        assert_eq!(idx.full((6, 0)), Some(false));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn old_layout_is_found() {
        let d = tmp("old");
        std::fs::create_dir_all(d.join("DIM-1/region")).unwrap();
        assert_eq!(dimension_dir(&d, "world_nether"), d.join("DIM-1"));
        assert_eq!(dimension_dir(&d, "world_the_end"), d.join("dimensions/minecraft/the_end"));
        assert_eq!(region_of_name("r.-3.12.mca"), Some((-3, 12)));
        assert_eq!(region_of_name("r.1.mca"), None);
        let _ = std::fs::remove_dir_all(d);
    }
}
