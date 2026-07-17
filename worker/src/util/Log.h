// Log.h — stderr-only logging for the OneCAD worker.
//
// HARD INVARIANT: the worker's stdout carries protocol frames ONLY. Every
// diagnostic MUST go to stderr. These macros never touch stdout. Do not add a
// stdout sink here. (Grep gate in build-worker / CI forbids printf/std::cout in
// src/ outside the Frame writer.)
#pragma once

#include <cstdarg>
#include <chrono>
#include <cstdio>
#include <ctime>
#include <mutex>
#include <string_view>

namespace onecad::log {

enum class Level { Info, Warn, Error };

inline std::string_view level_name(Level l) {
    switch (l) {
        case Level::Info:  return "INFO";
        case Level::Warn:  return "WARN";
        case Level::Error: return "ERROR";
    }
    return "?";
}

// Serializes concurrent log lines (stdin reader thread + kernel thread both log).
inline std::mutex& log_mutex() {
    static std::mutex m;
    return m;
}

// Millisecond-precision UTC-ish local timestamp: "YYYY-MM-DD HH:MM:SS.mmm".
inline void write_timestamp(char* buf, size_t n) {
    using namespace std::chrono;
    const auto now = system_clock::now();
    const auto secs = time_point_cast<seconds>(now);
    const auto ms = duration_cast<milliseconds>(now - secs).count();
    const std::time_t t = system_clock::to_time_t(now);
    std::tm tm_buf{};
#if defined(_WIN32)
    localtime_s(&tm_buf, &t);
#else
    localtime_r(&t, &tm_buf);
#endif
    char date[20];
    std::strftime(date, sizeof(date), "%Y-%m-%d %H:%M:%S", &tm_buf);
    std::snprintf(buf, n, "%s.%03lld", date, static_cast<long long>(ms));
}

// Single entry point. Always writes to stderr and flushes. printf-style; the
// format(printf) attribute enables compile-time format checking and avoids the
// -Wformat-security warning a non-literal fprintf passthrough would raise.
__attribute__((format(printf, 2, 3))) inline void emit(Level lvl, const char* fmt, ...) {
    std::lock_guard<std::mutex> guard(log_mutex());
    char ts[32];
    write_timestamp(ts, sizeof(ts));
    std::fprintf(stderr, "[%s] %-5s ", ts, level_name(lvl).data());
    std::va_list ap;
    va_start(ap, fmt);
    std::vfprintf(stderr, fmt, ap);
    va_end(ap);
    std::fputc('\n', stderr);
    std::fflush(stderr);
}

}  // namespace onecad::log

// Usage: WLOG_INFO("hello %s", name);  — printf-style, newline appended.
#define WLOG_INFO(...)  ::onecad::log::emit(::onecad::log::Level::Info,  __VA_ARGS__)
#define WLOG_WARN(...)  ::onecad::log::emit(::onecad::log::Level::Warn,  __VA_ARGS__)
#define WLOG_ERROR(...) ::onecad::log::emit(::onecad::log::Level::Error, __VA_ARGS__)
