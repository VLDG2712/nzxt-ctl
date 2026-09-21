//! Exposes select modules for `examples/` (manual hardware verification
//! tools) and integration tests. The daemon itself is a binary
//! (`main.rs`); this library target exists only so those don't have to
//! duplicate module code.

pub mod gauge;
pub mod lcd;
