"""Skip heavy CI only for a nonempty change consisting of blame metadata alone."""

import json
import os
from pathlib import Path
import re
import subprocess


def metadata_only(event_name, event):
    if event_name == "pull_request":
        pr = event.get("pull_request", {})
        before = pr.get("base", {}).get("sha", "")
        after = pr.get("head", {}).get("sha", "")
    elif event_name == "push":
        before, after = event.get("before", ""), event.get("after", "")
    else:
        return False  # Manual runs always exercise the full workflow.
    if not all(re.fullmatch(r"[0-9a-f]{40}", sha) and sha != "0" * 40 for sha in (before, after)):
        return False
    try:
        # PRs describe changes since the common ancestor; a push includes every pushed commit.
        if event_name == "pull_request":
            before = subprocess.check_output(["git", "merge-base", before, after], stderr=subprocess.PIPE).decode().strip()
        changed = subprocess.check_output(
            ["git", "diff", "--no-renames", "--name-only", "-z", before, after, "--"],
            stderr=subprocess.PIPE,
        )
    except subprocess.CalledProcessError:
        return False  # Missing history or an unresolvable range must not bypass CI.
    return changed == b".git-blame-ignore-revs\0"


if __name__ == "__main__":
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
    only_metadata = metadata_only(os.environ["GITHUB_EVENT_NAME"], event)
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        output.write(f"metadata_only={str(only_metadata).lower()}\n")
    print("Only blame metadata changed; heavy steps are skipped." if only_metadata else "Run all normal CI steps.")
