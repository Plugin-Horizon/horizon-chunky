//! `chunky debug map`: a PNG of the selection showing, for every chunk, how far the save has generated it (Chunky's
//! `GenerationMap`). Green: fully generated, lit and saved. Blue: generated, but unlit or not saved yet. Yellow: a
//! partial (not `full`) status. Red: unreadable or filed under the wrong position. Black: no chunk saved. White: outside
//! the shape. A selection wider than 4096 chunks is drawn with several chunks to a pixel (the worst status wins), and
//! the missing count is then approximate.

use crate::horizon_chunky_core::geom::Selection;
use crate::horizon_chunky_core::region;
use std::io::Write;
use std::path::Path;

pub const FULL: usize = 0;
pub const UNFINISHED: usize = 1;
pub const MISSING: usize = 2;
pub const INCOMPLETE: usize = 3;
pub const INVALID: usize = 4;
const STATUSES: usize = 5;
/// Palette: the five statuses, then the colour of a pixel outside the shape.
const COLORS: [u32; 6] = [0x7FB238, 0x4F8FE0, 0x000000, 0xE5E533, 0xE03C3C, 0xFFFFFF];
const OUTSIDE: u8 = STATUSES as u8;
const MAX_SIZE: i64 = 4096;

pub struct GenMap {
    min: (i64, i64),
    chunks: (i64, i64),
    scale: i64,
    width: i64,
    height: i64,
    pixels: Vec<u8>,
    present: Vec<u32>,
    pub counts: [u64; STATUSES],
    shape: Selection,
}

fn ceil_div(a: i64, b: i64) -> i64 {
    (a + b - 1).div_euclid(b)
}

/// How far the chunk record `nbt` of chunk (`x`, `z`) is generated.
fn classify(nbt: &[u8], x: i32, z: i32) -> usize {
    if region::nbt_int(nbt, "xPos") != Some(x) || region::nbt_int(nbt, "zPos") != Some(z) {
        return INVALID;
    }
    if region::is_full(nbt) != Some(true) {
        return INCOMPLETE;
    }
    let lit = region::nbt_has(nbt, 1, "isLightOn");
    let saved = region::nbt_long(nbt, "LastUpdate").is_some_and(|v| v > 0);
    if lit && saved { FULL } else { UNFINISHED }
}

/// Scans the dimension folder `dir` (`<world>/dimensions/minecraft/overworld`) for the chunks of `sel`.
pub fn scan(dir: &Path, sel: &Selection) -> std::io::Result<GenMap> {
    let (rx, rz) = sel.chunk_radii();
    let (cx, cz) = sel.centre_chunks();
    let min = (i64::from(cx) - i64::from(rx), i64::from(cz) - i64::from(rz));
    let chunks = (2 * i64::from(rx) + 1, 2 * i64::from(rz) + 1);
    let scale = ceil_div(chunks.0.max(chunks.1), MAX_SIZE).max(1);
    let (width, height) = (ceil_div(chunks.0, scale), ceil_div(chunks.1, scale));
    let size = (width * height) as usize;
    let mut map = GenMap {
        min,
        chunks,
        scale,
        width,
        height,
        pixels: vec![if scale == 1 { MISSING as u8 } else { 0 }; size],
        present: if scale == 1 { Vec::new() } else { vec![0; size] },
        counts: [0; STATUSES],
        shape: sel.clone(),
    };
    let (min_rx, min_rz) = (min.0.div_euclid(32), min.1.div_euclid(32));
    let (max_rx, max_rz) = ((min.0 + chunks.0 - 1).div_euclid(32), (min.1 + chunks.1 - 1).div_euclid(32));
    for (r, path) in region::region_files(dir, "region") {
        let (r0, r1) = (i64::from(r.0), i64::from(r.1));
        if r0 < min_rx || r0 > max_rx || r1 < min_rz || r1 > max_rz {
            continue;
        }
        let mut f = std::fs::File::open(&path)?;
        let loc = region::read_locations(&mut f)?;
        for i in 0..1024 {
            let c = region::chunk_at(r, i);
            let (ox, oz) = (i64::from(c.0) - min.0, i64::from(c.1) - min.1);
            if loc[i] == 0 || ox < 0 || oz < 0 || ox >= chunks.0 || oz >= chunks.1 {
                continue;
            }
            let status = match region::read_record(&mut f, loc[i]) {
                Some(nbt) => classify(&nbt, c.0, c.1),
                None => INVALID,
            };
            if sel.contains(c) {
                map.counts[status] += 1;
            }
            let pixel = ((oz / scale) * width + ox / scale) as usize;
            if scale == 1 {
                map.pixels[pixel] = status as u8;
            } else {
                map.pixels[pixel] = map.pixels[pixel].max(status as u8);
                map.present[pixel] += 1;
            }
        }
    }
    map.count_missing();
    Ok(map)
}

impl GenMap {
    /// Whether every pixel is one chunk (the missing count is exact).
    pub fn exact(&self) -> bool {
        self.scale == 1
    }

    fn count_missing(&mut self) {
        for pz in 0..self.height {
            let covered_z = self.scale.min(self.chunks.1 - pz * self.scale);
            for px in 0..self.width {
                let pixel = (pz * self.width + px) as usize;
                let covered_x = self.scale.min(self.chunks.0 - px * self.scale);
                let missing = if self.scale == 1 {
                    i64::from(self.pixels[pixel] == MISSING as u8)
                } else {
                    covered_x * covered_z - i64::from(self.present[pixel])
                };
                if missing <= 0 {
                    continue;
                }
                self.pixels[pixel] = self.pixels[pixel].max(MISSING as u8);
                if self.inside(px, pz) {
                    self.counts[MISSING] += missing as u64;
                }
            }
        }
    }

    /// Whether the pixel's middle chunk is inside the shape.
    fn inside(&self, px: i64, pz: i64) -> bool {
        let x = self.min.0 + px * self.scale + self.scale.min(self.chunks.0 - px * self.scale) / 2;
        let z = self.min.1 + pz * self.scale + self.scale.min(self.chunks.1 - pz * self.scale) / 2;
        self.shape.contains((x as i32, z as i32))
    }

    /// Writes the map as an 8-bit palette PNG, creating the folder.
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut raw = Vec::with_capacity(((self.width + 1) * self.height) as usize);
        for pz in 0..self.height {
            raw.push(0); // filter: none
            for px in 0..self.width {
                raw.push(if self.inside(px, pz) { self.pixels[(pz * self.width + px) as usize] } else { OUTSIDE });
            }
        }
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&raw)?;
        let idat = z.finish()?;
        let mut head = Vec::new();
        head.extend((self.width as u32).to_be_bytes());
        head.extend((self.height as u32).to_be_bytes());
        head.extend([8, 3, 0, 0, 0]);
        let palette: Vec<u8> = COLORS.iter().flat_map(|c| [(c >> 16) as u8, (c >> 8) as u8, *c as u8]).collect();
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png_chunk(&mut out, b"IHDR", &head);
        png_chunk(&mut out, b"PLTE", &palette);
        png_chunk(&mut out, b"IDAT", &idat);
        png_chunk(&mut out, b"IEND", &[]);
        std::fs::write(path, out)
    }
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend(kind);
    out.extend(data);
    let mut crc = 0xFFFF_FFFFu32;
    for &b in &out[start..] {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    out.extend((!crc).to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::horizon_chunky_core::geom::{Pattern, Shape};
    use crate::horizon_chunky_core::region::testutil::write_region;

    fn chunk(x: i32, z: i32, status: &str, lit: bool, last_update: i64) -> Vec<u8> {
        let mut v = vec![10, 0, 0, 3, 0, 4];
        v.extend(b"xPos");
        v.extend(x.to_be_bytes());
        v.extend([3, 0, 4]);
        v.extend(b"zPos");
        v.extend(z.to_be_bytes());
        v.extend([8, 0, 6]);
        v.extend(b"Status");
        v.extend((status.len() as u16).to_be_bytes());
        v.extend(status.as_bytes());
        if lit {
            v.extend([1, 0, 9]);
            v.extend(b"isLightOn");
            v.push(1);
        }
        v.extend([4, 0, 10]);
        v.extend(b"LastUpdate");
        v.extend(last_update.to_be_bytes());
        v.push(0);
        v
    }

    #[test]
    fn counts_each_status_and_writes_a_png() {
        let d = std::env::temp_dir().join(format!("chunky-genmap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        write_region(
            &region::region_path(&d, "region", (0, 0)),
            &[
                (0, chunk(0, 0, "minecraft:full", true, 5)),
                (1, chunk(1, 0, "minecraft:features", true, 5)),
                (2, chunk(2, 0, "minecraft:full", false, 5)),
                (32, chunk(9, 9, "minecraft:full", true, 5)),
            ],
        );
        let sel = Selection { shape: Shape::Square, pattern: Pattern::Region, cx: 0.0, cz: 0.0, radius: 32.0, radius_z: 32.0 };
        let map = scan(&d, &sel).unwrap();
        // A 5 x 5 chunk box: one chunk of each status but missing, the other 21 saved nowhere.
        assert_eq!(map.counts, [1, 1, 21, 1, 1]);
        assert!(map.exact());
        let png = d.join("debug/world.png");
        map.write(&png).unwrap();
        let bytes = std::fs::read(&png).unwrap();
        assert_eq!(&bytes[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&bytes[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 5);
        let _ = std::fs::remove_dir_all(d);
    }
}
