//! Per-sketch container section — intentionally **empty** in v2.0.
//!
//! # Decision: sketches stay inline in `document.json`
//!
//! The migration plan sketched a `sketches/<uuid>.json` layout (one file per
//! sketch). The v2.0 container **does not** implement it: [`Document`] already
//! holds sketches inline in a `BTreeMap<SketchId, Sketch>`
//! (`document.json → sketches`), so a separate section would create a **second
//! source of truth** that must be kept in sync with the inline copy, and would
//! churn the already-frozen `document.json` shape. Keeping sketches inline avoids
//! both problems (single authoritative payload; no dual-write reconciliation).
//!
//! This divergence from the plan is deliberate and flagged for orchestrator review
//! (see [`super`] module docs). This module is kept as the reserved home for a
//! future split — e.g. if very large sketches ever warrant lazy per-sketch loading
//! — at which point `sketches/` would become a **derived cache** of the inline
//! data, never a second authoritative source.
//!
//! [`Document`]: crate::document::Document
//! [`Sketch`]: crate::sketch::Sketch
