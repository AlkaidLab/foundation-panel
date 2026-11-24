#!/usr/bin/env python3
"""Cases for resolve-version.sh: one version per run (panel-gha-dispatch-version)."""

import datetime
import pathlib
import re
import subprocess


SCRIPT = pathlib.Path(__file__).with_name("resolve-version.sh")


def resolve(*args):
    result = subprocess.run(
        ["bash", str(SCRIPT), *args],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def main():
    assert resolve("--event", "push", "--ref", "refs/tags/v1.0.20") == "1.0.20"
    assert resolve("--event", "workflow_dispatch", "--ref", "refs/tags/v1.0.20") == "1.0.20"
    assert (
        resolve(
            "--event",
            "workflow_dispatch",
            "--ref",
            "refs/heads/main",
            "--create-release",
            "false",
        )
        == "0.0.0-dev"
    )
    assert resolve("--event", "workflow_dispatch", "--ref", "refs/heads/main") == "0.0.0-dev"
    assert resolve("--event", "pull_request", "--ref", "refs/pull/7/merge") == "0.0.0-dev"

    before = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0)
    version = resolve(
        "--event",
        "workflow_dispatch",
        "--ref",
        "refs/heads/main",
        "--create-release",
        "true",
    )
    after = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0)

    assert re.fullmatch(r"\d{4}\.\d{4}\.\d{6}", version), version
    stamped = datetime.datetime.strptime(version, "%Y.%m%d.%H%M%S").replace(
        tzinfo=datetime.timezone.utc
    )
    assert before - datetime.timedelta(seconds=1) <= stamped <= after + datetime.timedelta(
        seconds=1
    ), (before, stamped, after)
    print("resolve-version: ok")


if __name__ == "__main__":
    main()
