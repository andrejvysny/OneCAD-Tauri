// ElementIdentity.h — the element-identity verbs (SCHEMA §7.5, W-WP5).
//
//   * AcquireElementIds — promote snapshot-scoped TopoKeys to persistent
//     ElementIds. STATELESS worker-side: the worker resolves each pick's
//     {topoKey, anchor} against the snapshot's body shape into evidence
//     {topoKey, kind, bodyId, descriptor, anchor echo}; RUST mints the id (the
//     worker echoes an existing id only when the live partition already holds one
//     for that binding — Invariant 1). The worker stores NOTHING here.
//   * QueryElement — look up a current binding (by elementId, or by
//     {topoKey, bodyId}) within a snapshot; no mutation.
//   * ResolveRefs — dry-run ladder for repair dialogs; W-WP5 minimal (history/
//     descriptor echo, no scoring — that is W-WP6).
//
// All three operate on COPIES of the live session state (Session::bodies_copy /
// partition_copy), so they never touch the head lock while resolving.
#pragma once

#include "protocol/Envelope.h"
#include "session/Session.h"

namespace onecad::session {

protocol::Envelope handle_acquire_element_ids(Session& session, const protocol::Envelope& req);
protocol::Envelope handle_query_element(Session& session, const protocol::Envelope& req);
protocol::Envelope handle_resolve_refs(Session& session, const protocol::Envelope& req);

}  // namespace onecad::session
