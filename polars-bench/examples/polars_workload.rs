//! Polars hot-path workload for the autonomous-profiler.
//!
//! Exercises high-headroom polars kernels on a parquet file:
//!   1. decode + decompress (ParquetReader)
//!   2. numeric cast to f64 + arrow reduction (sum)
//!   3. SORT a numeric column (comparison/radix sort over millions of rows)
//! repeated `iters` times to amplify the hot paths.
//!
//!   cargo run --profile profiling --example polars_workload -- <parquet> [iters]
//!
//! Prints a hardened correctness fingerprint so the improver loop can prove an
//! optimization preserved behavior. The BASELINE output is ground truth:
//!   FINGERPRINT sum=<global numeric sum> rows=<row count> sortfp=<order-sensitive>
//! - `sum`     catches decode/cast/reduction corruption (value-level).
//! - `rows`    catches row loss/duplication (exact).
//! - `sortfp`  = Σ sorted_col[i] * i  catches mis-ordering AND value corruption
//!              in the sorted column (position-weighted; insensitive only to
//!              tie reordering of *equal* values, which is not a correctness change).

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
    let mut sortfp = 0.0f64;
    let mut rows = 0usize;

    for _ in 0..iters {
        let file = File::open(&path).expect("open parquet file");
        let df = ParquetReader::new(file).finish()?; // decode + decompress
        rows = df.height();

        // First Float64 column drives the sort + is representative of the cast path.
        let sort_col: Option<String> = df
            .columns()
            .iter()
            .find(|c| matches!(c.dtype(), DataType::Float64))
            .map(|c| c.name().to_string());

        // Cast every primitive-numeric column to f64 and reduce (decode+cast+sum path).
        for col in df.columns() {
            if col.dtype().is_primitive_numeric() {
                let f = col.cast(&DataType::Float64)?;
                if let Ok(ca) = f.f64() {
                    acc += ca.sum().unwrap_or(0.0);
                }
            }
        }

        // Sort by the chosen numeric column (the high-headroom kernel).
        if let Some(name) = sort_col {
            let sorted = df.sort([name.as_str()], SortMultipleOptions::default())?;
            let sc = sorted.column(&name)?.cast(&DataType::Float64)?;
            let ca = sc.f64()?;
            // Position-weighted sum: order-sensitive correctness fingerprint.
            for (i, v) in ca.iter().enumerate() {
                if let Some(x) = v {
                    sortfp += x * (i as f64);
                }
            }
        }
    }

    println!("FINGERPRINT sum={acc:.6} rows={rows} sortfp={sortfp:.6} iters={iters}");
    eprintln!("[polars_workload] done: {path} x{iters}");
    Ok(())
}
