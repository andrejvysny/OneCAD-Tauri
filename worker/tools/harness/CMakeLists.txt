# Standalone protocol harness: spawns onecad-worker and drives it over stdio.
add_executable(worker_harness main.cpp)
target_link_libraries(worker_harness PRIVATE worker_core)
