# solverbench — standalone solver-lane latency gate. Spawns onecad-worker and
# drives the SCHEMA §7.4 gesture protocol over stdio.
add_executable(solverbench main.cpp)
target_link_libraries(solverbench PRIVATE worker_core)

# A fast smoke run keeps the harness exercised in CI without the full 1000-drag
# gate cost (the real gate is run manually via `solverbench --worker <path>`).
add_test(
    NAME solverbench_smoke
    COMMAND solverbench --worker $<TARGET_FILE:onecad-worker> --quick
            --out ${CMAKE_CURRENT_BINARY_DIR}/RESULTS_smoke.md
)
