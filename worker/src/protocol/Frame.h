// Frame.h — OneCAD worker wire framing.
//
// Frame layout (all multi-byte integers little-endian):
//
//   +--------+----------+---------+------------------+-----------------+
//   | magic  | jsonLen  | binLen  | json bytes       | bin bytes       |
//   | 4B     | u32 (LE) | u32(LE) | jsonLen bytes    | binLen bytes    |
//   +--------+----------+---------+------------------+-----------------+
//
//   magic  : ASCII "OCW1" = bytes 0x4F 0x43 0x57 0x31, compared bytewise.
//            (Equivalent to a Rust b"OCW1" literal; the value 0x4F435731 is
//            its big-endian reading — the wire bytes are the contract.)
//   jsonLen: length of the JSON envelope, capped at 16 MiB.
//   binLen : length of the binary tail, capped at 1 GiB.
//
// Transport rules (per plan "Key protocol decisions"):
//   * stdout carries frames ONLY (fd 1). stdin carries frames (fd 0).
//   * partial reads are looped until complete or EOF.
//   * EOF at a frame boundary is clean; EOF mid-frame is protocol loss.
//   * a cap violation is a protocol error.
//   * bad magic => NO resync; the process must exit(2).
#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace onecad::protocol {

struct Frame {
    std::string json;                // UTF-8 JSON envelope
    std::vector<std::uint8_t> bin;   // binary tail (possibly empty)
};

inline constexpr char kMagic[4] = {'O', 'C', 'W', '1'};
inline constexpr std::size_t kHeaderLen = 12;  // 4 magic + 4 jsonLen + 4 binLen

inline constexpr std::uint32_t kMaxJsonLen = 16u * 1024u * 1024u;      // 16 MiB
inline constexpr std::uint32_t kMaxBinLen = 1024u * 1024u * 1024u;     // 1 GiB

enum class ReadStatus {
    Ok,             // a complete frame was read
    Eof,            // clean EOF exactly at a frame boundary
    ProtocolError,  // cap violation, EOF mid-frame, or I/O error
    BadMagic,       // magic mismatch — caller MUST exit(2), no resync
};

struct ReadResult {
    ReadStatus status = ReadStatus::ProtocolError;
    Frame frame;         // valid iff status == Ok
    std::string error;   // human-readable detail for stderr logging
};

// Encode a frame to its on-wire byte sequence. Exposed for unit tests.
std::vector<std::uint8_t> encode_frame(const Frame& frame);

// Blocking read of exactly one frame from `fd`, looping over partial reads.
ReadResult read_frame(int fd);

// Blocking write of exactly one frame to `fd`, fully flushed before return.
// This is the ONLY sanctioned path that writes to stdout. Returns true on
// success; false on a short/failed write (I/O error).
bool write_frame(int fd, const Frame& frame);

}  // namespace onecad::protocol
