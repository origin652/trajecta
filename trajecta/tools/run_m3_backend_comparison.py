#!/usr/bin/env python3
"""Full-field backend comparison via frozen expected matrix + Rust adjudicator.

Requires jsonschema (Draft 2020-12). Missing dependency → incomplete (exit 2).
Requires ALL three families. Exit:
  0 passed, 1 failed (hard gate), 2 incomplete.
"""
from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "target" / "m3-comparison"
REPORTS = OUT / "reports"
REGISTRY = ROOT / "testdata" / "M3_TOLERANCES.v1.json"
SCHEMA = ROOT / "testdata" / "M3_COMPARISON_REPORT.schema.json"
MATRIX = ROOT / "testdata" / "M3_BACKEND_EXPECTED_MATRIX.v1.json"
REQUIRED_FAMILIES = ("era5_pressure", "era5_hybrid", "cfsr_pressure")


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def require_jsonschema():
    try:
        from jsonschema import Draft202012Validator  # noqa: F401
    except ImportError:
        print(
            "INCOMPLETE: jsonschema package required (Draft 2020-12); no weak fallback",
            file=sys.stderr,
        )
        raise SystemExit(2)


def validate_report_schema(path: Path) -> None:
    from jsonschema import Draft202012Validator

    schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
    instance = json.loads(path.read_text(encoding="utf-8"))
    errors = sorted(Draft202012Validator(schema).iter_errors(instance), key=lambda e: list(e.path))
    if errors:
        msgs = [f"{list(e.path)}: {e.message}" for e in errors[:12]]
        raise SystemExit(f"report schema invalid {path}: {msgs}")


def run(cmd: list[str], env: dict | None = None) -> subprocess.CompletedProcess[str]:
    print("+", " ".join(cmd), flush=True)
    return subprocess.run(
        cmd,
        cwd=ROOT,
        text=True,
        capture_output=True,
        env=env,
        check=False,
        encoding="utf-8",
        errors="replace",
    )


def native_env(*, with_eccodes: bool) -> dict[str, str]:
    sys.path.insert(0, str(ROOT / "tools"))
    from measure_rust_native_diff import native_env as ne  # type: ignore

    return ne(with_eccodes=with_eccodes)


def verify_file(entry: dict) -> Path:
    rel = entry["relative_path"]
    path = ROOT / rel
    if not path.is_file():
        raise SystemExit(f"INCOMPLETE: missing frozen file {rel}")
    size = path.stat().st_size
    digest = sha256_file(path)
    if size != int(entry["size"]):
        raise SystemExit(
            f"INCOMPLETE: size mismatch {rel}: disk={size} frozen={entry['size']}"
        )
    if digest != entry["sha256"]:
        raise SystemExit(
            f"INCOMPLETE: sha256 mismatch {rel}: disk={digest} frozen={entry['sha256']}"
        )
    return path


def main() -> int:
    require_jsonschema()
    started = datetime.now(timezone.utc).isoformat()
    run_id = str(uuid.uuid4())
    REPORTS.mkdir(parents=True, exist_ok=True)
    for old in REPORTS.glob("*_backend_report.json"):
        old.unlink()

    if not MATRIX.is_file():
        print("INCOMPLETE: missing", MATRIX, file=sys.stderr)
        return 2
    matrix = json.loads(MATRIX.read_text(encoding="utf-8"))
    registry = json.loads(REGISTRY.read_text(encoding="utf-8"))
    matrix_registry_version = matrix.get("registry_version")
    registry_version = registry.get("registry_version")
    if matrix_registry_version != registry_version:
        print(
            "INCOMPLETE: matrix registry_version mismatch: "
            f"matrix={matrix_registry_version!r} registry={registry_version!r}",
            file=sys.stderr,
        )
        return 2

    env_nc = native_env(with_eccodes=False)
    env_ec = native_env(with_eccodes=True)

    summary: dict = {
        "run_id": run_id,
        "started_at_utc": started,
        "registry_sha256": sha256_file(REGISTRY),
        "matrix_sha256": sha256_file(MATRIX),
        "required_families": list(REQUIRED_FAMILIES),
        "reports": [],
        "missing_inputs": [],
    }

    reports: dict[str, Path] = {}

    for fam in REQUIRED_FAMILIES:
        cfg = matrix["families"].get(fam)
        if not cfg:
            print(f"INCOMPLETE: family {fam} absent from matrix", file=sys.stderr)
            return 2
        paths = []
        for entry in cfg["files"]:
            paths.append(verify_file(entry))

        out = REPORTS / f"{fam}_backend_report.json"
        if out.exists():
            out.unlink()

        feature = "native-eccodes" if cfg["format"] == "grib" else "native-netcdf"
        env = env_ec if cfg["format"] == "grib" else env_nc
        cmd = [
            "cargo",
            "run",
            "--offline",
            "-p",
            "trajecta-met",
            "--features",
            feature,
            "--example",
            "adjudicate_backend_fields",
            "--",
            "--format",
            cfg["format"],
            "--family",
            fam,
            "--variant",
            cfg["variant"],
            "--registry",
            str(REGISTRY),
            "--report-id",
            cfg["report_id"],
            "--out",
            str(out),
            "--manifest",
            str(MATRIX),
            "--manifest-family",
            fam,
        ]
        proc = run(cmd, env=env)
        print((proc.stdout or "")[-2000:], flush=True)
        if proc.stderr:
            print((proc.stderr or "")[-3000:], file=sys.stderr, flush=True)
        if not out.is_file():
            print(
                f"INCOMPLETE: no report for {fam} rc={proc.returncode}",
                file=sys.stderr,
            )
            return 2
        validate_report_schema(out)
        # freshness
        age = time.time() - out.stat().st_mtime
        if age > 3600:
            print(f"INCOMPLETE: stale report {out} age={age}s", file=sys.stderr)
            return 2
        reports[fam] = out

    statuses = []
    for fam in REQUIRED_FAMILIES:
        path = reports[fam]
        st = path.stat()
        rep = json.loads(path.read_text(encoding="utf-8"))
        summary["reports"].append(
            {
                "family": fam,
                "path": path.as_posix(),
                "sha256": sha256_file(path),
                "size": st.st_size,
                "mtime_unix": st.st_mtime,
                "status": rep.get("status"),
                "report_id": rep.get("report_id"),
                "compared_total": sum(
                    r.get("compared_count", 0) for r in rep.get("results", [])
                ),
                "blockers": rep.get("blockers", [])[:20],
            }
        )
        statuses.append(rep.get("status"))

    if any(s is None for s in statuses) or len(summary["reports"]) != 3:
        summary["status"] = "incomplete"
        rc = 2
    elif any(s in ("incomplete", "unvalidated") for s in statuses):
        summary["status"] = "incomplete"
        rc = 2
    elif any(s == "failed" for s in statuses):
        summary["status"] = "failed"
        rc = 1
    elif all(s == "passed" for s in statuses):
        summary["status"] = "passed"
        rc = 0
    else:
        summary["status"] = "incomplete"
        rc = 2

    summary["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
    (OUT / "SUMMARY.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, indent=2))
    return rc


if __name__ == "__main__":
    raise SystemExit(main())
