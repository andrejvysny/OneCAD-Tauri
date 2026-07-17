// test_frame_roundtrip.cpp — unit tests for the frame codec.
//
// No test framework (matches OneCAD-CPP prototype style): assert-style checks,
// process exit code == number of failures (0 == pass). Uses a real pipe so the
// blocking read path (read_frame) is exercised, including partial reads.
#include <unistd.h>

#include <algorithm>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <thread>
#include <vector>

#include "protocol/Frame.h"

using onecad::protocol::encode_frame;
using onecad::protocol::Frame;
using onecad::protocol::read_frame;
using onecad::protocol::ReadResult;
using onecad::protocol::ReadStatus;

namespace {

int g_failures = 0;

#define CHECK(cond)                                                          \
    do {                                                                     \
        if (!(cond)) {                                                       \
            std::fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond); \
            ++g_failures;                                                    \
        }                                                                    \
    } while (0)

void write_all(int fd, const std::uint8_t* p, std::size_t n) {
    std::size_t sent = 0;
    while (sent < n) {
        ssize_t w = ::write(fd, p + sent, n - sent);
        if (w <= 0) {
            if (w < 0 && errno == EINTR) continue;
            break;
        }
        sent += static_cast<std::size_t>(w);
    }
}

// Feed `bytes` into a fresh pipe, then read one frame back. Optionally close
// the write end after writing (to signal EOF).
ReadResult feed_and_read(const std::vector<std::uint8_t>& bytes, bool close_write) {
    int fds[2];
    if (pipe(fds) != 0) {
        std::fprintf(stderr, "pipe() failed\n");
        ++g_failures;
        return {};
    }
    write_all(fds[1], bytes.data(), bytes.size());
    if (close_write) close(fds[1]);
    ReadResult rr = read_frame(fds[0]);
    close(fds[0]);
    if (!close_write) close(fds[1]);
    return rr;
}

void test_roundtrip_with_bin() {
    Frame in;
    // The JSON is an opaque byte payload to the codec (round-tripped verbatim);
    // spelled in SCHEMA §3 shape for consistency (`t`, u64 `id`, `args`).
    in.json = R"({"v":1,"t":"req","id":1,"verb":"Tessellate","args":{}})";
    in.bin = {0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x7F};
    ReadResult rr = feed_and_read(encode_frame(in), /*close_write=*/true);
    CHECK(rr.status == ReadStatus::Ok);
    CHECK(rr.frame.json == in.json);
    CHECK(rr.frame.bin == in.bin);
}

void test_roundtrip_empty_bin() {
    Frame in;
    in.json = R"({"v":1,"t":"resp","id":1,"ok":true,"seq":0})";
    ReadResult rr = feed_and_read(encode_frame(in), /*close_write=*/true);
    CHECK(rr.status == ReadStatus::Ok);
    CHECK(rr.frame.json == in.json);
    CHECK(rr.frame.bin.empty());
}

// Genuine partial reads: a background writer emits the frame in tiny chunks
// with delays, forcing read() to return fewer bytes than requested.
void test_partial_reads() {
    Frame in;
    in.json = R"({"v":1,"t":"req","id":7,"verb":"SketchUpsert","args":{"n":42}})";
    in.bin = {1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13};
    const std::vector<std::uint8_t> bytes = encode_frame(in);

    int fds[2];
    if (pipe(fds) != 0) {
        std::fprintf(stderr, "pipe() failed\n");
        ++g_failures;
        return;
    }

    std::thread writer([&] {
        for (std::size_t i = 0; i < bytes.size(); i += 3) {
            std::size_t n = std::min<std::size_t>(3, bytes.size() - i);
            write_all(fds[1], bytes.data() + i, n);
            usleep(300);  // force the reader to see partial data
        }
        close(fds[1]);
    });

    ReadResult rr = read_frame(fds[0]);
    writer.join();
    close(fds[0]);

    CHECK(rr.status == ReadStatus::Ok);
    CHECK(rr.frame.json == in.json);
    CHECK(rr.frame.bin == in.bin);
}

void test_bad_magic() {
    std::vector<std::uint8_t> bytes = {'X', 'X', 'X', 'X', 0, 0, 0, 0, 0, 0, 0, 0};
    ReadResult rr = feed_and_read(bytes, /*close_write=*/true);
    CHECK(rr.status == ReadStatus::BadMagic);
}

void test_json_cap_violation() {
    // Valid magic, jsonLen = 16 MiB + 1 -> protocol error, body never read.
    const std::uint32_t oversized = (16u * 1024u * 1024u) + 1u;
    std::vector<std::uint8_t> bytes = {'O', 'C', 'W', '1'};
    bytes.push_back(static_cast<std::uint8_t>(oversized & 0xFF));
    bytes.push_back(static_cast<std::uint8_t>((oversized >> 8) & 0xFF));
    bytes.push_back(static_cast<std::uint8_t>((oversized >> 16) & 0xFF));
    bytes.push_back(static_cast<std::uint8_t>((oversized >> 24) & 0xFF));
    bytes.insert(bytes.end(), {0, 0, 0, 0});  // binLen 0
    ReadResult rr = feed_and_read(bytes, /*close_write=*/true);
    CHECK(rr.status == ReadStatus::ProtocolError);
}

void test_eof_at_start() {
    ReadResult rr = feed_and_read({}, /*close_write=*/true);
    CHECK(rr.status == ReadStatus::Eof);
}

void test_eof_mid_frame() {
    // Only 6 of the 12 header bytes, then EOF -> protocol loss.
    std::vector<std::uint8_t> bytes = {'O', 'C', 'W', '1', 0, 0};
    ReadResult rr = feed_and_read(bytes, /*close_write=*/true);
    CHECK(rr.status == ReadStatus::ProtocolError);
}

}  // namespace

int main() {
    test_roundtrip_with_bin();
    test_roundtrip_empty_bin();
    test_partial_reads();
    test_bad_magic();
    test_json_cap_violation();
    test_eof_at_start();
    test_eof_mid_frame();

    if (g_failures == 0) {
        std::fprintf(stderr, "test_frame_roundtrip: OK\n");
    } else {
        std::fprintf(stderr, "test_frame_roundtrip: %d failure(s)\n", g_failures);
    }
    return g_failures;
}
