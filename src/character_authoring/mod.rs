// =========================================
// =========================================
// src/character_authoring/mod.rs

//! Headless character construction, editing and rig authoring.
//!
//! Import through `motionloom::api::character_authoring`. The DSL remains
//! authoritative; each host owns its documents and rig sessions. Templates,
//! immutable edits and byte persistence are portable. Reviews and static GLB
//! export are asynchronous and return data in memory. Native path and blocking
//! conveniences are isolated in `native`; no UI or transport is required.
//! See the package's docs/CHARACTER_AUTHORING.md and docs/RIG_AUTHORING.md guides.
//!
//! ```
//! use motionloom::api::character_authoring::{build_template, CharacterParameters, ProportionPreset};
//! use motionloom::api::character_authoring::rig::humanoid_rig_standard;
//! let doc = build_template("example".into(), CharacterParameters::preset(ProportionPreset::Anime)).unwrap();
//! assert!(!doc.current.source.is_empty());
//! assert_eq!(humanoid_rig_standard().references[0].joints.len(), 65);
//! ```
mod cpu_review;
mod document;
mod edit;
mod error;
mod export;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
mod review;
pub mod rig;
mod service;
mod templates;
pub use cpu_review::*;
pub use document::*;
pub use edit::*;
pub use error::*;
pub use export::*;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{bounds, render_review, review_report};
pub use review::*;
pub use service::*;
pub use templates::*;
