#!/usr/bin/env python3
"""Cases for resolve-version.sh: one version per run (panel-gha-dispatch-version)."""

import datetime
import os
import pathlib
import re
import subprocess


SCRIPT = pathlib.Path(__file__).with_name("resolve-version.sh")


def resolve(*args, now=None):
    env = dict(os.environ)
    env.pop("RESOLVE_VERSION_NOW", None)
    if now is not None:
        env["RESOLVE_VERSION_NOW"] = now
    result = subprocess.run(
        ["bash", str(SCRIPT), *args],
        check=True,
        env=env,
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

    publish = (
        "--event",
        "workflow_dispatch",
        "--ref",
        "refs/heads/main",
        "--create-release",
        "true",
    )
    # No numeric component may carry a leading zero (SemVer 2.0.0, item 2).
    assert resolve(*publish, now="2026-11-24T15:58:15Z") == "2026.1124.155815"
    assert resolve(*publish, now="2026-03-05T09:30:12Z") == "2026.305.93012"
    assert resolve(*publish, now="2026-01-01T00:00:00Z") == "2026.101.0"
    assert resolve(*publish, now="2026-10-09T00:00:07Z") == "2026.1009.7"

    before = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0)
    version = resolve(*publish)
    after = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0)
    match = re.fullmatch(r"(\d{4})\.([1-9]\d?)(\d{2})\.(0|[1-9]\d{0,5})", version)
    assert match, version
    year, month, day, clock = (int(g) for g in match.groups())
    stamped = datetime.datetime(
        year,
        month,
        day,
        clock // 10000,
        clock // 100 % 100,
        clock % 100,
        tzinfo=datetime.timezone.utc,
    )
    assert before - datetime.timedelta(seconds=1) <= stamped <= after + datetime.timedelta(
        seconds=1
    ), (before, stamped, after)
    print("resolve-version: ok")


if __name__ == "__main__":
    main()
