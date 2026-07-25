//! Shared schema for the daemon and GUI. Both binaries depend on this crate
//! so the config file format, the IPC wire format, and the validation rules
//! are defined exactly once - the hand-synced duplicates this replaces broke
//! twice from a field added on one side only.

pub mod config;
pub mod ipc;
