//! Polars hot-path workload for the autonomous-profiler. Reads a parquet file,
//! casts every numeric column to f64, and reduces it — repeated to amplify the
//! decode + cast + arrow-iteration paths the analyzer's taxi run is bottlenecked
//! on (polars_parquet decode, polars_compute cast, polars_arrow iterators).
//!
//!   cargo run --profile profiling --example polars_workload -- <parquet> [iters]
//!
//! Prints a deterministic `CHECKSUM` line so the loop can verify behavior is
//! preserved across polars edits.

use polars::prelude::*;
use std::fs::File;

fn main() -> PolarsResult<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "nyc-taxi.parquet".to_string());
    let iters: usize = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);

    let mut acc = 0.0f64;
    let mut rows = 0usize;
    for _ in 0..iters {
        let file = File::open(&path).expect("open parquet file");
        let df = ParquetReader::new(file).finish()?; // decode + decompress
        rows = df.height();
        for col in df.get_columns() {
            if col.dtype().is_primitive_numeric() {
                let f = col.cast(&DataType::Float64)?; // cast::primitive_to
                if let Ok(ca) = f.f64() {
                    acc += ca.sum().unwrap_or(0.0); // arrow reduction / iteration
                }
            }
        }
    }

    // Deterministic: same input + same polars semantics => same checksum.
    println!("CHECKSUM {acc:.6} rows {rows} iters {iters}");
    eprintln!("[polars_workload] done: {path} x{iters}");
    Ok(())
}
