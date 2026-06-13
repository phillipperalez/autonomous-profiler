# Highlight: an LLM loop made polars' sort-gather 4% faster — provably without changing behavior

**Target:** [polars](https://github.com/pola-rs/polars) `0.54.4` (local clone, never pushed upstream)
**Commit:** `e30ab42` `perf(sort): resolve_chunked_idx` *[autoperf]*
**Result:** end-to-end sort+gather workload on NYC-taxi (2.96M rows) **−4.0%** (496 ms → 474 ms, min-of-5), peak RAM unchanged.
**Generated + verified by:** the autonomous-profiler improve loop — profiled → found the hotspot → edited polars → gated on a correctness fingerprint + a quiesced benchmark → committed.

---

## The change

`resolve_chunked_idx` maps a global row index to `(chunk, offset)` and runs **once per gathered element** — millions of times in a sort, where the rows are scattered across the array's chunks. The original used a binary search (`partition_point`) on every call.

```diff
 #[rustfmt::skip]
 #[inline]
 fn resolve_chunked_idx(idx: IdxSize, cumlens: &[IdxSize]) -> (usize, usize) {
-    let chunk_idx = cumlens.partition_point(|cl| idx >= *cl) - 1;
+    // `cumlens` holds the exclusive prefix sums of the chunk lengths (the first
+    // element is always 0). We want the last chunk whose start offset is <= idx.
+    // The vast majority of gathered arrays have only a handful of chunks, so a
+    // branch-predictor-friendly linear scan beats the binary search performed by
+    // `partition_point` in the per-element gather hot path. Falls back to the
+    // binary search for arrays with many chunks.
+    let chunk_idx = if cumlens.len() <= 8 {
+        // Count how many chunk-start offsets are <= idx; the matching chunk is
+        // one before that count. cumlens[0] == 0 so the count is always >= 1.
+        let mut count = 0;
+        for cl in cumlens {
+            count += (idx >= *cl) as usize;
+        }
+        count - 1
+    } else {
+        cumlens.partition_point(|cl| idx >= *cl) - 1
+    };
     (chunk_idx, (idx - cumlens[chunk_idx]) as usize)
 }
```

*One file, +17/−1: `crates/polars-core/src/chunked_array/ops/gather.rs`.*

## Why it's a win

- The gather calls this **per element**. Real chunked arrays almost always have a *handful* of chunks (NYC-taxi columns have **3**), so the array being searched is tiny.
- For a tiny slice, a **linear scan is faster than binary search**: the loop is branchless (`count += (idx >= *cl) as usize` compiles to a compare + conditional-add, no hard-to-predict branch), it streams the few elements through registers, and it sheds `partition_point`'s loop/branch overhead that doesn't pay off until the array is large.
- The `len() <= 8` guard keeps the **original O(log n) binary search for highly-fragmented arrays**, so there is no regression on the many-chunk case — only an upside on the common small-chunk case.
- Measured **−4.0%** on the full decode→cast→sort→gather workload (min-of-5, quiesced), RAM flat.

## Why it does **not** change behavior

The two paths are **mathematically identical for every input**:

- `cumlens` is the exclusive prefix-sum of chunk lengths, so it is **sorted non-decreasing** with `cumlens[0] == 0`. The predicate `idx >= cl` is therefore **monotone** over the slice — every `true` precedes every `false`.
- For a monotone predicate, `slice.partition_point(pred)` returns *exactly the number of elements for which `pred` is true*. The new branch computes that same count directly: `count = Σ (idx >= cl)`. So `count == partition_point(|cl| idx >= *cl)` **for all inputs**, including duplicate offsets (empty chunks) — the count of `true`s is invariant to how the comparisons are performed. Hence `count - 1` equals the original `chunk_idx`, and the returned `(chunk_idx, offset)` is **bit-for-bit identical**.
- For `len() > 8` the code *is* the original expression, unchanged.

Because the function is **pure** (no I/O, no allocation, no mutation, no global state) and **private** with only **two callers, both in `gather.rs`** (`grep` confirms: defined line 83, used lines 110 & 121), identical return values mean **zero possible side effects** — every caller observes exactly what it did before, just computed faster.

### Verification

- ✅ **Correctness fingerprint** (the loop's gate): over the gather of **2,964,624** elements, the workload's 3-part oracle matched the baseline exactly — `sum` (value integrity), `rows` (no loss), and `sortfp = Σ value·position` (ordering integrity): `sum=4683913106.640002 rows=2964624 sortfp=103271781267229.53`.
- ✅ **No regression gate:** committed only because it cleared −3% on a quiesced min-of-5 benchmark under the RAM budget.
- ⏳ **Full `polars-core` test suite** (`cargo test -p polars-core`): _run results appended below once the concurrent analyzer benchmark finishes (running it during a live benchmark would skew timings)._

> Note: this is exactly the kind of micro-optimization that is *unsafe to eyeball* — "swap binary search for linear scan" sounds like it could change tie-breaking on empty chunks. The equivalence proof above (count-of-trues == partition_point for a monotone predicate) is what makes it safe, and the fingerprint over ~3M elements is the empirical backstop.
