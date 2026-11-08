fn chunky_chunk_unload(dimension: &str, chunk: [i32; 2]) {
    horizon_chunky_core::with(|e| e.chunk_unloaded(dimension, (chunk[0], chunk[1])));
}
