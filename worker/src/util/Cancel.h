// Cancel.h — cooperative cancellation token.
//
// PLACEHOLDER for W-WP0. A cancel frame (type:"cancel", matched by request id)
// flips the token for the in-flight job; the kernel thread polls it at safe
// points. The OCCT-aware variant — a Message_ProgressIndicator subclass whose
// UserBreak() consults this token so long OCCT builders abort — arrives with
// the real ops (later WP). Keep this type stable: the Dispatcher already hands
// a CancelToken to handlers.
#pragma once

#include <atomic>
#include <memory>

namespace onecad {

// Thread-safe flag. Set by the stdin reader thread (on a cancel frame),
// polled by the kernel worker thread executing a job.
class CancelToken {
public:
    CancelToken() = default;

    // Request cancellation. Idempotent; safe from any thread.
    void cancel() noexcept { cancelled_.store(true, std::memory_order_relaxed); }

    // Poll from the executing thread.
    [[nodiscard]] bool cancelled() const noexcept {
        return cancelled_.load(std::memory_order_relaxed);
    }

    // Reset for reuse (e.g. token pooled across jobs). Not concurrent-safe with
    // an in-flight cancel(); reset only between jobs on the owning thread.
    void reset() noexcept { cancelled_.store(false, std::memory_order_relaxed); }

private:
    std::atomic<bool> cancelled_{false};
};

using CancelTokenPtr = std::shared_ptr<CancelToken>;

}  // namespace onecad
