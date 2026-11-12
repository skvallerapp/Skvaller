#!/usr/bin/env python3
"""Lists the direct dependencies that have a newer release.

For each crate the workspace depends on, it compares the version in Cargo.lock with
the newest stable release on crates.io:

- "update" means the Cargo.toml requirement already allows it: `cargo update` gets it.
- "breaking" means a new major (or 0.x minor) release: the requirement needs editing.

For the git dependencies on GitHub (the Blitz fork), it shows how many commits the
pinned rev is behind the upstream repo's main branch.

Usage: scripts/check-deps.py  (exits 1 when something is outdated)
Needs Python 3.9+, cargo and network access; no extra packages.
"""

import json
import re
import subprocess
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
USER_AGENT = "skvaller-check-deps"
# The fork's upstream, to see what the pinned rev is missing.
UPSTREAMS = {"SinderC/blitz": "DioxusLabs/blitz"}


def fetch_json_lines(url):
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=30) as resp:
        return [json.loads(line) for line in resp.read().decode().splitlines() if line]


def parse_version(text):
    """Returns (major, minor, patch, is_prerelease), ignoring build metadata."""
    core, _, pre = text.split("+")[0].partition("-")
    nums = [int(n) for n in core.split(".")]
    return (*nums, bool(pre))


def compatible(a, b):
    """Cargo's caret rule: same leftmost nonzero component."""
    if a[0] != b[0]:
        return False
    if a[0] != 0:
        return True
    if a[1] != b[1]:
        return False
    return a[1] != 0 or a[2] == b[2]


def index_url(name):
    name = name.lower()
    if len(name) <= 2:
        prefix = str(len(name))
    elif len(name) == 3:
        prefix = f"3/{name[0]}"
    else:
        prefix = f"{name[:2]}/{name[2:4]}"
    return f"https://index.crates.io/{prefix}/{name}"


def latest_stable(name):
    versions = [
        parse_version(e["vers"])
        for e in fetch_json_lines(index_url(name))
        if not e["yanked"]
    ]
    stable = [v for v in versions if not v[3]] or versions
    return max(stable)


def direct_dependencies():
    """Yields (crate name, locked version, source) for each direct dependency."""
    meta = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--locked"], cwd=ROOT
        )
    )
    packages = {p["id"]: p for p in meta["packages"]}
    members = set(meta["workspace_members"])
    seen = set()
    for node in meta["resolve"]["nodes"]:
        if node["id"] not in members:
            continue
        for dep in node["deps"]:
            pkg = packages[dep["pkg"]]
            if pkg["id"] in members or pkg["id"] in seen:
                continue
            seen.add(pkg["id"])
            yield pkg["name"], pkg["version"], pkg["source"] or ""


def git_status(source):
    """Returns a line about a GitHub git dependency, or None."""
    m = re.match(r"git\+https://github\.com/([^/?#]+/[^/?#]+?)(?:\.git)?[?#].*#(\w+)$", source)
    if not m:
        return None
    repo, rev = m.groups()
    upstream = UPSTREAMS.get(repo, repo)
    head_repo = repo.split("/")[0] + ":" + rev if upstream != repo else rev
    url = f"https://api.github.com/repos/{upstream}/compare/main...{head_repo}"
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=30) as resp:
        data = json.load(resp)
    return f"{repo}@{rev[:8]}: {data['behind_by']} commits behind {upstream} main, {data['ahead_by']} ahead"


def main():
    outdated = []
    git_lines = []
    for name, locked, source in sorted(direct_dependencies()):
        if source.startswith("git+"):
            git_lines.append(git_status(source) or f"{name}: {source} (not checked)")
            continue
        if not source.startswith("registry+"):
            continue
        current = parse_version(locked)
        newest = latest_stable(name)
        if newest[:3] > current[:3]:
            kind = "update" if compatible(current, newest) else "breaking"
            outdated.append((name, locked, ".".join(map(str, newest[:3])), kind))

    if outdated:
        width = max(len(o[0]) for o in outdated)
        for name, locked, newest, kind in outdated:
            print(f"{name:<{width}}  {locked:>10} -> {newest:<10}  {kind}")
    else:
        print("All crates.io dependencies are on their newest release.")
    for line in git_lines:
        print(line)
    return 1 if outdated else 0


if __name__ == "__main__":
    sys.exit(main())
