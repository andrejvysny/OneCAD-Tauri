// CancelProgress.h — a Message_ProgressIndicator that aborts an OCCT builder when
// the worker's cooperative cancel token is set (W-WP5).
//
// SCHEMA §3.5 / §8: cancellation is cooperative and the terminal `resp` is never
// dropped. A `cancel` frame flips the request's `CancelToken` (util/Cancel.h); this
// indicator's `UserBreak()` consults that token, so a long boolean `Build()` bails
// out promptly instead of running to completion. OCCT calls `UserBreak()` from the
// worker thread between algorithm steps; when it returns true the builder finishes
// not-done (or raises), which the caller maps to CANCELLED.
//
// `UserBreak()` must be cheap + thread-safe (OCCT doc): it only does a relaxed
// atomic load on the token. `Show()` is a required no-op (we render no progress UI
// in the worker; per-step `progress` frames are emitted by the Dispatcher layer).
#pragma once

#include <Message_ProgressIndicator.hxx>
#include <Message_ProgressScope.hxx>
#include <Standard_Boolean.hxx>

#include "util/Cancel.h"

namespace onecad::ops {

class CancelProgress : public Message_ProgressIndicator {
public:
    explicit CancelProgress(const onecad::CancelToken& token) : token_(token) {}

    // Consulted by OCCT between algorithm steps; true ⇒ abort. Thread-safe (a
    // relaxed atomic load), matching the OCCT contract for UserBreak().
    Standard_Boolean UserBreak() override { return token_.cancelled() ? Standard_True : Standard_False; }

    // No progress surface in the worker; intentionally empty.
    void Show(const Message_ProgressScope& /*scope*/, const Standard_Boolean /*isForce*/) override {}

private:
    const onecad::CancelToken& token_;
};

}  // namespace onecad::ops
