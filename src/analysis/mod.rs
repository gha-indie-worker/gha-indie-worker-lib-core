//! Embeddings and statistics: the "search, correlate, discover" half of the product.
//!
//! [`embedding`] is storage identity — one 4100-slot vector shape for every model, and the
//! comparison-space rules that stop two incompatible vectors from being scored against each other.
//! [`regression`] is the analysis: correlation, attribution, regression detection and
//! false-discovery control.

pub mod embedding;
pub mod regression;
