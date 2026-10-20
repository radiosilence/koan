"""Whether a change to the shared build inputs reaches the apps.

`Cargo.lock`, `justfile` and `mise.toml` are edited for every part of the
workspace, so a change to one says nothing on its own about the Apple or
Android builds. This prints `apple=true|false` and `android=true|false` for
the change between two commits:

- Cargo.lock: a package koan-ffi depends on, directly or not, was added,
  removed or changed. The lock lists every target's dependencies, so the
  Apple-only crates are in that closure too.
- justfile: a recipe the Apple jobs run, or one those recipes run, changed;
  or anything outside a recipe (variables, settings), which any recipe may
  read. The Android job runs Gradle, not just.
- mise.toml: a tool the Android job installs, or anything outside [tools].
  The Apple jobs do not use mise.

    python3 native_inputs.py BASE HEAD
"""

import re
import subprocess
import sys
import tomllib

APPLE_ROOTS = {
    "macos-ffi",
    "macos-dmg",
    "macos-verify",
    "ios-typecheck",
    "tv-typecheck",
    "tv-build",
    "ios-testflight",
    "tv-testflight",
}
ANDROID_TOOLS = {"rust", "java", "cargo:cargo-ndk", "just"}


def show(rev, path):
    out = subprocess.run(["git", "show", f"{rev}:{path}"], capture_output=True, text=True)
    return out.stdout if out.returncode == 0 else ""


def lock_reaches_ffi(old, new):
    def packages(text):
        return tomllib.loads(text).get("package", []) if text else []

    def key(p):
        return (p["name"], p["version"])

    before = {key(p): p for p in packages(old)}
    after = {key(p): p for p in packages(new)}
    changed = {k[0] for k in before.keys() ^ after.keys()}
    changed |= {k[0] for k in before.keys() & after.keys() if before[k] != after[k]}
    if not changed:
        return False
    graph = {}
    for p in [*before.values(), *after.values()]:
        graph.setdefault(p["name"], set()).update(d.split()[0] for d in p.get("dependencies", []))
    seen, stack = set(), ["koan-ffi"]
    while stack:
        name = stack.pop()
        if name not in seen:
            seen.add(name)
            stack.extend(graph.get(name, ()))
    return bool(changed & seen)


RECIPE = re.compile(r"^@?([A-Za-z_][\w-]*)[^:=]*:(?!=)(.*)$")


def recipes(text):
    """Each top-level unit of a justfile: a recipe by name, anything else by its line."""
    units, current = {}, None
    for line in text.splitlines():
        if line and not line[0].isspace() and not line.startswith("#"):
            m = RECIPE.match(line)
            current = ("recipe", m.group(1)) if m else ("other", line)
            units[current] = [line]
        elif current and line.strip() and not line.startswith("#"):
            units[current].append(line)
    return {k: "\n".join(v) for k, v in units.items()}


def depends(body):
    header, _, rest = body.partition("\n")
    deps = set(re.findall(r"\(?([a-z][\w-]*)", RECIPE.match(header).group(2)))
    deps |= set(re.findall(r"\bjust\s+(?:--?\S+\s+)*([a-z][\w-]*)", rest))
    return deps


def justfile_reaches_apple(old, new):
    before, after = recipes(old), recipes(new)
    changed = {k for k in before.keys() | after.keys() if before.get(k) != after.get(k)}
    if any(kind == "other" for kind, _ in changed):
        return True
    graph = {}
    for units in (before, after):
        for (kind, name), body in units.items():
            if kind == "recipe":
                graph.setdefault(name, set()).update(depends(body))
    seen, stack = set(), list(APPLE_ROOTS)
    while stack:
        name = stack.pop()
        if name not in seen:
            seen.add(name)
            stack.extend(graph.get(name, ()))
    return any(name in seen for _, name in changed)


def mise_reaches_android(old, new):
    before = tomllib.loads(old) if old else {}
    after = tomllib.loads(new) if new else {}
    tools_before, tools_after = before.pop("tools", {}), after.pop("tools", {})
    if before != after:
        return True
    return any(tools_before.get(t) != tools_after.get(t) for t in ANDROID_TOOLS)


def main():
    base, head = sys.argv[1:3]
    lock = lock_reaches_ffi(show(base, "Cargo.lock"), show(head, "Cargo.lock"))
    just = justfile_reaches_apple(show(base, "justfile"), show(head, "justfile"))
    mise = mise_reaches_android(show(base, "mise.toml"), show(head, "mise.toml"))
    print(f"apple={str(lock or just).lower()}")
    print(f"android={str(lock or mise).lower()}")


main()
