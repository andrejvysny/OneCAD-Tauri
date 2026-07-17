cmake_minimum_required(VERSION 3.25)
project(onecad-worker VERSION 0.1.0 LANGUAGES CXX)

# --- Language standard ---
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# Export compile_commands.json for tooling (clangd, etc.)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# --- Dependency discovery (OCCT / Eigen3 / Boost) ---
include(${CMAKE_CURRENT_SOURCE_DIR}/cmake/Dependencies.cmake)

# --- RPATH so the worker finds Homebrew OCCT dylibs when run in-tree, and
#     bundled Frameworks when shipped inside the Tauri app bundle (M3). ---
set(CMAKE_BUILD_RPATH   "/opt/homebrew/lib;@executable_path/../Frameworks")
set(CMAKE_INSTALL_RPATH "/opt/homebrew/lib;@executable_path/../Frameworks")
set(CMAKE_MACOSX_RPATH ON)

# ---------------------------------------------------------------------------
# Vendored PlaneGCS (FreeCAD Sketcher solver) — added BEFORE the first-party
# -Wall -Wextra so PlaneGCS builds quietly, mirroring OneCAD-CPP's root
# CMakeLists (add_subdirectory(third_party/planegcs) precedes its warning
# flags). Directory-scoped add_compile_options only affects targets declared
# after it, so PlaneGCS's own sources never see -Wall -Wextra.
# ---------------------------------------------------------------------------
add_subdirectory(third_party/planegcs)

# Mark PlaneGCS's public headers as SYSTEM for every consumer so warnings from
# GCS.h/Geo.h/Constraints.h do not leak into first-party TUs compiled with
# -Wall -Wextra. Done here (not in the vendored CMakeLists, which is copied
# wholesale) to keep the vendor tree byte-identical to OneCAD-CPP.
get_target_property(_planegcs_inc planegcs INTERFACE_INCLUDE_DIRECTORIES)
if(_planegcs_inc)
    set_target_properties(planegcs PROPERTIES
        INTERFACE_SYSTEM_INCLUDE_DIRECTORIES "${_planegcs_inc}")
endif()

# --- Warnings: first-party code only. No -Werror (per project convention). ---
add_compile_options(-Wall -Wextra)

# ---------------------------------------------------------------------------
# worker_core : STATIC library — everything except main.cpp
# ---------------------------------------------------------------------------
add_library(worker_core STATIC
    src/protocol/Frame.cpp
    src/protocol/Envelope.cpp
    src/protocol/Dispatcher.cpp
    # --- W-WP3b: sketch solver lane (SCHEMA §7.4 verbs) ---
    src/protocol/SolverLane.cpp
    # --- W-WP4: session + transactional ExecutePlan (SCHEMA §7.1/§7.2) ---
    src/util/Hashing.cpp
    src/session/Session.cpp
    src/session/ScratchJob.cpp
    src/session/ShapeMetrics.cpp
    src/session/Signatures.cpp
    src/session/PlanExecutor.cpp
    # --- W-WP5: REAL OCCT ops + ElementMap V2 + tessellation ---
    src/elementmap/ElementMapPartition.cpp
    # --- W-WP6: resolution ladder (scoring / min-cost assignment / orchestration) ---
    src/elementmap/Scoring.cpp
    src/elementmap/Assignment.cpp
    src/elementmap/Ladder.cpp
    src/ops/OpCommon.cpp
    src/ops/ExtrudeOp.cpp
    src/ops/BooleanOp.cpp
    # --- W-WP6: new OCCT ops (Revolve / Fillet / Chamfer) + STEP IO ---
    src/ops/RevolveOp.cpp
    src/ops/FilletChamferOp.cpp
    src/io/ExportStep.cpp
    src/session/ElementIdentity.cpp
    src/tess/Mesh1.cpp
    src/tess/Tessellate.cpp
    src/sketch/WireSketch.cpp
    src/sketch/RegionId.cpp
    # --- W-WP2 kernel port (mirrors OneCAD-CPP src/kernel onecad_kernel lib) ---
    src/kernel/geometry/EdgeChainer.cpp
    src/kernel/modeling/FaceExtrudeProfileBuilder.cpp
    src/kernel/topology/CoplanarFacePatch.cpp
    src/kernel/topology/FacePatchResolver.cpp
    src/kernel/topology/SelectionTopologyResolver.cpp
    # --- W-WP3a: Qt-free sketch entity stack (ported from OneCAD-CPP
    #     src/core/sketch; QString/QUuid/QJson/qDebug stripped). ---
    src/sketch/SketchEntity.cpp
    src/sketch/SketchPoint.cpp
    src/sketch/SketchLine.cpp
    src/sketch/SketchArc.cpp
    src/sketch/SketchCircle.cpp
    src/sketch/SketchEllipse.cpp
    src/sketch/SketchConstraint.cpp
    src/sketch/constraints/Constraints.cpp
    src/sketch/solver/ConstraintSolver.cpp
    src/sketch/solver/SolverAdapter.cpp
    src/sketch/Sketch.cpp
    src/sketch/ConstraintApplicability.cpp    # onecad::ui selection→constraint rules
    # --- W-WP3a: loop stack (W-WP2 copied, now compiled — needs sketch types) ---
    src/loop/AdjacencyGraph.cpp
    src/loop/FaceBuilder.cpp
    src/loop/LoopDetector.cpp
    src/loop/RegionUtils.cpp
    # --- W-WP3a: boolean op (W-WP2 copied, now compiled; include repointed to
    #     worker-local modeling/BooleanMode.h for app::BooleanMode) ---
    src/modeling/BooleanOperation.cpp
)

target_include_directories(worker_core
    PUBLIC
    ${CMAKE_CURRENT_SOURCE_DIR}/src
    ${CMAKE_CURRENT_SOURCE_DIR}/src/kernel  # kernel-internal includes: "elementmap/...", "geometry/..."
    ${CMAKE_CURRENT_SOURCE_DIR}/third_party
    ${OpenCASCADE_INCLUDE_DIR}
)

target_link_libraries(worker_core
    PUBLIC
    ${OpenCASCADE_LIBRARIES}
    Eigen3::Eigen      # REQUIRED as of W-WP2
    Boost::headers     # REQUIRED as of W-WP2
    planegcs           # vendored solver (linked now; used by W-WP3 sketch solver)
)

if(OpenCASCADE_LIBRARY_DIR)
    target_link_directories(worker_core PUBLIC ${OpenCASCADE_LIBRARY_DIR})
endif()

# ---------------------------------------------------------------------------
# onecad-worker : the sidecar executable
# ---------------------------------------------------------------------------
add_executable(onecad-worker src/main.cpp)
target_link_libraries(onecad-worker PRIVATE worker_core)

# ---------------------------------------------------------------------------
# Testing
# ---------------------------------------------------------------------------
enable_testing()
add_subdirectory(tests)
add_subdirectory(tools/harness)
add_subdirectory(tools/solverbench)
