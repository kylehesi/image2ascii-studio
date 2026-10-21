//! Data-parallel helpers: rayon with the `parallel` feature (native builds),
//! plain loops without it (WebAssembly). Every task is independent and
//! writes only its own output, so results are bit-identical either way.

#[cfg(feature = "parallel")]
use rayon::prelude::*;

/// Calls `f(index, chunk)` for each `size`-long chunk of `data`.
pub(crate) fn for_each_chunk<T: Send>(data: &mut [T], size: usize, f: impl Fn(usize, &mut [T]) + Sync + Send) {
    assert!(size > 0, "chunk size must be positive");
    #[cfg(feature = "parallel")]
    data.par_chunks_mut(size).enumerate().for_each(|(i, c)| f(i, c));
    #[cfg(not(feature = "parallel"))]
    data.chunks_mut(size).enumerate().for_each(|(i, c)| f(i, c));
}

/// `(0..n).map(f).collect()`.
pub(crate) fn map<R: Send>(n: usize, f: impl Fn(usize) -> R + Sync + Send) -> Vec<R> {
    #[cfg(feature = "parallel")]
    return (0..n).into_par_iter().map(f).collect();
    #[cfg(not(feature = "parallel"))]
    (0..n).map(f).collect()
}
