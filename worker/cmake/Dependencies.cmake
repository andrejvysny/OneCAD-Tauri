# Ported from OneCAD-CPP/CMakeLists.txt @ 2026-07-16
#
# Dependency discovery for the OneCAD C++ sidecar worker.
#
# The worker deliberately does NOT depend on Qt (unlike OneCAD-CPP). It links:
#   - OpenCASCADE (OCCT)  : geometry kernel (required; TKernel proves linkage)
#   - Eigen3              : header-only linear algebra (REQUIRED as of W-WP2 / PlaneGCS)
#   - Boost               : headers only (REQUIRED as of W-WP2 / PlaneGCS)
#
# Eigen3 and Boost back the vendored PlaneGCS solver (third_party/planegcs).
# As of W-WP2 they are REQUIRED (Boost 1.90 + Eigen 5.0.1 present on the
# build machines); PlaneGCS's own CMakeLists find_package()s them too.

# ---------------------------------------------------------------------------
# 1. OpenCASCADE (OCCT)  — REQUIRED
# ---------------------------------------------------------------------------
find_package(OpenCASCADE)
if(NOT OpenCASCADE_FOUND)
    set(POSSIBLE_OCCT_DIRS
        "/opt/homebrew/lib/cmake/opencascade"
        "/usr/local/lib/cmake/opencascade"
        "/opt/homebrew/opt/opencascade/lib/cmake/opencascade"
        "/usr/lib/cmake/opencascade"
        "/usr/lib/x86_64-linux-gnu/cmake/opencascade"
        "/usr/lib/aarch64-linux-gnu/cmake/opencascade"
    )
    foreach(DIR ${POSSIBLE_OCCT_DIRS})
        if(EXISTS ${DIR})
            message(STATUS "Found potential OpenCASCADE config at: ${DIR}")
            set(OpenCASCADE_DIR ${DIR})
            find_package(OpenCASCADE REQUIRED)
            break()
        endif()
    endforeach()
endif()

if(NOT OpenCASCADE_FOUND)
    message(FATAL_ERROR "OpenCASCADE not found. Please install it.")
endif()

# Drop OCCT's Draw/Test harness libraries (TKDraw, TKQADraw, TKTopTest,
# TK*DRAW*, TKDCAF, TKOpenGlTest, ...). They are Tcl-based tooling that must
# never be linked into the app: TKDraw's static Tcl initialization installs an
# atexit handler that REWRITES the process exit status to 0, silently turning
# every non-abort test failure (return 1 / std::exit(1)) into a pass.
#
# For the worker this is doubly load-bearing: worker exit codes ARE protocol
# signals (bad magic -> exit 2, clean shutdown -> exit 0). A rewritten exit
# code would make protocol errors indistinguishable from success.
list(FILTER OpenCASCADE_LIBRARIES EXCLUDE REGEX "Draw|DRAW|Test|TKDCAF")
message(STATUS "OpenCASCADE libraries (harness libs filtered): ${OpenCASCADE_LIBRARIES}")
message(STATUS "OpenCASCADE Version: ${OpenCASCADE_VERSION}")

# ---------------------------------------------------------------------------
# 2. Eigen3  — header-only, REQUIRED (backs PlaneGCS)
# ---------------------------------------------------------------------------
# NOTE (from OneCAD-CPP): Homebrew currently ships Eigen 5.x which is
# API-compatible with Eigen 3.x for this project, but CMake package
# compatibility can reject a strict 3.3 request. Use NO_MODULE, no version.
find_package(Eigen3 REQUIRED NO_MODULE)
message(STATUS "Eigen3 Version: ${Eigen3_VERSION} (target Eigen3::Eigen)")

# ---------------------------------------------------------------------------
# 3. Boost  — headers only, CONFIG mode, REQUIRED (backs PlaneGCS)
# ---------------------------------------------------------------------------
# PlaneGCS needs Boost headers only (no compiled Boost libs). Located via
# CONFIG (Homebrew ships BoostConfig.cmake). PlaneGCS's own CMakeLists also
# find_package(Boost REQUIRED CONFIG)s; this pre-locates it for the tree.
find_package(Boost REQUIRED CONFIG)
message(STATUS "Boost Version: ${Boost_VERSION} (headers: ${Boost_INCLUDE_DIRS})")
