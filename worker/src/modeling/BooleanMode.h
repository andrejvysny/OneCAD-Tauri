// Ported from OneCAD-CPP src/app/document/OperationRecord.h @ b4ddcccc (2026-07-16)
//
// W-WP3a adaptation: BooleanOperation.{h,cpp} originally pulled in the whole
// document layer (app/document/OperationRecord.h) solely for the app::BooleanMode
// enum. The rest of OperationRecord is Rust-owned in the Tauri architecture, so
// this header carries ONLY the enum (byte-identical to OperationRecord.h:32-37),
// keeping the C++ worker free of the (removed) document layer.
#ifndef ONECAD_MODELING_BOOLEANMODE_H
#define ONECAD_MODELING_BOOLEANMODE_H

namespace onecad::app {

enum class BooleanMode {
    NewBody,
    Add,
    Cut,
    Intersect
};

}  // namespace onecad::app

#endif  // ONECAD_MODELING_BOOLEANMODE_H
