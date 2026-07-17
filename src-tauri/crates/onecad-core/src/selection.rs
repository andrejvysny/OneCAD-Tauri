//! Selection semantics + pick-token / anchor capture.
//!
//! A faithful port of OneCAD-CPP `src/app/selection/SelectionManager.{h,cpp}`
//! (+ `SelectionTypes.h`): the pick sort, ambiguity gate, replace/extend/toggle
//! rules, and the deep-select machinery. Meshes label faces/edges with
//! snapshot-scoped `TopoKey`s; selecting a model element captures its
//! [`AnchorIntent`] (world point + surface UV) so the ref can later be promoted
//! to a persistent `ElementId` on demand (`AcquireElementIds`).
//!
//! ## Divergence (reported): deep-select cycles instead of a menu
//!
//! C++ `handleClick` returns `needsDeepSelect` with the candidate list on an
//! ambiguous click (a menu UI then calls `applySelectionCandidate`) and leaves
//! the C++ `lastClickIndex_` / `kClickCyclePixelThreshold` cycle scaffolding
//! unused. This port wires that scaffolding up: a **repeated click at the same
//! screen position cycles** through the ambiguous candidates (select the next
//! occluded element). The sorted candidates and an `ambiguous` flag are still
//! surfaced on the outcome so a menu UI remains possible.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::document::refs::{AnchorIntent, ElementRef};
use crate::math::{Vec2, Vec3};

/// Two ambiguous top hits within this many pixels are a deep-select ambiguity
/// (C++ `kAmbiguityPixelEpsilon`).
pub const AMBIGUITY_PIXEL_EPSILON: f64 = 2.0;
/// Two clicks within this many pixels are "the same location" for cycling
/// (C++ `kClickCyclePixelThreshold`).
pub const CLICK_CYCLE_PIXEL_THRESHOLD: f64 = 3.0;

/// What the picker is selecting against (C++ `SelectionMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionMode {
    /// Selecting sketch geometry (points/edges/regions/constraints).
    Sketch,
    /// Selecting model geometry (vertices/edges/faces/bodies).
    Model,
}

/// The kind of a selectable element (C++ `SelectionKind`, 9 values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionKind {
    /// Nothing.
    None,
    /// A sketch point entity.
    SketchPoint,
    /// A sketch edge (line/arc/circle).
    SketchEdge,
    /// A closed sketch region.
    SketchRegion,
    /// A sketch constraint glyph.
    SketchConstraint,
    /// A model vertex.
    Vertex,
    /// A model edge.
    Edge,
    /// A model face.
    Face,
    /// A whole body.
    Body,
}

impl SelectionKind {
    /// True for model (not sketch) kinds — these carry a topological anchor.
    #[must_use]
    pub fn is_model(self) -> bool {
        matches!(self, Self::Vertex | Self::Edge | Self::Face | Self::Body)
    }
}

/// Owner + element identity of a selectable (C++ `SelectionId`). `owner_id` is
/// the body / sketch id; `element_id` is the topo key or entity id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionId {
    /// Owning body / sketch id.
    pub owner_id: String,
    /// The element id within the owner (topo key / entity id).
    pub element_id: String,
}

impl SelectionId {
    /// Constructs an id.
    #[must_use]
    pub fn new(owner_id: impl Into<String>, element_id: impl Into<String>) -> Self {
        Self {
            owner_id: owner_id.into(),
            element_id: element_id.into(),
        }
    }
}

/// A raw pick candidate from the raycast (C++ `SelectionItem` hit). Carries the
/// data the [`AnchorIntent`] is built from: `world_pos`, `surface_uv`, the
/// owner, and (for model picks) the snapshot-scoped `topo_key`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickCandidate {
    /// The element kind.
    pub kind: SelectionKind,
    /// Owner + element identity.
    pub owner: SelectionId,
    /// Snapshot-scoped topology key (model picks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topo_key: Option<crate::ids::TopoKey>,
    /// Sort priority (lower = preferred; C++ `priority`).
    #[serde(default)]
    pub priority: i32,
    /// Screen-space distance to the click, pixels (C++ `screenDistance`).
    #[serde(default)]
    pub screen_distance: f64,
    /// Depth (nearer = preferred; C++ `depth`).
    #[serde(default)]
    pub depth: f64,
    /// Pick world position (anchor world point).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_pos: Option<Vec3>,
    /// Surface parameters at the pick (anchor UV).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_uv: Option<Vec2>,
    /// Whether the element is construction geometry.
    #[serde(default)]
    pub is_construction: bool,
}

/// A resolved, stored selection entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionItem {
    /// The element kind.
    pub kind: SelectionKind,
    /// Owner + element identity.
    pub owner: SelectionId,
    /// A repairable element ref (anchor-only until an `ElementId` is minted);
    /// `None` for sketch selections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<ElementRef>,
    /// Pick world position, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_pos: Option<Vec3>,
}

impl SelectionItem {
    fn key(&self) -> (SelectionKind, SelectionId) {
        (self.kind, self.owner.clone())
    }
}

/// A selection kind allow-list (empty ⇒ allow all; C++ `SelectionFilter`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionFilter {
    allowed: HashSet<SelectionKind>,
}

impl SelectionFilter {
    /// A filter allowing everything.
    #[must_use]
    pub fn all() -> Self {
        Self::default()
    }

    /// A filter allowing only the given kinds.
    #[must_use]
    pub fn only(kinds: impl IntoIterator<Item = SelectionKind>) -> Self {
        Self {
            allowed: kinds.into_iter().collect(),
        }
    }

    /// Whether `kind` is allowed (empty allow-list ⇒ all).
    #[must_use]
    pub fn allows(&self, kind: SelectionKind) -> bool {
        self.allowed.is_empty() || self.allowed.contains(&kind)
    }
}

/// Click modifier keys (C++ `ClickModifiers`; `cmd` = toggle on macOS).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClickModifiers {
    /// Shift held ⇒ extend selection.
    pub shift: bool,
    /// Cmd/Ctrl held ⇒ toggle selection.
    pub cmd: bool,
}

/// A screen-space click position (pixels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenPos {
    /// X pixel.
    pub x: f64,
    /// Y pixel.
    pub y: f64,
}

impl ScreenPos {
    /// Constructs a position.
    #[must_use]
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn within(self, other: ScreenPos, threshold: f64) -> bool {
        (self.x - other.x).abs() <= threshold && (self.y - other.y).abs() <= threshold
    }
}

/// The result of resolving a click.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionOutcome {
    /// Whether the selection set changed.
    pub changed: bool,
    /// The selection after resolution.
    pub selection: Vec<SelectionItem>,
    /// Whether this click advanced a deep-select cycle at the same position.
    pub cycled: bool,
    /// Whether the top hits are ambiguous (a deep-select is available).
    pub ambiguous: bool,
    /// The sorted, filtered candidates (for a menu UI).
    pub candidates: Vec<SelectionItem>,
    /// The [`AnchorIntent`] captured from the picked candidate (model picks).
    pub anchor: Option<AnchorIntent>,
}

/// The selection state machine (C++ `SelectionManager`, sans Qt signals).
#[derive(Debug, Clone)]
pub struct SelectionState {
    mode: SelectionMode,
    filter: SelectionFilter,
    deep_select: bool,
    selection: Vec<SelectionItem>,
    last_click: Option<ScreenPos>,
    last_keys: Vec<(SelectionKind, SelectionId)>,
    last_index: usize,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            mode: SelectionMode::Model,
            filter: SelectionFilter::all(),
            deep_select: true,
            selection: Vec::new(),
            last_click: None,
            last_keys: Vec::new(),
            last_index: 0,
        }
    }
}

impl SelectionState {
    /// A default state (Model mode, all kinds allowed, deep-select enabled).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The selection mode.
    #[must_use]
    pub fn mode(&self) -> SelectionMode {
        self.mode
    }

    /// Sets the mode; changing it clears the selection and cycle state (C++
    /// `setMode`).
    pub fn set_mode(&mut self, mode: SelectionMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.clear();
    }

    /// Sets the pick filter.
    pub fn set_filter(&mut self, filter: SelectionFilter) {
        self.filter = filter;
    }

    /// The pick filter.
    #[must_use]
    pub fn filter(&self) -> &SelectionFilter {
        &self.filter
    }

    /// Enables / disables deep-select cycling.
    pub fn set_deep_select(&mut self, enabled: bool) {
        self.deep_select = enabled;
    }

    /// The current selection.
    #[must_use]
    pub fn selection(&self) -> &[SelectionItem] {
        &self.selection
    }

    /// Clears the selection and cycle state.
    pub fn clear(&mut self) {
        self.selection.clear();
        self.reset_cycle();
    }

    /// Resolves a click against `candidates` with `modifiers` at `screen_pos`,
    /// mutating the selection and returning the outcome.
    ///
    /// Ports C++ `handleClick` + `applySelectionInternal`, extended with
    /// same-position deep-select cycling (see the module note).
    pub fn resolve_click(
        &mut self,
        candidates: Vec<PickCandidate>,
        modifiers: ClickModifiers,
        screen_pos: ScreenPos,
    ) -> SelectionOutcome {
        let hits = self.filter_and_sort(candidates);
        let candidate_items: Vec<SelectionItem> = hits.iter().map(item_from_candidate).collect();

        if hits.is_empty() {
            let changed = !modifiers.shift && !modifiers.cmd && !self.selection.is_empty();
            if changed {
                self.selection.clear();
            }
            self.reset_cycle();
            return SelectionOutcome {
                changed,
                selection: self.selection.clone(),
                cycled: false,
                ambiguous: false,
                candidates: candidate_items,
                anchor: None,
            };
        }

        // Deep-select cycling: a repeated click at the same location advances the
        // index through the (occluded) candidates. Requires ≥ 2 candidates — a
        // single-candidate repeat is not real cycling (index would stay put), so
        // the `cycled` flag must not be raised for it (F9).
        let same_location = self
            .last_click
            .is_some_and(|p| p.within(screen_pos, CLICK_CYCLE_PIXEL_THRESHOLD));
        let cycling =
            self.deep_select && same_location && !self.last_keys.is_empty() && hits.len() >= 2;
        let index = if cycling {
            (self.last_index + 1) % hits.len()
        } else {
            0
        };

        let chosen = &hits[index];
        let item = item_from_candidate(chosen);
        let anchor = anchor_from_candidate(chosen);

        let before = self.selection_keys();
        self.apply_selection(item, modifiers);
        let changed = before != self.selection_keys();

        self.last_click = Some(screen_pos);
        self.last_index = index;
        self.last_keys = hits.iter().map(|c| (c.kind, c.owner.clone())).collect();

        SelectionOutcome {
            changed,
            selection: self.selection.clone(),
            cycled: cycling,
            ambiguous: is_ambiguous(&hits),
            candidates: candidate_items,
            anchor,
        }
    }

    fn apply_selection(&mut self, item: SelectionItem, m: ClickModifiers) {
        if m.cmd {
            self.toggle(item);
        } else if m.shift {
            self.add(item);
        } else {
            self.selection = vec![item];
        }
    }

    fn toggle(&mut self, item: SelectionItem) {
        let key = item.key();
        if let Some(pos) = self.selection.iter().position(|s| s.key() == key) {
            self.selection.remove(pos);
        } else {
            self.selection.push(item);
        }
    }

    fn add(&mut self, item: SelectionItem) {
        let key = item.key();
        if !self.selection.iter().any(|s| s.key() == key) {
            self.selection.push(item);
        }
    }

    fn selection_keys(&self) -> Vec<(SelectionKind, SelectionId)> {
        self.selection.iter().map(SelectionItem::key).collect()
    }

    fn reset_cycle(&mut self) {
        self.last_click = None;
        self.last_keys.clear();
        self.last_index = 0;
    }

    /// Filters candidates by the allow-list and sorts by (priority, screen
    /// distance, depth) — C++ `filterHits`.
    fn filter_and_sort(&self, candidates: Vec<PickCandidate>) -> Vec<PickCandidate> {
        let mut hits: Vec<PickCandidate> = candidates
            .into_iter()
            .filter(|c| self.filter.allows(c.kind))
            .collect();
        hits.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then(cmp_f64(a.screen_distance, b.screen_distance))
                .then(cmp_f64(a.depth, b.depth))
        });
        hits
    }
}

fn cmp_f64(a: f64, b: f64) -> std::cmp::Ordering {
    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
}

/// C++ `isAmbiguous`: the top two hits share a priority and are within the pixel
/// epsilon.
fn is_ambiguous(hits: &[PickCandidate]) -> bool {
    hits.len() >= 2
        && hits[0].priority == hits[1].priority
        && (hits[0].screen_distance - hits[1].screen_distance).abs() <= AMBIGUITY_PIXEL_EPSILON
}

fn item_from_candidate(c: &PickCandidate) -> SelectionItem {
    // A model pick captures an anchor-only element ref (identity minted on
    // demand); a sketch pick carries no topological ref.
    let element = if c.kind.is_model() {
        anchor_from_candidate(c).map(|anchor| ElementRef {
            primary: None,
            intent: None,
            anchor: Some(anchor),
            extra: Default::default(),
        })
    } else {
        None
    };
    SelectionItem {
        kind: c.kind,
        owner: c.owner.clone(),
        element,
        world_pos: c.world_pos,
    }
}

/// Captures an [`AnchorIntent`] from a pick candidate (world point + surface UV).
///
/// The anchor is intentionally **partial** (F11): only the `world_point` and
/// `surface_uv` known at pick time are filled. `local_frame` and
/// `adjacency_hint` need worker-side topology and are left `None` here — the
/// worker fills them later, when the ref is promoted to a persistent `ElementId`
/// via `AcquireElementIds`.
fn anchor_from_candidate(c: &PickCandidate) -> Option<AnchorIntent> {
    c.world_pos.map(|world_point| AnchorIntent {
        world_point,
        surface_uv: c.surface_uv,
        local_frame: None,
        adjacency_hint: None,
        extra: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(
        kind: SelectionKind,
        elem: &str,
        priority: i32,
        screen: f64,
        depth: f64,
    ) -> PickCandidate {
        PickCandidate {
            kind,
            owner: SelectionId::new("body_1", elem),
            topo_key: Some(crate::ids::TopoKey::new(elem)),
            priority,
            screen_distance: screen,
            depth,
            world_pos: Some(Vec3::new_unchecked(1.0, 2.0, 3.0)),
            surface_uv: Some(Vec2::new_unchecked(0.5, 0.5)),
            is_construction: false,
        }
    }

    #[test]
    fn replace_extend_toggle_rules() {
        let mut s = SelectionState::new();
        let pos = ScreenPos::new(100.0, 100.0);
        // replace: pick face f:1.
        let out = s.resolve_click(
            vec![candidate(SelectionKind::Face, "f:1", 0, 0.0, 1.0)],
            ClickModifiers::default(),
            pos,
        );
        assert!(out.changed && s.selection().len() == 1);
        assert!(out.anchor.is_some(), "model pick captures an anchor");
        assert!(s.selection()[0].element.is_some());
        // shift-extend: add f:2 (different location so no cycle).
        let out = s.resolve_click(
            vec![candidate(SelectionKind::Face, "f:2", 0, 0.0, 1.0)],
            ClickModifiers {
                shift: true,
                cmd: false,
            },
            ScreenPos::new(200.0, 200.0),
        );
        assert!(out.changed && s.selection().len() == 2);
        // cmd-toggle: remove f:2.
        let out = s.resolve_click(
            vec![candidate(SelectionKind::Face, "f:2", 0, 0.0, 1.0)],
            ClickModifiers {
                shift: false,
                cmd: true,
            },
            ScreenPos::new(300.0, 300.0),
        );
        assert!(out.changed && s.selection().len() == 1);
        assert_eq!(s.selection()[0].owner.element_id, "f:1");
    }

    #[test]
    fn empty_hits_clears_only_without_modifiers() {
        let mut s = SelectionState::new();
        s.resolve_click(
            vec![candidate(SelectionKind::Face, "f:1", 0, 0.0, 1.0)],
            ClickModifiers::default(),
            ScreenPos::new(10.0, 10.0),
        );
        assert_eq!(s.selection().len(), 1);
        // empty click with shift keeps selection.
        let out = s.resolve_click(
            vec![],
            ClickModifiers {
                shift: true,
                cmd: false,
            },
            ScreenPos::new(10.0, 10.0),
        );
        assert!(!out.changed && s.selection().len() == 1);
        // empty click, no modifiers, clears.
        let out = s.resolve_click(
            vec![],
            ClickModifiers::default(),
            ScreenPos::new(10.0, 10.0),
        );
        assert!(out.changed && s.selection().is_empty());
    }

    #[test]
    fn deep_select_cycles_at_same_position() {
        let mut s = SelectionState::new();
        let pos = ScreenPos::new(50.0, 50.0);
        let cands = || {
            vec![
                candidate(SelectionKind::Face, "f:1", 0, 0.0, 1.0),
                candidate(SelectionKind::Face, "f:2", 0, 0.5, 2.0),
            ]
        };
        // First click: top hit f:1, ambiguous.
        let out = s.resolve_click(cands(), ClickModifiers::default(), pos);
        assert!(out.ambiguous && !out.cycled);
        assert_eq!(s.selection()[0].owner.element_id, "f:1");
        // Repeat at same position: cycles to f:2.
        let out = s.resolve_click(cands(), ClickModifiers::default(), pos);
        assert!(out.cycled);
        assert_eq!(s.selection()[0].owner.element_id, "f:2");
        // Repeat again: wraps back to f:1.
        let out = s.resolve_click(cands(), ClickModifiers::default(), pos);
        assert!(out.cycled);
        assert_eq!(s.selection()[0].owner.element_id, "f:1");
    }

    #[test]
    fn single_candidate_repeat_does_not_cycle() {
        let mut s = SelectionState::new();
        let pos = ScreenPos::new(50.0, 50.0);
        let cand = || vec![candidate(SelectionKind::Face, "f:1", 0, 0.0, 1.0)];
        // First click: not a cycle (no prior click).
        let out = s.resolve_click(cand(), ClickModifiers::default(), pos);
        assert!(!out.cycled);
        // Repeat at the same position with only ONE candidate: not real cycling
        // (the index cannot advance), so `cycled` stays false (F9).
        let out = s.resolve_click(cand(), ClickModifiers::default(), pos);
        assert!(
            !out.cycled,
            "single-candidate repeat must not report cycled"
        );
        assert_eq!(s.selection()[0].owner.element_id, "f:1");
    }

    #[test]
    fn filter_restricts_kinds() {
        let mut s = SelectionState::new();
        s.set_filter(SelectionFilter::only([SelectionKind::Edge]));
        // A face is filtered out; only the edge remains a hit.
        let out = s.resolve_click(
            vec![
                candidate(SelectionKind::Face, "f:1", 0, 0.0, 1.0),
                candidate(SelectionKind::Edge, "e:1", 1, 1.0, 1.0),
            ],
            ClickModifiers::default(),
            ScreenPos::new(0.0, 0.0),
        );
        assert_eq!(out.candidates.len(), 1);
        assert_eq!(s.selection()[0].kind, SelectionKind::Edge);
    }

    #[test]
    fn set_mode_clears_selection() {
        let mut s = SelectionState::new();
        s.resolve_click(
            vec![candidate(SelectionKind::Face, "f:1", 0, 0.0, 1.0)],
            ClickModifiers::default(),
            ScreenPos::new(0.0, 0.0),
        );
        assert!(!s.selection().is_empty());
        s.set_mode(SelectionMode::Sketch);
        assert!(s.selection().is_empty());
    }
}
