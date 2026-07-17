// ExportStep.h — the ExportStep verb (SCHEMA §7.8, D2).
//
// Writes the requested live bodies to a STEP file at the Rust-provided temp path
// via STEPControl_Writer. Returns { written, bytes }. All IO is worker-side (the
// webview has zero fs capability). OCCT failures are guarded into a recoverable
// OP_FAILED (SCHEMA §8; session intact).
#pragma once

#include "protocol/Envelope.h"
#include "session/Session.h"

namespace onecad::io {

protocol::Envelope handle_export_step(session::Session& session, const protocol::Envelope& req);

}  // namespace onecad::io
