#!/usr/bin/env bash
# Cross-builds the Windows packages (x64 and ARM64) of one commit with cargo-zigbuild, strips
# them, zips the game, splits the zip into 8 MiB parts and writes a README with the sha256s.
# Nothing is published or uploaded: the output folder is the result. Untested on Windows.
#
# Output layout and README text match what package_windows.sh reads: <out>/<sha8>/ (x86_64) and
# <out>/<sha8>-arm64/ (aarch64), each with thedas-app.zip.part-*, thedas.exe and README.txt whose
# "thedas-app.zip sha256 ..." line is parsed and whose first line names the target triple.
#
# Usage: tools/triage/build-windows.sh <sha> [out-root]
# Env:   OUT        output root (default /mnt/project-files/windows-build)
#        WORKTREE   existing checkout of <sha> to build in (default: a throwaway worktree in
#                   $TMPDIR, removed afterwards)
#        TARGET_DIR cargo target dir (default $HOME/target-win; keep it out of the checkouts)
#        NICE       nice level (default 15): builds share the machine with the player
# Needs: cargo-zigbuild, zig, rustup targets x86_64-pc-windows-gnu and
#        aarch64-pc-windows-gnullvm, zip, shasum or sha256sum. A missing resource compiler only
#        costs the exe icon (the build says so). Installs nothing.
set -euo pipefail

sha="${1:?usage: build-windows.sh <sha> [out-dir]}"
out="${2:-${OUT:-/mnt/project-files/windows-build}}"
target_dir="${TARGET_DIR:-$HOME/target-win}"
nice_level="${NICE:-15}"

for tool in cargo cargo-zigbuild zip git; do
  command -v "$tool" >/dev/null || { echo "missing tool: $tool"; exit 1; }
done
# cargo-zigbuild finds zig on the PATH or as the Python module `ziglang`.
command -v zig >/dev/null || python3 -c 'import ziglang' 2>/dev/null \
  || { echo "missing tool: zig (or pip ziglang)"; exit 1; }
sum() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }
strip_exe() { if command -v llvm-strip >/dev/null; then llvm-strip "$1"; else echo "no llvm-strip: leaving $1 unstripped"; fi; }

if [ -n "${WORKTREE:-}" ]; then
  wt="$WORKTREE"
else
  wt="$(mktemp -d "${TMPDIR:-/tmp}/win-build.XXXXXX")"
  git worktree add -q --detach "$wt" "$sha"
  trap 'git worktree remove --force "$wt" >/dev/null 2>&1 || true' EXIT
fi
full=$(git -C "$wt" rev-parse HEAD)
[ "${full:0:${#sha}}" = "$sha" ] || [ "$(git -C "$wt" rev-parse "$sha")" = "$full" ] \
  || { echo "$wt is not at $sha"; exit 1; }

# unrar's C++ includes <PowrProf.h> and <Wbemidl.h>; the mingw headers zig ships are lower case
# and a Linux or macOS checkout has a case-sensitive include lookup, so give it forwarding headers.
shim="$target_dir/include-shim"
mkdir -p "$shim"
printf '#include <powrprof.h>\n' > "$shim/PowrProf.h"
printf '#include <wbemidl.h>\n' > "$shim/Wbemidl.h"
export CXXFLAGS_x86_64_pc_windows_gnu="${CXXFLAGS_x86_64_pc_windows_gnu:-} -I$shim"
export CXXFLAGS_aarch64_pc_windows_gnullvm="${CXXFLAGS_aarch64_pc_windows_gnullvm:-} -I$shim"

mkdir -p "$out"
for arch in "${sha:0:8}:x86_64-pc-windows-gnu" "${sha:0:8}-arm64:aarch64-pc-windows-gnullvm"; do
  name="${arch%%:*}"; triple="${arch#*:}"
  echo "== $name ($triple)"
  (cd "$wt" && CARGO_TARGET_DIR="$target_dir" nice -n "$nice_level" \
     cargo zigbuild --release -p thedas-app -p thedas-cli --target "$triple")
  bin="$target_dir/$triple/release"
  stage="$out/$name"; rm -rf "$stage"; mkdir -p "$stage"
  cp "$bin/thedas-app.exe" "$bin/thedas.exe" "$stage/"
  strip_exe "$stage/thedas-app.exe"; strip_exe "$stage/thedas.exe"
  (cd "$stage" && zip -9 -q thedas-app.zip thedas-app.exe && split -b 8m thedas-app.zip thedas-app.zip.part- )
  {
    echo "OpenThedas Windows build, commit ${full:0:8} ($triple, cross-built on Linux). Untested on Windows."
    echo "thedas-app.exe is the game: join the parts in order into thedas-app.zip and unzip. thedas.exe is the tools."
    echo "thedas-app.zip sha256 $(sum "$stage/thedas-app.zip")"
    echo "thedas-app.exe sha256 $(sum "$stage/thedas-app.exe")"
    echo "thedas.exe sha256 $(sum "$stage/thedas.exe")"
  } > "$stage/README.txt"
  echo "wrote $stage"
done
echo "done: $out/${sha:0:8} and $out/${sha:0:8}-arm64 (nothing uploaded or published)"
