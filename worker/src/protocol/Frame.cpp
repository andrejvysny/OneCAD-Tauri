#include "protocol/Frame.h"

#include <cerrno>
#include <cstring>
#include <unistd.h>

#include "util/LittleEndian.h"

namespace onecad::protocol {
namespace {

// Result of a low-level fixed-size read.
enum class FillStatus {
    Complete,     // all n bytes read
    EofAtStart,   // EOF before any byte (clean boundary)
    EofMidway,    // EOF after >=1 byte (protocol loss)
    IoError,      // unrecoverable read() error
};

// Read exactly n bytes into buf, looping over partial reads and retrying EINTR.
FillStatus read_fully(int fd, std::uint8_t* buf, std::size_t n) {
    std::size_t got = 0;
    while (got < n) {
        const ssize_t r = ::read(fd, buf + got, n - got);
        if (r == 0) {
            return got == 0 ? FillStatus::EofAtStart : FillStatus::EofMidway;
        }
        if (r < 0) {
            if (errno == EINTR) continue;
            return FillStatus::IoError;
        }
        got += static_cast<std::size_t>(r);
    }
    return FillStatus::Complete;
}

// Write exactly n bytes, looping over partial writes and retrying EINTR.
bool write_fully(int fd, const std::uint8_t* buf, std::size_t n) {
    std::size_t sent = 0;
    while (sent < n) {
        const ssize_t w = ::write(fd, buf + sent, n - sent);
        if (w < 0) {
            if (errno == EINTR) continue;
            return false;
        }
        sent += static_cast<std::size_t>(w);
    }
    return true;
}

std::uint32_t load_u32_le(const std::uint8_t* p) {
    return static_cast<std::uint32_t>(p[0]) | (static_cast<std::uint32_t>(p[1]) << 8) |
           (static_cast<std::uint32_t>(p[2]) << 16) | (static_cast<std::uint32_t>(p[3]) << 24);
}

void store_u32_le(std::uint8_t* p, std::uint32_t v) {
    p[0] = static_cast<std::uint8_t>(v & 0xFF);
    p[1] = static_cast<std::uint8_t>((v >> 8) & 0xFF);
    p[2] = static_cast<std::uint8_t>((v >> 16) & 0xFF);
    p[3] = static_cast<std::uint8_t>((v >> 24) & 0xFF);
}

}  // namespace

std::vector<std::uint8_t> encode_frame(const Frame& frame) {
    const auto json_len = static_cast<std::uint32_t>(frame.json.size());
    const auto bin_len = static_cast<std::uint32_t>(frame.bin.size());

    std::vector<std::uint8_t> out;
    out.reserve(kHeaderLen + frame.json.size() + frame.bin.size());
    out.insert(out.end(), kMagic, kMagic + 4);

    std::uint8_t lens[8];
    store_u32_le(lens, json_len);
    store_u32_le(lens + 4, bin_len);
    out.insert(out.end(), lens, lens + 8);

    out.insert(out.end(), frame.json.begin(), frame.json.end());
    out.insert(out.end(), frame.bin.begin(), frame.bin.end());
    return out;
}

ReadResult read_frame(int fd) {
    ReadResult res;

    std::uint8_t header[kHeaderLen];
    switch (read_fully(fd, header, kHeaderLen)) {
        case FillStatus::Complete:
            break;
        case FillStatus::EofAtStart:
            res.status = ReadStatus::Eof;
            return res;
        case FillStatus::EofMidway:
            res.status = ReadStatus::ProtocolError;
            res.error = "EOF mid-header (protocol loss)";
            return res;
        case FillStatus::IoError:
            res.status = ReadStatus::ProtocolError;
            res.error = std::string("read() error on header: ") + std::strerror(errno);
            return res;
    }

    if (std::memcmp(header, kMagic, 4) != 0) {
        res.status = ReadStatus::BadMagic;
        res.error = "bad frame magic (no resync; exit)";
        return res;
    }

    const std::uint32_t json_len = load_u32_le(header + 4);
    const std::uint32_t bin_len = load_u32_le(header + 8);

    if (json_len > kMaxJsonLen) {
        res.status = ReadStatus::ProtocolError;
        res.error = "jsonLen exceeds 16 MiB cap";
        return res;
    }
    if (bin_len > kMaxBinLen) {
        res.status = ReadStatus::ProtocolError;
        res.error = "binLen exceeds 1 GiB cap";
        return res;
    }

    res.frame.json.resize(json_len);
    if (json_len > 0) {
        auto* p = reinterpret_cast<std::uint8_t*>(res.frame.json.data());
        const FillStatus s = read_fully(fd, p, json_len);
        if (s != FillStatus::Complete) {
            res.status = ReadStatus::ProtocolError;
            res.error = "EOF/error reading JSON body (protocol loss)";
            return res;
        }
    }

    res.frame.bin.resize(bin_len);
    if (bin_len > 0) {
        const FillStatus s = read_fully(fd, res.frame.bin.data(), bin_len);
        if (s != FillStatus::Complete) {
            res.status = ReadStatus::ProtocolError;
            res.error = "EOF/error reading binary body (protocol loss)";
            return res;
        }
    }

    res.status = ReadStatus::Ok;
    return res;
}

bool write_frame(int fd, const Frame& frame) {
    // Buffer the whole frame, then flush with a single looped write. Assembling
    // first keeps the frame contiguous so the peer never sees a torn header.
    const std::vector<std::uint8_t> buf = encode_frame(frame);
    return write_fully(fd, buf.data(), buf.size());
}

}  // namespace onecad::protocol
