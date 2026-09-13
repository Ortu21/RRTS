//! Navigazione — congestione: costi live da spatial index.
//!
//! Solo costi additivi su celle affollate; nessun replan a metà rotta.

/// Extra route cost per crowded body in the destination cell: routes spread
/// around live crowds instead of piling through them. Tunable; validated by
/// keeping zero path failures on the stress workloads.
pub const CONGESTION_WEIGHT: f32 = 0.5;

/// Costo congestione per cella: celle affollate costano di più.
/// Puro e deterministico sul conteggio bucket.
pub fn congestion_cost(bucket_count: usize) -> f32 {
    CONGESTION_WEIGHT * bucket_count as f32
}
