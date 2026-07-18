# Packaging (M3)

The OneCAD app ships as a Tauri bundle that embeds the C++ OCCT sidecar
(`onecad-worker`) alongside the main executable. This document is the end-to-end
packaging story: how the sidecar is built, how Tauri bundles it, how the worker's
non-system dylibs are folded into the `.app` on macOS, and the clean-Mac
verification checklist that must pass before a release is signed.

macOS (Apple Silicon) is the target platform. Linux `deb` bundling is wired and
smoke-tested (see below), but a packaged Linux build would additionally need
OCCT shipped inside the bundle — out of M3 scope.

## Pipeline overview

```
scripts/build-worker.sh        # build + stage sidecar for the rust-host triple
        │
        ▼
src-tauri/binaries/onecad-worker-<triple>   # what bundle.externalBin consumes
        │
        ▼
bun run tauri build            # bundles the sidecar next to the main executable
        │
        ▼
scripts/bundle-dylibs.sh <app> # macOS: fold worker's dylib closure into the .app
        │
        ▼
codesign / notarize            # sign the whole bundle (placeholder below)
```

## 1. Build + stage the worker

```bash
scripts/build-worker.sh          # Release by default; Debug|Release accepted
```

This configures + builds `worker/` via CMake and copies the result to
`src-tauri/binaries/onecad-worker-<rust-host-triple>` — the exact name Tauri's
`bundle.externalBin` expects (the triple suffix is stripped at install time, so
the bundled binary is plain `onecad-worker`).

## 2. externalBin bundling

`src-tauri/tauri.conf.json` declares the sidecar:

```json
"bundle": {
  "externalBin": ["binaries/onecad-worker"]
}
```

Tauri resolves `binaries/onecad-worker-<triple>` at bundle time and places the
sidecar next to the main `onecad` executable:

- macOS `.app`: `Contents/MacOS/onecad-worker`
- Linux `.deb`: `/usr/bin/onecad-worker` (beside `/usr/bin/onecad`)
- Windows: `onecad-worker.exe` beside `onecad.exe`

### How the app finds the worker

`src-tauri/src/worker/mod.rs::resolve_worker_path` walks a fixed precedence chain
so the same binary works in dev and packaged environments:

1. `ONECAD_WORKER_PATH` env override — if it names an existing file (tests, CI);
2. `<exe_dir>/onecad-worker` (`.exe` on Windows) — the bundled sidecar location;
3. `../worker/build/onecad-worker` — the dev-tree fallback (relative to `src-tauri/`).

If none exist the app boots with `PendingBackend` rather than spawning a missing
binary. The resolution core (`resolve_worker_path_from`) is a pure function with
unit tests covering each rung.

## 3. macOS dylib bundling

The worker links Homebrew OCCT (`/opt/homebrew/lib`), which is absent on a clean
Mac. `scripts/bundle-dylibs.sh` makes the `.app` self-contained:

```bash
scripts/bundle-dylibs.sh path/to/OneCAD.app
```

It:

1. locates the worker in `Contents/MacOS/` (`onecad-worker` or `onecad-worker-*`);
2. computes the transitive non-system dylib closure via `otool -L`
   (deps under `/opt/homebrew` or `/usr/local`; `/usr/lib` + `/System` skipped);
3. copies each dylib into `Contents/Frameworks/`;
4. rewrites install names to `@rpath/<name>` (`install_name_tool -change` on the
   worker + every copied dylib; `-id @rpath/<name>` on each copied dylib);
5. adds `@executable_path/../Frameworks` to the worker's rpath (tolerating an
   already-present rpath);
6. ad-hoc re-signs (`codesign --force --sign -`) every Mach-O it touched, since
   `install_name_tool` invalidates signatures.

It is macOS-only (refuses to run otherwise) and idempotent — a second run is a
no-op because the worker's deps then resolve through `@rpath` and no longer look
bundleable.

The worker's rpath (`/opt/homebrew/lib` + `@executable_path/../Frameworks`) is
already baked in by `worker/CMakeLists.txt`, so the in-tree dev binary finds
Homebrew OCCT while the bundled binary finds `Contents/Frameworks/`.

## 4. Sign + notarize (placeholder)

After `bundle-dylibs.sh`, sign the whole bundle with a real Developer ID and
notarize:

```bash
# Placeholder — fill in the real Developer ID + credentials at release time.
codesign --force --deep --options runtime \
  --sign "Developer ID Application: <TEAM>" path/to/OneCAD.app
xcrun notarytool submit path/to/OneCAD.dmg \
  --apple-id <APPLE_ID> --team-id <TEAM_ID> --password <APP_PASSWORD> --wait
xcrun stapler staple path/to/OneCAD.app
```

Tauri can perform signing during `tauri build` when `APPLE_CERTIFICATE` /
`APPLE_SIGNING_IDENTITY` env vars are set; the `bundle-dylibs.sh` re-sign of the
sidecar must happen **before** the outer bundle is signed.

## 5. Clean-Mac verification checklist (deferred, run on a Mac)

This is the M3 gate that cannot be verified on Linux — it must run on a Mac
**without Homebrew** (or with Homebrew's OCCT uninstalled) to prove the bundle is
self-contained. Run each step and expect the stated result:

1. **Bundle + sign on the build Mac**

   ```bash
   scripts/build-worker.sh Release
   bun run tauri build            # produces the .app + .dmg
   scripts/bundle-dylibs.sh src-tauri/target/release/bundle/macos/onecad.app
   # then codesign + notarize per §4
   ```

2. **Copy the signed `.app` to a clean Mac** with no Homebrew on `PATH` and no
   `/opt/homebrew/lib` OCCT. Verify Gatekeeper accepts it:

   ```bash
   spctl --assess --type execute --verbose /Applications/onecad.app   # → accepted
   ```

3. **Run the bundled worker's self-test from inside the `.app`** — this exercises
   `hello` + a PlaneGCS `SketchUpsert` in-process and returns exit 0 only if the
   bundled OCCT/PlaneGCS dylibs load through `@rpath/../Frameworks`:

   ```bash
   /Applications/onecad.app/Contents/MacOS/onecad-worker --selftest
   echo $?    # expect 0
   ```

   A non-zero exit or a dyld "image not found" message means a dylib is missing
   from `Contents/Frameworks/` — re-run `bundle-dylibs.sh` and re-check the
   `otool -L` closure.

4. **Confirm the dylib closure resolves via rpath** (no lingering
   `/opt/homebrew` references):

   ```bash
   otool -L /Applications/onecad.app/Contents/MacOS/onecad-worker \
     | grep -E '/opt/homebrew|/usr/local'    # expect no output
   ```

5. **STEP-export stdout hygiene** — the worker's protocol lane owns `stdout`; any
   OCCT/STEP-writer chatter leaking to `stdout` would corrupt the OCW1 frame
   stream. The worker already guards this: `worker/tests/test_wp6_exportstep.cpp`
   (ctest `wp6_exportstep`) captures fd 1 across `handle_export_step()` and
   asserts **zero bytes** hit the real `stdout` (STEP diagnostics are redirected
   to `stderr` by `main.cpp`'s `redirect_occt_to_stderr()`). On the clean Mac,
   drive an extrude → STEP export through the running app and confirm the export
   succeeds while the frame stream stays intact (protocol frames only on
   `stdout`; any OCCT noise appears on `stderr`).

Until all five pass on a clean Mac, the M3 packaging gate stays open.

## Linux `deb` smoke (CI-friendly, non-gating)

The bundling path is exercised on Linux to prove `externalBin` staging works end
to end:

```bash
scripts/build-worker.sh Release          # stages the linux-gnu sidecar
bun run tauri build --bundles deb        # release compile + .deb under target/release/bundle/deb/
dpkg-deb -c src-tauri/target/release/bundle/deb/*.deb | grep onecad-worker
```

The `.deb` contains `onecad-worker` next to `onecad` in `/usr/bin`. Tauri also
copies the sidecar to `src-tauri/target/release/onecad-worker`; a Linux self-test
needs the conda OCCT on the loader path:

```bash
LD_LIBRARY_PATH=/opt/occt793/lib src-tauri/target/release/onecad-worker --selftest
echo $?    # expect 0
```

A packaged Linux build would need those OCCT libs bundled (as `bundle-dylibs.sh`
does for macOS) — out of M3 scope, since macOS is the target platform.
