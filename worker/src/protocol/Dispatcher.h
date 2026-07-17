// Dispatcher.h — verb registry + reader/kernel/solver threading for the worker.
//
// Threading model (W-WP3b: two worker lanes behind one reader):
//   * The caller's thread runs the stdin reader loop (blocking read_frame).
//   * The KERNEL thread pops OCCT/modeling jobs from its queue (single-writer
//     rule for the OCCT lane).
//   * The SOLVER lane thread pops Sketch* jobs from its OWN queue so PlaneGCS
//     drags never queue behind modeling (plan: "solver lane in V1"). Its mailbox
//     is LATEST-WINS per gesture for SolveDrag (only the newest unprocessed
//     target survives; superseded ones get a terminal CANCELLED/superseded resp
//     so the one-resp-per-id contract holds). Non-drag Sketch verbs are FIFO.
//   * Both lanes write terminal frames to stdout under a shared write mutex, so
//     frame bytes never interleave; each emitted frame is stamped with the §3
//     stamp — the session head (documentRevision/workerEpoch/snapshotId, via the
//     stamp source) plus a monotonic `seq` — under that same lock (§2).
//   * Cancel frames flip the atomic CancelToken registered under the target id.
//
// Contract:
//   * exactly one terminal resp per req.
//   * unknown verb => terminal resp, error.code = "PROTOCOL_ERROR" (well-framed-
//     illegal sub-case, SCHEMA §8; never a process exit).
//   * bad magic / protocol loss => loop returns exit code 2 (no resync).
//   * a handler may request clean shutdown via HandlerContext::request_shutdown.
#pragma once

#include <atomic>
#include <condition_variable>
#include <cstdint>
#include <deque>
#include <functional>
#include <mutex>
#include <queue>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <vector>

#include "protocol/Envelope.h"
#include "util/Cancel.h"

namespace onecad::protocol {

// Handed to each handler. Lets a handler observe cancellation, request a clean
// process shutdown (used by the "Shutdown" verb), and stream non-terminal frames
// (used by ExecutePlan to emit per-step `event` frames before its terminal resp).
struct HandlerContext {
    CancelToken& cancel;
    std::function<void(int exit_code)> request_shutdown;
    // Stamp + write a non-terminal worker frame (event/progress) on this lane's
    // output. Serialized with terminal resps under the single write mutex, so an
    // ExecutePlan planStep never interleaves mid-frame with a solver resp.
    std::function<void(Envelope& frame)> emit;
};

// A handler maps a request to its single terminal response. `bin` is the
// request frame's binary tail. Handlers run on the kernel or solver thread.
using Handler =
    std::function<Envelope(const Envelope& req, const std::vector<std::uint8_t>& bin,
                           HandlerContext& ctx)>;

class Dispatcher {
public:
    Dispatcher() = default;

    // Register (or replace) the handler for a verb routed to the KERNEL lane.
    void register_verb(std::string verb, Handler handler);

    // Register (or replace) the handler for a verb routed to the SOLVER lane
    // (Sketch* verbs). SolveDrag on this lane is coalesced latest-wins.
    void register_solver_verb(std::string verb, Handler handler);

    // Source of the §3 session-head stamp (documentRevision/workerEpoch/
    // snapshotId) applied to every worker frame. Set by main from WorkerSession;
    // when unset the head is all-zero (pre-session). `seq` is always assigned by
    // the Dispatcher and is NOT taken from the source.
    void set_stamp_source(std::function<Stamp()> source);

    // Run the full reader/kernel/solver loop over the given fds until EOF,
    // shutdown, or protocol error. If `hello` is non-null it is emitted as the
    // unsolicited first frame (SCHEMA §6, seq 0) before the reader loop starts.
    // Returns the process exit code (0/2).
    int run(int in_fd, int out_fd, const Envelope* hello = nullptr);

    // Execute a single request synchronously in-process, no threads/fds.
    // Used by --selftest. Returns the terminal response.
    Envelope dispatch_once(const Envelope& req,
                           const std::vector<std::uint8_t>& bin = {});

private:
    struct Job {
        Envelope env;
        std::vector<std::uint8_t> bin;
        CancelTokenPtr cancel;
        // Latest-wins coalescing hints (SolveDrag only).
        bool is_drag = false;
        std::uint64_t drag_gesture = 0;
        std::uint64_t drag_seq = 0;
    };

    // Execute one job's handler, translating unknown verbs and handler
    // exceptions into recoverable error responses. `emit` writes any non-terminal
    // frames the handler streams (wired to `stamp_and_write` on the lane's fd).
    Envelope execute(const Job& job, const std::function<void(Envelope&)>& emit);

    void kernel_loop(int out_fd);
    void solver_loop(int out_fd);

    // Enqueue onto the solver mailbox with latest-wins coalescing for drags.
    // Superseded drags are terminal-responded CANCELLED/superseded on `out_fd`.
    void enqueue_solver_job(Job job, int out_fd);

    // Serialize + stamp (monotonic seq) + write a terminal resp under the write
    // mutex, copying any handler binary (`out_bin`) into the frame tail.
    void stamp_and_write(int out_fd, Envelope& resp);

    std::unordered_map<std::string, Handler> handlers_;
    std::unordered_set<std::string> solver_verbs_;  // routing set (subset of handlers_)

    // §3 session-head stamp source (documentRevision/workerEpoch/snapshotId).
    std::function<Stamp()> stamp_source_;

    // Kernel work queue (reader -> kernel).
    std::mutex queue_mu_;
    std::condition_variable queue_cv_;
    std::queue<Job> queue_;
    bool kernel_stop_ = false;

    // Solver mailbox (reader -> solver lane); deque so drags can be coalesced.
    std::mutex solver_mu_;
    std::condition_variable solver_cv_;
    std::deque<Job> solver_queue_;
    bool solver_stop_ = false;

    // Single writer discipline across both lanes + monotonic output seq (§2).
    std::mutex write_mu_;
    std::uint64_t out_seq_ = 0;

    // Active cancel tokens keyed by request id (reader sets, lane clears).
    std::mutex tokens_mu_;
    std::unordered_map<std::uint64_t, CancelTokenPtr> tokens_;

    // Shutdown coordination.
    std::atomic<bool> shutdown_requested_{false};
    std::atomic<int> exit_code_{0};
    int in_fd_ = -1;  // closed by a lane on shutdown to unblock the reader
};

}  // namespace onecad::protocol
