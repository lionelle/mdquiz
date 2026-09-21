#!/usr/bin/env python3
"""Refuse cargo commands that RUN this crate's code without a memory cap.

Building is bounded — `.cargo/config.toml` caps rustc at 4 jobs and a cold
build peaks under 2 GB. Running is not: a `cargo mutants` mutant of the
list-numbering recursion reached 11 GB three times on 2026-09-16/17, and
because swap here is zram-only the kernel OOM killer fired and closed VS Code.

`memcap` (in ~/.local/bin) puts the run in its own cgroup with MemoryMax, so
a runaway dies alone. This hook is here because a note in CLAUDE.md did not
hold: the session read it, agreed, and ran `cargo mutants` bare anyway.
"""

import json
import re
import sys

# Subcommands that compile and then EXECUTE the crate's own code.
RUNS_CODE = re.compile(r"\bcargo(?:\s+\+\S+)?\s+(mutants|test|nextest)\b")
CAPPED = re.compile(r"\b(memcap|systemd-run)\b")
# `--list` only enumerates tests or mutants; it runs none of them.
LISTS_ONLY = re.compile(r"(?<![-\w])--list\b")
# Shell operators that start a new command. A segment is checked on its own, so
# `cargo fmt && cargo test` is judged on the `cargo test` half.
SEPARATORS = re.compile(r"&&|\|\||[;\n|]")


def main() -> int:
    try:
        payload = json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError):
        return 0  # Never block because the hook itself could not parse.

    command = (payload.get("tool_input") or {}).get("command") or ""

    offenders = [
        segment.strip()
        for segment in SEPARATORS.split(command)
        if RUNS_CODE.search(segment)
        and not CAPPED.search(segment)
        and not LISTS_ONLY.search(segment)
    ]
    if not offenders:
        return 0

    reason = (
        "Blocked: this runs mdquiz's own code with no memory cap, and an "
        "uncapped run has taken the whole desktop down three times "
        "(a cargo-mutants mutant reaching ~11 GB, kernel OOM, VS Code "
        "killed with it).\n\n"
        f"Uncapped: {offenders[0]}\n\n"
        "Prefix it with `memcap`, which runs it in its own cgroup at "
        "MemoryMax=6G so a runaway dies by itself:\n"
        "    memcap cargo test --all-features\n"
        "    memcap cargo mutants --jobs 1 --file src/foo.rs\n\n"
        "Raise the cap for one run with MEMCAP=12G if a run legitimately "
        "needs more. Do not work around this by calling the test binary "
        "in target/debug/deps directly."
    )
    print(
        json.dumps(
            {
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": reason,
                }
            }
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
