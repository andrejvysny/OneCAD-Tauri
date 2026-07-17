#include "protocol/Dispatcher.h"

#include <thread>
#include <unistd.h>
#include <utility>

#include "protocol/Frame.h"
#include "util/Log.h"

namespace onecad::protocol {

namespace {

std::uint64_t read_u64(const nlohmann::json& p, const char* key) {
    if (p.is_object() && p.contains(key) && p[key].is_number()) {
        return p[key].get<std::uint64_t>();
    }
    return 0;
}

}  // namespace

void Dispatcher::register_verb(std::string verb, Handler handler) {
    handlers_[std::move(verb)] = std::move(handler);
}

void Dispatcher::register_solver_verb(std::string verb, Handler handler) {
    solver_verbs_.insert(verb);
    handlers_[std::move(verb)] = std::move(handler);
}

void Dispatcher::set_stamp_source(std::function<Stamp()> source) {
    stamp_source_ = std::move(source);
}

Envelope Dispatcher::execute(const Job& job, const std::function<void(Envelope&)>& emit) {
    const Envelope& req = job.env;

    auto it = handlers_.find(req.verb);
    if (it == handlers_.end()) {
        // Unknown verb: well-framed but protocol-illegal (SCHEMA §8) — a terminal
        // PROTOCOL_ERROR resp, NOT a process exit. UNSUPPORTED is reserved for a
        // KNOWN verb with an unsupported op/param.
        return Envelope::error_response(
            req.id, ErrorInfo{"PROTOCOL_ERROR", "unknown verb: " + req.verb,
                              /*retriable=*/false});
    }

    HandlerContext ctx{
        *job.cancel,
        [this](int code) {
            exit_code_.store(code, std::memory_order_relaxed);
            shutdown_requested_.store(true, std::memory_order_relaxed);
        },
        emit,
    };

    try {
        return it->second(req, job.bin, ctx);
    } catch (const std::exception& ex) {
        WLOG_ERROR("handler for verb '%s' threw: %s", req.verb.c_str(), ex.what());
        // A handler failure is a recoverable op failure (SCHEMA §8 OP_FAILED):
        // the session is untouched (all work was in scratch).
        return Envelope::error_response(
            req.id, ErrorInfo{"OP_FAILED", ex.what(), /*retriable=*/false});
    }
}

void Dispatcher::stamp_and_write(int out_fd, Envelope& resp) {
    std::lock_guard<std::mutex> lk(write_mu_);
    if (stamp_source_) {
        const Stamp head = stamp_source_();  // §3 session-head fencing tokens
        resp.stamp.document_revision = head.document_revision;
        resp.stamp.worker_epoch = head.worker_epoch;
        resp.stamp.snapshot_id = head.snapshot_id;
    }
    resp.stamp.seq = out_seq_++;  // §2: monotonic across every emitted frame

    Frame f;
    try {
        f.json = serialize(resp);
    } catch (const EnvelopeError& ex) {
        WLOG_ERROR("failed to serialize response for id %llu: %s",
                   static_cast<unsigned long long>(resp.id), ex.what());
        Envelope fallback = Envelope::error_response(
            resp.id, ErrorInfo{"OP_FAILED", "response serialization failed", false});
        fallback.stamp = resp.stamp;  // preserve the head + assigned seq
        f.json = serialize(fallback);
        f.bin.clear();
    }
    f.bin = resp.out_bin;
    if (!write_frame(out_fd, f)) {
        WLOG_ERROR("write_frame failed (broken stdout); stopping lanes");
        shutdown_requested_.store(true, std::memory_order_relaxed);
    }
}

void Dispatcher::kernel_loop(int out_fd) {
    for (;;) {
        Job job;
        {
            std::unique_lock<std::mutex> lk(queue_mu_);
            queue_cv_.wait(lk, [this] { return !queue_.empty() || kernel_stop_; });
            if (queue_.empty()) {
                return;  // stop requested and drained
            }
            job = std::move(queue_.front());
            queue_.pop();
        }

        Envelope resp = execute(job, [this, out_fd](Envelope& e) { stamp_and_write(out_fd, e); });
        {
            std::lock_guard<std::mutex> lk(tokens_mu_);
            tokens_.erase(job.env.id);
        }
        stamp_and_write(out_fd, resp);

        // If a handler asked to shut down, unblock the reader by closing stdin.
        if (shutdown_requested_.load(std::memory_order_relaxed)) {
            if (in_fd_ >= 0) {
                ::close(in_fd_);
                in_fd_ = -1;
            }
            return;
        }
    }
}

void Dispatcher::solver_loop(int out_fd) {
    for (;;) {
        Job job;
        {
            std::unique_lock<std::mutex> lk(solver_mu_);
            solver_cv_.wait(lk, [this] { return !solver_queue_.empty() || solver_stop_; });
            if (solver_queue_.empty()) {
                return;  // stop requested and drained
            }
            job = std::move(solver_queue_.front());
            solver_queue_.pop_front();
        }

        Envelope resp = execute(job, [this, out_fd](Envelope& e) { stamp_and_write(out_fd, e); });
        {
            std::lock_guard<std::mutex> lk(tokens_mu_);
            tokens_.erase(job.env.id);
        }
        stamp_and_write(out_fd, resp);
    }
}

void Dispatcher::enqueue_solver_job(Job job, int out_fd) {
    std::vector<std::uint64_t> to_cancel;  // superseded request ids
    bool enqueue = true;
    {
        std::lock_guard<std::mutex> lk(solver_mu_);
        if (job.is_drag) {
            // Latest-wins: at most one unprocessed drag per gesture survives.
            for (auto it = solver_queue_.begin(); it != solver_queue_.end(); ++it) {
                if (it->is_drag && it->drag_gesture == job.drag_gesture) {
                    if (it->drag_seq < job.drag_seq) {
                        to_cancel.push_back(it->env.id);  // drop older
                        solver_queue_.erase(it);
                    } else {
                        to_cancel.push_back(job.env.id);  // incoming stale
                        enqueue = false;
                    }
                    break;
                }
            }
        }
        if (enqueue) {
            solver_queue_.push_back(std::move(job));
        }
    }
    if (enqueue) {
        solver_cv_.notify_one();
    }
    // Terminal-respond superseded drags (CANCELLED/superseded) — never dropped
    // (SCHEMA §3.5/§5.4: the terminal frame is always sent).
    for (std::uint64_t id : to_cancel) {
        {
            std::lock_guard<std::mutex> lk(tokens_mu_);
            tokens_.erase(id);
        }
        Envelope resp = Envelope::error_response(
            id, ErrorInfo{"CANCELLED", "superseded", /*retriable=*/false});
        stamp_and_write(out_fd, resp);
    }
}

int Dispatcher::run(int in_fd, int out_fd, const Envelope* hello) {
    in_fd_ = in_fd;
    kernel_stop_ = false;
    solver_stop_ = false;

    // SCHEMA §6: emit the unsolicited hello (seq 0) before reading any request.
    if (hello != nullptr) {
        Envelope h = *hello;
        stamp_and_write(out_fd, h);
    }

    std::thread kernel(&Dispatcher::kernel_loop, this, out_fd);
    std::thread solver(&Dispatcher::solver_loop, this, out_fd);

    int exit_code = 0;
    for (;;) {
        ReadResult rr = read_frame(in_fd);

        if (shutdown_requested_.load(std::memory_order_relaxed)) {
            exit_code = exit_code_.load(std::memory_order_relaxed);
            break;
        }

        if (rr.status == ReadStatus::Eof) {
            exit_code = 0;
            break;
        }
        if (rr.status == ReadStatus::BadMagic || rr.status == ReadStatus::ProtocolError) {
            WLOG_ERROR("protocol: %s", rr.error.c_str());
            exit_code = 2;
            break;
        }

        Envelope env;
        try {
            env = parse(rr.frame.json);
        } catch (const EnvelopeError& ex) {
            WLOG_ERROR("protocol: malformed envelope: %s", ex.what());
            exit_code = 2;
            break;
        }

        if (env.type == MsgType::Cancel) {
            std::lock_guard<std::mutex> lk(tokens_mu_);
            auto it = tokens_.find(env.id);
            if (it != tokens_.end()) {
                it->second->cancel();
            } else {
                WLOG_WARN("cancel for unknown/finished id %llu ignored",
                          static_cast<unsigned long long>(env.id));
            }
            continue;
        }

        if (env.type == MsgType::Credit) {
            // The worker emits no bulk streams yet; credit is a no-op (SCHEMA §5.3).
            continue;
        }

        if (env.type != MsgType::Req) {
            WLOG_WARN("ignoring non-request frame type on stdin (id %llu)",
                      static_cast<unsigned long long>(env.id));
            continue;
        }

        const bool solver_routed = solver_verbs_.count(env.verb) != 0;

        Job job;
        job.cancel = std::make_shared<CancelToken>();
        if (solver_routed && env.verb == "SolveDrag") {
            job.is_drag = true;
            job.drag_gesture = read_u64(env.args, "gestureId");
            job.drag_seq = read_u64(env.args, "seq");
        }
        job.env = std::move(env);
        job.bin = std::move(rr.frame.bin);
        {
            std::lock_guard<std::mutex> lk(tokens_mu_);
            tokens_[job.env.id] = job.cancel;
        }

        if (solver_routed) {
            enqueue_solver_job(std::move(job), out_fd);
        } else {
            {
                std::lock_guard<std::mutex> lk(queue_mu_);
                queue_.push(std::move(job));
            }
            queue_cv_.notify_one();
        }
    }

    // Drain + join both lanes.
    {
        std::lock_guard<std::mutex> lk(queue_mu_);
        kernel_stop_ = true;
    }
    queue_cv_.notify_all();
    {
        std::lock_guard<std::mutex> lk(solver_mu_);
        solver_stop_ = true;
    }
    solver_cv_.notify_all();
    if (kernel.joinable()) kernel.join();
    if (solver.joinable()) solver.join();

    return exit_code;
}

Envelope Dispatcher::dispatch_once(const Envelope& req,
                                   const std::vector<std::uint8_t>& bin) {
    Job job;
    job.env = req;
    job.bin = bin;
    job.cancel = std::make_shared<CancelToken>();
    // Synchronous single-shot path (--selftest): no streaming, drop any events.
    return execute(job, [](Envelope&) {});
}

}  // namespace onecad::protocol
