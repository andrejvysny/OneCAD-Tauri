// MeshExport.h — the ExportStl / ExportObj verbs (SCHEMA §7.8).
//
// Writes the requested live bodies to an STL or OBJ mesh file at the Rust-provided
// temp path. Meshes are produced by tess::tessellate_raw (the SAME triangulation the
// viewport uses), so an export is deterministic + byte-consistent with the rendered
// geometry (the STL triangle count equals the tessellation triangle count).
//
// STL: binary (default) or ASCII, chosen by `args.binary` (SCHEMA §7.8). Binary is
// the 80-byte header + uint32 count + 50-byte triangle records; ASCII is the
// solid/facet/endsolid text form. Per-triangle geometric normals (legacy parity).
// OBJ: ASCII `v`/`vn`/`f` with per-body groups + 1-based vertex offsets.
//
// Returns { written, bytes, triangleCount } (STL) / { written, bytes } (OBJ). All IO
// is worker-side (the webview has zero fs capability). Failures are guarded into a
// recoverable OP_FAILED (SCHEMA §8; session intact).
#pragma once

#include "protocol/Envelope.h"
#include "session/Session.h"

namespace onecad::io {

protocol::Envelope handle_export_stl(session::Session& session, const protocol::Envelope& req);
protocol::Envelope handle_export_obj(session::Session& session, const protocol::Envelope& req);

}  // namespace onecad::io
