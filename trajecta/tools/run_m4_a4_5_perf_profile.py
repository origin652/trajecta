"""Run one frozen WSL M4-A4.5 CPU sampling profile without changing numerics."""
from __future__ import annotations

import argparse
import json
import subprocess
import time
from pathlib import Path
from typing import Any

from run_m4_a4_real_matrix import (
    FAMILY,
    ROOT,
    TEST_FILTER,
    TEST_NAME,
    TEST_PACKAGE,
    environment,
    host_identity,
    next_attempt_directory,
    sha256,
    to_wsl_path,
    utc_now,
    validate_cell,
    write_json,
)


def run_profile(
    attempt: Path,
    *,
    target_dir: str,
    distro: str | None,
) -> tuple[int, str, str]:
    artifact = to_wsl_path(attempt)
    values = environment(
        {},
        mode="attribution",
        direction="forward",
        particles=1_000,
        workers=4,
        artifact_dir=artifact,
        observe_performance=False,
    )
    exports = "\n".join(f"export {key}={json.dumps(value)}" for key, value in values.items())
    script = f"""set -euo pipefail
export PATH="/root/.cargo/bin:/usr/local/bin:/usr/bin:/bin"
export CARGO_TARGET_DIR={json.dumps(target_dir)}
export RUSTFLAGS="-C force-frame-pointers=yes -C debuginfo=1"
{exports}
cd /mnt/e/flexpart/trajecta
uname -a
rustc -Vv
cargo -V
perf version
cat /proc/sys/kernel/perf_event_paranoid || true
git rev-parse HEAD
git status --short
cargo test --offline --release -p {TEST_PACKAGE} --test {TEST_NAME} --no-run
binary=$(find "$CARGO_TARGET_DIR/release/deps" -maxdepth 1 -type f -name 'm4_a4_real_perf-*' -perm -111 | sort | tail -n 1)
test -n "$binary"
printf '%s\n' "$binary" > {json.dumps(artifact + '/profile-binary-path.txt')}
sha256sum "$binary" > {json.dumps(artifact + '/profile-binary-sha256.txt')}
perf record -F 199 -g --call-graph dwarf -o {json.dumps(artifact + '/perf.data')} -- \
  "$binary" {TEST_FILTER} --ignored --nocapture --exact
perf report --stdio --percent-limit 0.25 -i {json.dumps(artifact + '/perf.data')} \
  > {json.dumps(artifact + '/perf-report-inclusive.txt')}
perf report --stdio --no-children --percent-limit 0.25 -i {json.dumps(artifact + '/perf.data')} \
  > {json.dumps(artifact + '/perf-report-self.txt')}
perf script -i {json.dumps(artifact + '/perf.data')} \
  > {json.dumps(artifact + '/perf-script.txt')}
if command -v stackcollapse-perf.pl >/dev/null 2>&1 && command -v flamegraph.pl >/dev/null 2>&1; then
  stackcollapse-perf.pl {json.dumps(artifact + '/perf-script.txt')} \
    > {json.dumps(artifact + '/perf-folded.txt')}
  flamegraph.pl --title 'Trajecta M4-A4.5 WSL ERA5-hybrid 1k w4' \
    {json.dumps(artifact + '/perf-folded.txt')} \
    > {json.dumps(artifact + '/flamegraph.svg')}
else
  printf '%s\n' 'stackcollapse-perf.pl and/or flamegraph.pl unavailable; perf.data and text reports retained' \
    > {json.dumps(artifact + '/FLAMEGRAPH_BLOCKER.txt')}
fi
"""
    runner = attempt / "wsl-perf-run.sh"
    runner.write_bytes(script.replace("\r\n", "\n").replace("\r", "\n").encode("utf-8"))
    command = ["wsl.exe"] + (["-d", distro] if distro else []) + ["--", "bash", to_wsl_path(runner)]
    process = subprocess.run(
        command,
        cwd=ROOT,
        text=True,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
    )
    return process.returncode, process.stdout or "", process.stderr or ""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--artifact-root",
        type=Path,
        default=ROOT / "target" / "m4-a4.5" / "profiles",
    )
    parser.add_argument("--wsl-target-dir", default="/tmp/trajecta-m4-a4.5-profile-target")
    parser.add_argument("--wsl-distro")
    args = parser.parse_args()

    cell = "wsl__era5-hybrid__forward__p1000__w4__perf"
    attempt = next_attempt_directory(args.artifact_root, cell)
    attempt.mkdir(parents=True, exist_ok=False)
    write_json(attempt / "host-identity.json", host_identity("wsl-perf"))
    started = time.monotonic()
    started_utc = utc_now()
    returncode, stdout, stderr = run_profile(
        attempt,
        target_dir=args.wsl_target_dir,
        distro=args.wsl_distro,
    )
    (attempt / "stdout.log").write_text(stdout, encoding="utf-8", errors="replace")
    (attempt / "stderr.log").write_text(stderr, encoding="utf-8", errors="replace")

    validation = validate_cell(
        attempt,
        direction="forward",
        particles=1_000,
        workers=4,
        mode="attribution",
        returncode=returncode,
        require_performance_attribution=False,
    )
    files = {
        name: {
            "present": (attempt / name).is_file(),
            "size": (attempt / name).stat().st_size if (attempt / name).is_file() else None,
            "sha256": sha256(attempt / name),
        }
        for name in (
            "perf.data",
            "perf-report-inclusive.txt",
            "perf-report-self.txt",
            "perf-script.txt",
            "perf-folded.txt",
            "flamegraph.svg",
            "FLAMEGRAPH_BLOCKER.txt",
            "profile-binary-sha256.txt",
        )
    }
    profile_valid = all(
        files[name]["present"] and (files[name]["size"] or 0) > 0
        for name in (
            "perf.data",
            "perf-report-inclusive.txt",
            "perf-report-self.txt",
            "perf-script.txt",
            "profile-binary-sha256.txt",
        )
    )
    result: dict[str, Any] = {
        "schema_version": "trajecta.m4-a4.5-perf-profile-run/v1",
        "implemented": True,
        "executed": True,
        "status": "passed" if validation["passed"] and profile_valid else "failed",
        "started_utc": started_utc,
        "finished_utc": utc_now(),
        "elapsed_seconds": time.monotonic() - started,
        "returncode": returncode,
        "artifact_dir": str(attempt),
        "flamegraph_status": (
            "produced" if files["flamegraph.svg"]["present"] else "external_blocked"
        ),
        "files": files,
        "run_validation": validation,
    }
    write_json(attempt / "M4_A4_5_PERF_PROFILE_RESULT.json", result)
    print(json.dumps({"status": result["status"], "artifact_dir": str(attempt)}))
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
