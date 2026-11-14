//! REST handlers grouped by domain.
//!
//! The router in [`super`] wires these functions; the shared backend trait lives in
//! [`super::backend`].

pub(super) mod admin;
pub(super) mod agents;
pub(super) mod projects;
pub(super) mod sessions;
pub(super) mod settings;
pub(super) mod stream;
pub(super) mod system;
pub(super) mod terminals;
pub(super) mod workspace;
