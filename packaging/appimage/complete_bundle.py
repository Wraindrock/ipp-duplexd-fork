#!/usr/bin/env python3
"""Copy the libraries an AppDir still borrows from the build host into it.

linuxdeploy leaves out a handful of libraries it considers "always present"
(fontconfig, freetype, harfbuzz, fribidi, ...). An AppImage that relies on them
is not self-contained: it only runs on a machine whose base system happens to
carry the same versions, and the GUI fails at the loader before it can say
anything useful.

This walks the `ldd` closure of every ELF file in the bundle and copies what is
still resolved outside it, except a small set of libraries that must come from
the host: glibc, the compiler runtime, and the OpenGL driver stack (a bundled
libGL would bypass the actual graphics driver).

    complete_bundle.py <AppDir>          # copy what is missing
    complete_bundle.py --check <AppDir>  # exit 1 if something is missing
"""

import os
import re
import shutil
import subprocess
import sys

# These must match the machine: glibc, the dynamic loader, the compiler runtime
# and the GL/driver stack. Everything else is fair game to bundle.
ALLOWED_PREFIXES = (
    "ld-linux",
    "libc.so",
    "libm.so",
    "libmvec.so",
    "libpthread.so",
    "libdl.so",
    "librt.so",
    "libresolv.so",
    "libutil.so",
    "libanl.so",
    "libnsl.so",
    "libnss_",
    "libcrypt.so",
    "libthread_db.so",
    "libgcc_s.so",
    "libstdc++.so",
    "libGL.so",
    "libGLX",
    "libGLdispatch.so",
    "libOpenGL.so",
    "libEGL.so",
    "libGLESv",
    "libdrm.so",
    "libgbm.so",
    "libglapi.so",
    "libvulkan.so",
)

LDD_LINE = re.compile(r"^\s+(\S+) => (\S+) \(0x")


def allowed(name):
    return name.startswith(ALLOWED_PREFIXES)


def bundled_elfs(appdir):
    for root, _dirs, files in os.walk(appdir):
        if root.endswith(("apprun-hooks", "share")):
            continue
        for name in files:
            path = os.path.join(root, name)
            if os.path.islink(path) or not os.path.isfile(path):
                continue
            yield path


def dependencies(path, appdir):
    """Returns (inside_appdir, outside) sonames, or (None, None) for non-ELF."""
    library_path = os.path.join(appdir, "usr", "lib")
    env = dict(os.environ)
    env["LD_LIBRARY_PATH"] = library_path + (":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")
    try:
        out = subprocess.run(
            ["ldd", path], env=env, capture_output=True, text=True, timeout=60
        )
    except (OSError, subprocess.SubprocessError):
        return None, None
    if out.returncode != 0 and "not a dynamic executable" in out.stdout + out.stderr:
        return None, None
    inside, outside, missing = set(), {}, set()
    for line in out.stdout.splitlines():
        match = LDD_LINE.match(line)
        if match:
            soname, resolved = match.group(1), match.group(2)
            real = os.path.realpath(resolved)
            if real.startswith(os.path.realpath(os.path.join(appdir, "usr", "lib"))):
                inside.add(soname)
            elif not allowed(os.path.basename(soname)):
                outside[soname] = real
            continue
        if "=> not found" in line:
            soname = line.split("=>")[0].strip()
            if not allowed(os.path.basename(soname)):
                missing.add(soname)
    return inside, (outside, missing)


def main(argv):
    check = False
    args = list(argv[1:])
    if args and args[0] == "--check":
        check = True
        args = args[1:]
    if len(args) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    appdir = args[0]
    library_dir = os.path.join(appdir, "usr", "lib")

    copied, unresolved = {}, set()
    for _round in range(10):
        changed = False
        for path in bundled_elfs(appdir):
            inside, result = dependencies(path, appdir)
            if inside is None:
                continue
            outside, missing = result
            unresolved |= missing
            for soname, real in outside.items():
                if soname in copied:
                    continue
                if check:
                    copied[soname] = real
                    continue
                destination = os.path.join(library_dir, soname)
                shutil.copy2(real, destination)
                copied[soname] = real
                changed = True
        if not changed:
            break

    for soname in sorted(unresolved):
        print(f"{soname} is not available on this build host", file=sys.stderr)

    if check:
        for soname, real in sorted(copied.items()):
            print(f"{soname} is still borrowed from {real}", file=sys.stderr)
        return 1 if copied or unresolved else 0

    if copied:
        print(f"copied {len(copied)} libraries the bundle still borrowed")
    return 1 if unresolved else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
