#!/usr/bin/env python3
"""Fail if the server could build against a crate that is not on the allowlist.

Checks, all from files on disk (no cargo, no network; review V6 F11, IP20):

1. Every external crate named in any Cargo.toml is listed in server/deps.allow,
   and every allowlisted name is used.
2. Every crate dependency is a local path or `workspace = true`, so no crate
   can name a git URL or another registry.
3. Every Cargo.lock package with a `source` is allowlisted by name and comes
   from the crates.io index (which config.toml replaces with the distro
   directory). With deps.allow empty, that means any `source` fails.
4. No `[patch]` or `[replace]` in any Cargo.toml or in server/.cargo/config.toml.
5. server/.cargo/config.toml keeps `[net] offline = true` and
   `[source.crates-io] replace-with = "distro"`, and `distro` is a
   `directory` source. Those lines are what make rule 3 hold for builds.
6. No crate has a build script (`crates/*/build.rs`, or a `build = "..."` key)
   unless its name is in BUILD_RS_ALLOW.

Standard library only.
"""
import pathlib
import sys
import tomllib

SERVER = pathlib.Path(__file__).resolve().parent.parent / "server"
DEP_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")
# Lock sources an allowlisted crate may come from. The distro directory
# registry stands in for crates.io, so Cargo.lock records the crates.io index.
LOCK_SOURCES = ("registry+https://github.com/rust-lang/crates.io-index",)
# Crates allowed a build script, by package directory name. Empty: the server
# is std-only and has no build step.
BUILD_RS_ALLOW = frozenset()


def load(path):
    with open(path, "rb") as f:
        return tomllib.load(f)


def check(server):
    """Return (errors, summary) for the server tree rooted at `server`."""
    errors = []
    top = server.parent

    def rel(p):
        return p.relative_to(top).as_posix()

    allow = {
        line.strip()
        for line in (server / "deps.allow").read_text().splitlines()
        if line.strip() and not line.strip().startswith("#")
    }

    root_path = server / "Cargo.toml"
    root = load(root_path)
    workspace_deps = root.get("workspace", {}).get("dependencies", {})
    for name, spec in workspace_deps.items():
        if name not in allow:
            errors.append(f"{rel(root_path)}: '{name}' is not in deps.allow")
        if isinstance(spec, dict) and any(k in spec for k in ("git", "registry", "path")):
            errors.append(f"{rel(root_path)}: '{name}' must come from the distro registry")

    def no_overrides(path, data):
        for key in ("patch", "replace"):
            if key in data:
                errors.append(f"{rel(path)}: [{key}] is not allowed")

    no_overrides(root_path, root)

    for manifest in sorted(server.glob("crates/*/Cargo.toml")):
        data = load(manifest)
        no_overrides(manifest, data)
        crate = manifest.parent.name
        tables = [data.get(t, {}) for t in DEP_TABLES]
        for target in data.get("target", {}).values():
            tables += [target.get(t, {}) for t in DEP_TABLES]
        for table in tables:
            for name, spec in table.items():
                if isinstance(spec, dict) and "path" in spec:
                    continue
                if isinstance(spec, dict) and spec.get("workspace") is True:
                    if name not in workspace_deps:
                        errors.append(f"{rel(manifest)}: '{name}' is not in [workspace.dependencies]")
                    continue
                errors.append(f"{rel(manifest)}: '{name}' must be a path or `workspace = true` dependency")
        build = data.get("package", {}).get("build")
        if build not in (None, False) and crate not in BUILD_RS_ALLOW:
            errors.append(f"{rel(manifest)}: `build = {build!r}` is not in BUILD_RS_ALLOW")
        script = manifest.parent / "build.rs"
        if script.exists() and crate not in BUILD_RS_ALLOW and build is not False:
            errors.append(f"{rel(script)}: build script is not in BUILD_RS_ALLOW")

    for name in sorted(allow - set(workspace_deps)):
        errors.append(f"{rel(server / 'deps.allow')}: '{name}' is allowed but unused; remove it")

    lock_path = server / "Cargo.lock"
    lock = load(lock_path)
    external = []
    for pkg in lock.get("package", []):
        if "source" not in pkg:
            continue
        name, source = pkg.get("name", "?"), pkg["source"]
        external.append(name)
        if name not in allow:
            errors.append(f"{rel(lock_path)}: '{name}' from {source} is not in deps.allow")
        elif not source.startswith(LOCK_SOURCES):
            errors.append(f"{rel(lock_path)}: '{name}' comes from {source}, not the distro registry")

    cfg_path = server / ".cargo" / "config.toml"
    if not cfg_path.exists():
        errors.append(f"{rel(cfg_path)}: missing; builds would reach crates.io")
    else:
        cfg = load(cfg_path)
        no_overrides(cfg_path, cfg)
        if cfg.get("net", {}).get("offline") is not True:
            errors.append(f"{rel(cfg_path)}: [net] offline = true is missing")
        sources = cfg.get("source", {})
        if sources.get("crates-io", {}).get("replace-with") != "distro":
            errors.append(f'{rel(cfg_path)}: [source.crates-io] replace-with = "distro" is missing')
        distro = sources.get("distro", {})
        if set(distro) != {"directory"}:
            errors.append(f"{rel(cfg_path)}: [source.distro] must be exactly one `directory` key")

    summary = (f"deps: {len(workspace_deps)} direct, {len(external)} external in Cargo.lock"
               + (f": {' '.join(sorted(external))}" if external else ""))
    return errors, summary


def main():
    errors, summary = check(SERVER)
    print(summary)
    for e in errors:
        print(f"deps: FAIL {e}", file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
