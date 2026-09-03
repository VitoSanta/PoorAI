#!/usr/bin/env python3
"""Regenerates the roadmap's status table from docs/milestones.json.

Milestone state was asserted in the gate table, in the status table and in a
dozen document tails, and they disagreed: the same milestone was declared
complete and in progress in one file. One source, generated into one place, is
the only shape where that cannot happen again.

Run it after editing docs/milestones.json. It rewrites only the block between
the markers and refuses to touch anything else.
"""
import json
import pathlib
import sys

START = "<!-- generated:milestones -->"
END = "<!-- /generated:milestones -->"


def table(manifest: dict) -> str:
    rows = [
        "| Milestone | Status | Evidence recorded | What remains before advancement |",
        "|---|---|---|---|",
    ]
    for milestone in manifest["milestones"]:
        rows.append(
            "| {id} {name} | **{status}** | {evidence} | {remaining} |".format(**milestone)
        )
    return "\n".join(rows)


def main() -> int:
    root = pathlib.Path(__file__).resolve().parent.parent
    manifest = json.loads((root / "docs/milestones.json").read_text())
    roadmap = root / "docs/roadmap.md"
    text = roadmap.read_text()
    if START not in text or END not in text:
        print(f"{roadmap}: generated block markers are missing", file=sys.stderr)
        return 1
    head, rest = text.split(START, 1)
    _, tail = rest.split(END, 1)
    generated = (
        f"{START}\n"
        f"<!-- Generated from docs/milestones.json by scripts/milestones.py. Edit the manifest, not this table. -->\n\n"
        f"{table(manifest)}\n\n"
        f"Campaign evidence: {manifest['campaign_evidence']}.\n"
        f"{END}"
    )
    updated = head + generated + tail
    if updated == text:
        print("roadmap already matches the manifest")
        return 0
    roadmap.write_text(updated)
    print("roadmap status table regenerated")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
