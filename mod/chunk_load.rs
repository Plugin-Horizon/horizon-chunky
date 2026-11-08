fn chunky_chunk_load(dimension: &str, chunk: [i32; 2]) {
    horizon_chunky_core::with(|e| e.chunk_loaded(dimension, (chunk[0], chunk[1])));
}
