#!/usr/bin/env python3
"""Generate CHANGELOG.md from git tags and Conventional Commits. No third-party deps.

--next-version prints only the suggested version; --bump writes it to Cargo.toml.
"""
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "CHANGELOG.md"
CARGO = ROOT / "Cargo.toml"

GROUPS = [
    ("feat", "Features"),
    ("fix", "Bug Fixes"),
    ("perf", "Performance Improvements"),
    ("refactor", "Refactor"),
    ("docs", "Documentation"),
    ("style", "Styles"),
    ("test", "Tests"),
    ("build", "Build"),
    ("ci", "CI"),
]
SKIP_TYPES = {"chore", "revert"}
COMMIT_RE = re.compile(
    r"^(?P<type>[a-zA-Z]+)(?:\((?P<scope>[^)]+)\))?(?P<breaking>!)?: (?P<desc>.+)$"
)


def git(*args: str) -> str:
    result = subprocess.run(
        ["git", *args], cwd=ROOT, capture_output=True, text=True, check=True
    )
    return result.stdout.strip()


def commits(rev: str) -> list[str]:
    out = git("log", "--no-merges", "--format=%h%x09%s", rev)
    return [line for line in out.splitlines() if line]


VERSION_RE = re.compile(r"^v?(\d+)\.(\d+)\.(\d+)$")


def latest_version(tags: list[str]) -> tuple[int, int, int]:
    for tag in tags:
        match = VERSION_RE.match(tag)
        if match:
            return tuple(int(part) for part in match.groups())  # type: ignore[return-value]
    # No valid tag yet: fall back to the crate version.
    manifest = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'^version\s*=\s*"(\d+)\.(\d+)\.(\d+)"', manifest, re.M)
    if not match:
        raise SystemExit("cannot determine current version from tags or Cargo.toml")
    return tuple(int(part) for part in match.groups())  # type: ignore[return-value]


def next_version(entries: list[str], tags: list[str]) -> str:
    major, minor, patch = latest_version(tags)
    bump = 0  # 1=patch, 2=minor, 3=major
    for entry in entries:
        _, _, subject = entry.partition("\t")
        match = COMMIT_RE.match(subject)
        if not match or match.group("type").lower() in SKIP_TYPES:
            continue
        kind = match.group("type").lower()
        if match.group("breaking") or "BREAKING CHANGE" in subject:
            bump = 3
        elif kind == "feat" and bump < 2:
            bump = 2
        elif kind in {"fix", "perf"} and bump < 1:
            bump = 1
    if bump == 3:
        major, minor, patch = major + 1, 0, 0
    elif bump == 2:
        minor, patch = minor + 1, 0
    elif bump == 1:
        patch += 1
    return f"{major}.{minor}.{patch}"


def bump_version(version: str) -> str:
    """Write version into the [package] version line of Cargo.toml."""
    text = CARGO.read_text(encoding="utf-8")
    pattern = re.compile(
        r'(?m)^(\[package\][\s\S]*?^version\s*=\s*)"[^"]+"'
    )
    updated, count = pattern.subn(r'\g<1>"' + version + '"', text, count=1)
    if count != 1:
        raise SystemExit("could not find [package] version in Cargo.toml")
    CARGO.write_text(updated, encoding="utf-8")
    return version


def render_section(title: str, date: str | None, entries: list[str]) -> list[str]:
    lines = [f"## {title}" + (f" - {date}" if date else ""), ""]
    grouped: dict[str, list[tuple[str, str]]] = {
        name: [] for _, name in GROUPS
    }
    grouped["Other"] = []

    for entry in entries:
        short_hash, _, subject = entry.partition("\t")
        match = COMMIT_RE.match(subject)
        if not match:
            grouped["Other"].append((short_hash, subject))
            continue
        kind = match.group("type").lower()
        if kind in SKIP_TYPES:
            continue
        group = next((name for key, name in GROUPS if key == kind), "Other")
        scope = match.group("scope")
        label = f"**{scope}:** " if scope else ""
        desc = match.group("desc")
        if match.group("breaking"):
            desc = "**BREAKING:** " + desc
        grouped[group].append((short_hash, label + desc))

    wrote = False
    for name in [n for _, n in GROUPS] + ["Other"]:
        if not grouped[name]:
            continue
        wrote = True
        lines += [f"### {name}", ""]
        lines += [f"- {text} (`{h}`)" for h, text in grouped[name]]
        lines.append("")
    if not wrote:
        lines += ["_No notable changes._", ""]
    return lines


def main() -> None:
    tags = git("tag", "--sort=-version:refname").splitlines()
    sections: list[tuple[str, str | None, list[str]]] = []

    unreleased = commits(f"{tags[0]}..HEAD") if tags else commits("HEAD")
    version = next_version(unreleased, tags)
    if "--next-version" in sys.argv:
        print(version)
        return
    if "--bump" in sys.argv:
        bump_version(version)
        print(f"Set Cargo.toml version to {version}")
    if unreleased:
        sections.append(("Unreleased", None, unreleased))

    for i, tag in enumerate(tags):
        older = tags[i + 1] if i + 1 < len(tags) else None
        rev = f"{older}..{tag}" if older else tag
        sections.append(
            (tag.lstrip("v"), git("log", "-1", "--format=%cs", tag), commits(rev))
        )

    lines = ["# Changelog", "", "All notable changes to Guardian.", ""]
    for title, date, entries in sections:
        lines += render_section(title, date, entries)
    OUTPUT.write_text("\n".join(lines).rstrip() + "\n", encoding="utf-8")
    print(
        f"Wrote {OUTPUT.relative_to(ROOT)} ({len(sections)} sections); "
        f"next version: v{version}"
    )


if __name__ == "__main__":
    main()
