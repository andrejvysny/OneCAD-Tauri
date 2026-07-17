// PlanExecutor.h — the ExecutePlan / AcceptPrepared / DiscardPrepared verbs
// (kernel lane). W-WP5: REAL OCCT ops (Extrude + Boolean, src/ops/*), ElementMap
// V2 partition deltas (src/elementmap/*), OCCT-history rebinding, opaque head
// token (prefixHashes[]), and an inline tessellate artifact (src/tess/*).
//
// The transaction machinery this proves (SCHEMA §7.2):
//   * fence the plan (revision/epoch match head; expectedBaseHash == head
//     historyPrefixHash) → else PROTOCOL_ERROR with reconciliation detail;
//   * execute ops sequentially into a SCRATCH clone (never the live session),
//     streaming one `planStep` event per executed step (bodyEvents,
//     elementMapDelta, needsRepair, three §12 signatures, diagnostics);
//   * stop at the first failure / NeedsRepair, preparing snapshot `m−1`;
//   * end with a terminal `PlanPrepared`; publish on AcceptPrepared (atomic swap)
//     or drop on DiscardPrepared / cancel / failure.
//
// TEST HOOKS (compiled always; harmless — a Rust core never authors these opIds):
//   * opId contains "__crash"       → std::abort() mid-plan (chaos drill).
//   * opId contains "__fail"        → the step fails (OP_FAILED); stop, prepare
//                                     ≤ m−1, PlanPrepared stoppedReason "opFailed".
//   * opId contains "__needsrepair" → emit a §9-shaped NeedsRepair for the step;
//                                     stop, prepare m−1, stoppedReason "needsRepair".
//   * opId contains "__slow"        → sleep ~500 ms in 10 ms slices, polling the
//                                     cancel token (proves solver-lane liveness +
//                                     cooperative cancellation).
#pragma once

#include "protocol/Dispatcher.h"  // HandlerContext
#include "protocol/Envelope.h"
#include "session/Session.h"

namespace onecad::session {

// ExecutePlan (kernel lane): fence → execute into scratch (streaming planStep
// events via ctx.emit) → terminal PlanPrepared / PROTOCOL_ERROR / CANCELLED.
protocol::Envelope handle_execute_plan(Session& session, const protocol::Envelope& req,
                                       protocol::HandlerContext& ctx);

// AcceptPrepared (kernel lane): re-fence + atomic publish.
protocol::Envelope handle_accept_prepared(Session& session, const protocol::Envelope& req);

// DiscardPrepared (kernel lane): drop the scratch.
protocol::Envelope handle_discard_prepared(Session& session, const protocol::Envelope& req);

}  // namespace onecad::session
