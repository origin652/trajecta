#!/usr/bin/env python3
"""Build and verify one deterministic M5-A4 product archive."""

from __future__ import annotations

import argparse
from contextlib import nullcontext
import ctypes
import gzip
import hashlib
import json
import os
import platform
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile
from pathlib import Path, PurePosixPath
from typing import Any, Iterable


ROOT = Path(__file__).resolve().parents[1]
FEATURES = ["native-eccodes", "native-netcdf"]
FEATURE_ARGUMENT = ",".join(f"trajecta-met/{feature}" for feature in FEATURES)
REQUIRED_NATIVE_COMPONENTS = {"ecCodes", "netCDF-C", "HDF5"}
MANIFEST_NAME = "BUILD-MANIFEST.json"
SBOM_NAME = "SBOM.cdx.json"
LICENSE_INVENTORY_NAME = "THIRD-PARTY-LICENSES.json"
BUILD_RESULT_NAME = "M5_A4_PACKAGE_BUILD_RESULT.json"
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
VERSION_PATTERN = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$")


class PackageError(RuntimeError):
    """A stable product-package contract failure."""


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def file_identity(path: Path, relative: str) -> dict[str, Any]:
    return {"path": relative, "sha256": sha256(path), "bytes": path.stat().st_size}


def write_json(path: Path, value: object) -> None:
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
        newline="\n",
    )


def command_output(arguments: list[str], *, cwd: Path = ROOT, env: dict[str, str] | None = None) -> str:
    return subprocess.check_output(
        arguments,
        cwd=cwd,
        env=env,
        text=True,
        stderr=subprocess.STDOUT,
    ).strip()


def run_command(arguments: list[str], *, cwd: Path = ROOT, env: dict[str, str] | None = None) -> None:
    subprocess.run(arguments, cwd=cwd, env=env, check=True)


def normalized_relative(path: str) -> str:
    if "\\" in path:
        raise PackageError(f"package path uses a backslash: {path}")
    pure = PurePosixPath(path)
    if path in {"", "."} or pure.is_absolute() or ".." in pure.parts:
        raise PackageError(f"package path is not contained and normalized: {path}")
    return pure.as_posix()


def parse_porcelain_paths(raw: bytes, root_prefix: str = "") -> list[str]:
    prefix = root_prefix.strip("/")

    def relative_path(value: bytes) -> str:
        normalized = normalized_relative(os.fsdecode(value).replace("\\", "/"))
        if not prefix:
            return normalized
        marker = prefix + "/"
        if not normalized.startswith(marker):
            raise PackageError(
                f"git porcelain path is outside the package source root: {normalized}"
            )
        return normalized[len(marker) :]

    entries = raw.split(b"\0")
    paths: set[str] = set()
    index = 0
    while index < len(entries):
        entry = entries[index]
        index += 1
        if not entry:
            continue
        if len(entry) < 4 or entry[2:3] != b" ":
            raise PackageError(f"invalid git porcelain entry: {entry!r}")
        status = entry[:2].decode("ascii", errors="strict")
        paths.add(relative_path(entry[3:]))
        if "R" in status or "C" in status:
            if index >= len(entries) or not entries[index]:
                raise PackageError("git porcelain rename/copy entry is truncated")
            paths.add(relative_path(entries[index]))
            index += 1
    return sorted(paths)


def source_tree_identity() -> dict[str, Any]:
    head = command_output(["git", "rev-parse", "HEAD"])
    listing = subprocess.check_output(
        [
            "git",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            ".",
        ],
        cwd=ROOT,
    )
    paths = [os.fsdecode(item) for item in listing.split(b"\0") if item]
    digest = hashlib.sha256()
    included: list[str] = []
    root = ROOT.resolve()
    for relative in sorted(paths):
        normalized = normalized_relative(relative.replace("\\", "/"))
        path = (ROOT / relative).resolve()
        try:
            path.relative_to(root)
        except ValueError as error:
            raise PackageError(f"source path escaped repository root: {relative}") from error
        if not path.is_file():
            continue
        encoded = normalized.encode("utf-8")
        digest.update(len(encoded).to_bytes(8, "big"))
        digest.update(encoded)
        digest.update(path.stat().st_size.to_bytes(8, "big"))
        with path.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        included.append(normalized)
    repository_root = Path(command_output(["git", "rev-parse", "--show-toplevel"]))
    source_prefix = ROOT.resolve().relative_to(repository_root.resolve()).as_posix()
    status = subprocess.check_output(
        [
            "git",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            ".",
        ],
        cwd=ROOT,
    )
    dirty_paths = parse_porcelain_paths(status, source_prefix)
    return {
        "git_head": head,
        "source_tree_sha256": digest.hexdigest(),
        "file_count": len(included),
        "dirty_paths": dirty_paths,
    }


def workspace_version() -> str:
    document = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    version = document["workspace"]["package"]["version"]
    if not isinstance(version, str) or VERSION_PATTERN.fullmatch(version) is None:
        raise PackageError(f"workspace package version is invalid: {version!r}")
    return version


def host_platform_contract() -> dict[str, str]:
    machine = platform.machine().lower()
    if machine not in {"amd64", "x86_64"}:
        raise PackageError(f"M5-A4 requires x86_64, got {platform.machine()}")
    if sys.platform == "win32":
        return {
            "label": "windows-x86_64",
            "target_triple": "x86_64-pc-windows-gnu",
            "minimum_os": "Windows x64",
            "archive_format": "zip",
            "binary_source": "trajecta-cli.exe",
            "binary_product": "trajecta.exe",
        }
    if sys.platform.startswith("linux"):
        return {
            "label": "linux-x86_64",
            "target_triple": "x86_64-unknown-linux-gnu",
            "minimum_os": "Ubuntu 24.04",
            "archive_format": "tar.gz",
            "binary_source": "trajecta-cli",
            "binary_product": "trajecta",
        }
    raise PackageError(f"unsupported M5-A4 host platform: {sys.platform}")


def parse_native_component(value: str) -> dict[str, str]:
    parts = value.split("|", 3)
    if len(parts) != 4 or any(not part.strip() for part in parts):
        raise argparse.ArgumentTypeError(
            "native component must be NAME|VERSION|LICENSE|SOURCE"
        )
    name, version, license_expression, source = (part.strip() for part in parts)
    return {
        "name": name,
        "version": version,
        "license": license_expression,
        "source": source,
    }


def validate_native_components(components: list[dict[str, str]]) -> list[dict[str, str]]:
    names = [component["name"] for component in components]
    if len(names) != len(set(names)):
        raise PackageError("duplicate native component name")
    if set(names) != REQUIRED_NATIVE_COMPONENTS:
        raise PackageError(
            f"native component set must be {sorted(REQUIRED_NATIVE_COMPONENTS)}, got {sorted(names)}"
        )
    return sorted(components, key=lambda component: component["name"])


def cargo_environment(target_dir: Path, source_date_epoch: int, platform_contract: dict[str, str]) -> dict[str, str]:
    environment = os.environ.copy()
    environment["CARGO_TARGET_DIR"] = str(target_dir)
    environment["SOURCE_DATE_EPOCH"] = str(source_date_epoch)
    if platform_contract["label"] == "linux-x86_64":
        rpath = "-C link-arg=-Wl,--disable-new-dtags,-rpath,$ORIGIN/lib"
        prior = environment.get("RUSTFLAGS", "").strip()
        environment["RUSTFLAGS"] = f"{prior} {rpath}".strip()
    return environment


def build_release_binary(target_dir: Path, source_date_epoch: int, platform_contract: dict[str, str]) -> tuple[Path, dict[str, str]]:
    environment = cargo_environment(target_dir, source_date_epoch, platform_contract)
    rustc_details = command_output(["rustc", "-vV"])
    host = next(
        (line.split(":", 1)[1].strip() for line in rustc_details.splitlines() if line.startswith("host:")),
        None,
    )
    if host != platform_contract["target_triple"]:
        raise PackageError(
            f"host-native package requires rustc host {platform_contract['target_triple']}, got {host}"
        )
    command = [
        "cargo",
        "build",
        "--offline",
        "--locked",
        "--release",
        "--package",
        "trajecta-cli",
        "--features",
        FEATURE_ARGUMENT,
    ]
    run_command(command, env=environment)
    binary = (
        target_dir / "release" / platform_contract["binary_source"]
    )
    if not binary.is_file():
        raise PackageError(f"release binary was not produced: {binary}")
    return binary, environment


def cargo_metadata(platform_contract: dict[str, str], environment: dict[str, str]) -> dict[str, Any]:
    output = command_output(
        [
            "cargo",
            "metadata",
            "--offline",
            "--locked",
            "--format-version",
            "1",
            "--filter-platform",
            platform_contract["target_triple"],
            "--features",
            FEATURE_ARGUMENT,
        ],
        env=environment,
    )
    return json.loads(output)


def dependency_closure(metadata: dict[str, Any]) -> tuple[list[dict[str, Any]], dict[str, list[str]]]:
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    roots = [package["id"] for package in metadata["packages"] if package["name"] == "trajecta-cli"]
    if len(roots) != 1:
        raise PackageError("cargo metadata did not contain exactly one trajecta-cli package")
    included: set[str] = set()
    pending = roots[:]
    edges: dict[str, list[str]] = {}
    while pending:
        package_id = pending.pop()
        if package_id in included:
            continue
        included.add(package_id)
        node = nodes[package_id]
        dependencies: list[str] = []
        for dependency in node["deps"]:
            kinds = dependency.get("dep_kinds", [])
            if kinds and all(kind.get("kind") == "dev" for kind in kinds):
                continue
            dependency_id = dependency["pkg"]
            dependencies.append(dependency_id)
            pending.append(dependency_id)
        edges[package_id] = sorted(set(dependencies))
    selected = sorted(
        (packages[package_id] for package_id in included),
        key=lambda package: (package["name"], package["version"], package.get("source") or ""),
    )
    return selected, edges


def package_reference(package: dict[str, Any]) -> str:
    source = package.get("source") or "workspace"
    source_tag = hashlib.sha256(source.encode("utf-8")).hexdigest()[:12]
    return f"pkg:cargo/{package['name']}@{package['version']}?source={source_tag}"


def cargo_license(package: dict[str, Any]) -> str:
    expression = package.get("license")
    if expression:
        return expression
    license_file = package.get("license_file")
    if license_file:
        return f"LicenseRef-File-{Path(license_file).name}"
    raise PackageError(f"dependency has no license metadata: {package['name']} {package['version']}")


def write_supply_chain_files(
    stage: Path,
    packages: list[dict[str, Any]],
    edges: dict[str, list[str]],
    native_components: list[dict[str, str]],
    version: str,
) -> None:
    references = {package["id"]: package_reference(package) for package in packages}
    components: list[dict[str, Any]] = []
    inventory: list[dict[str, Any]] = []
    for package in packages:
        license_expression = cargo_license(package)
        first_party = package["name"].startswith("trajecta-")
        reference = references[package["id"]]
        component = {
            "type": "application" if package["name"] == "trajecta-cli" else "library",
            "bom-ref": reference,
            "name": package["name"],
            "version": package["version"],
            "purl": f"pkg:cargo/{package['name']}@{package['version']}",
            "licenses": [{"expression": license_expression}],
            "properties": [
                {"name": "trajecta:component-kind", "value": "first-party" if first_party else "third-party"},
                {"name": "trajecta:source", "value": package.get("source") or "workspace"},
            ],
        }
        components.append(component)
        inventory.append(
            {
                "ecosystem": "cargo",
                "name": package["name"],
                "version": package["version"],
                "license": license_expression,
                "source": package.get("source") or "workspace",
                "first_party": first_party,
            }
        )
    for native in native_components:
        reference = f"pkg:generic/{native['name']}@{native['version']}"
        components.append(
            {
                "type": "library",
                "bom-ref": reference,
                "name": native["name"],
                "version": native["version"],
                "licenses": [{"expression": native["license"]}],
                "externalReferences": [{"type": "website", "url": native["source"]}],
                "properties": [{"name": "trajecta:component-kind", "value": "native-runtime"}],
            }
        )
        inventory.append(
            {
                "ecosystem": "native",
                "name": native["name"],
                "version": native["version"],
                "license": native["license"],
                "source": native["source"],
                "first_party": False,
            }
        )
    dependencies = []
    included_ids = set(references)
    for package in packages:
        dependencies.append(
            {
                "ref": references[package["id"]],
                "dependsOn": sorted(
                    references[dependency]
                    for dependency in edges.get(package["id"], [])
                    if dependency in included_ids
                ),
            }
        )
    application_ref = next(
        references[package["id"]] for package in packages if package["name"] == "trajecta-cli"
    )
    application_dependencies = next(
        entry["dependsOn"] for entry in dependencies if entry["ref"] == application_ref
    )
    application_dependencies.extend(
        f"pkg:generic/{native['name']}@{native['version']}" for native in native_components
    )
    application_dependencies.sort()
    write_json(
        stage / SBOM_NAME,
        {
            "bomFormat": "CycloneDX",
            "specVersion": "1.5",
            "version": 1,
            "metadata": {
                "component": {
                    "type": "application",
                    "name": "trajecta",
                    "version": version,
                    "licenses": [{"expression": "MIT"}],
                }
            },
            "components": sorted(components, key=lambda item: item["bom-ref"]),
            "dependencies": sorted(dependencies, key=lambda item: item["ref"]),
        },
    )
    write_json(
        stage / LICENSE_INVENTORY_NAME,
        {
            "schema_version": "trajecta.third-party-license-inventory/v1",
            "product": "trajecta",
            "version": version,
            "components": sorted(
                inventory,
                key=lambda item: (item["ecosystem"], item["name"], item["version"]),
            ),
        },
    )


WINDOWS_SYSTEM_DLLS = {
    "advapi32.dll",
    "bcrypt.dll",
    "crypt32.dll",
    "gdi32.dll",
    "iphlpapi.dll",
    "kernel32.dll",
    "msvcrt.dll",
    "ntdll.dll",
    "ole32.dll",
    "oleaut32.dll",
    "rpcrt4.dll",
    "secur32.dll",
    "shell32.dll",
    "shlwapi.dll",
    "user32.dll",
    "userenv.dll",
    "ucrtbase.dll",
    "version.dll",
    "winhttp.dll",
    "winmm.dll",
    "ws2_32.dll",
}


def windows_imports(path: Path) -> list[str]:
    objdump = shutil.which("objdump")
    if objdump is None:
        raise PackageError("objdump is required to collect Windows runtime DLLs")
    output = command_output([objdump, "-p", str(path)], cwd=path.parent)
    names = []
    for line in output.splitlines():
        marker = "DLL Name:"
        if marker in line:
            names.append(line.split(marker, 1)[1].strip())
    return sorted(set(names), key=str.casefold)


def find_case_insensitive(name: str, directories: list[Path]) -> Path | None:
    folded = name.casefold()
    for directory in directories:
        if not directory.is_dir():
            continue
        direct = directory / name
        if direct.is_file():
            return direct
        for candidate in directory.iterdir():
            if candidate.is_file() and candidate.name.casefold() == folded:
                return candidate
    return None


def windows_system_dll(name: str) -> bool:
    folded = name.casefold()
    if folded in WINDOWS_SYSTEM_DLLS or folded.startswith(("api-ms-win-", "ext-ms-win-")):
        return True
    system_root = os.environ.get("SystemRoot")
    return system_root is not None and (Path(system_root) / "System32" / name).is_file()


def collect_windows_libraries(binary: Path, stage: Path, directories: list[Path]) -> list[str]:
    copied: dict[str, Path] = {}
    pending = [binary]
    while pending:
        current = pending.pop()
        for name in windows_imports(current):
            folded = name.casefold()
            if windows_system_dll(name):
                continue
            if folded in copied:
                continue
            source = find_case_insensitive(name, directories)
            if source is None:
                raise PackageError(f"required Windows runtime DLL was not found: {name}")
            destination = stage / source.name
            if destination.exists() and sha256(destination) != sha256(source):
                raise PackageError(f"runtime DLL name collision: {source.name}")
            if not destination.exists():
                shutil.copy2(source, destination)
            copied[folded] = destination
            pending.append(destination)
    names = sorted(path.name for path in copied.values())
    folded_names = [name.casefold() for name in names]
    if not any("eccodes" in name for name in folded_names):
        raise PackageError("Windows package did not collect an ecCodes runtime DLL")
    if not any("netcdf" in name for name in folded_names):
        raise PackageError("Windows package did not collect a netCDF runtime DLL")
    return names


LINUX_SYSTEM_PREFIXES = (
    "ld-linux",
    "libc.so",
    "libdl.so",
    "libm.so",
    "libpthread.so",
    "librt.so",
    "libresolv.so",
    "linux-vdso",
)


def linux_dependencies(path: Path) -> dict[str, Path]:
    output = command_output(["ldd", str(path)], cwd=path.parent)
    dependencies: dict[str, Path] = {}
    for line in output.splitlines():
        stripped = line.strip()
        if "=> not found" in stripped:
            raise PackageError(f"unresolved Linux runtime dependency: {stripped}")
        if "=>" in stripped:
            name, remainder = stripped.split("=>", 1)
            resolved = remainder.strip().split(" ", 1)[0]
        else:
            fields = stripped.split(" ", 1)
            if not fields or not fields[0].startswith("/"):
                continue
            resolved = fields[0]
            name = Path(resolved).name
        resolved_path = Path(resolved)
        if resolved_path.is_file():
            dependencies[name.strip()] = resolved_path
    return dependencies


def collect_linux_libraries(binary: Path, stage: Path) -> list[str]:
    library_root = stage / "lib"
    library_root.mkdir()
    copied: dict[str, Path] = {}
    pending = [binary]
    while pending:
        current = pending.pop()
        for name, source in linux_dependencies(current).items():
            if name.startswith(LINUX_SYSTEM_PREFIXES):
                continue
            if name in copied:
                continue
            destination = library_root / name
            shutil.copy2(source.resolve(), destination)
            copied[name] = destination
            pending.append(destination)
    names = sorted(f"lib/{name}" for name in copied)
    lowered = [name.lower() for name in names]
    if not any("eccodes" in name for name in lowered):
        raise PackageError("Linux package did not collect an ecCodes shared library")
    if not any("netcdf" in name for name in lowered):
        raise PackageError("Linux package did not collect a netCDF shared library")
    return names


def native_library_path(
    stage: Path,
    native_files: list[str],
    marker: str,
    *,
    excluded: tuple[str, ...] = (),
) -> Path:
    candidates = []
    for relative in native_files:
        name = Path(relative).name.casefold()
        if marker in name and not any(item in name for item in excluded):
            candidates.append(stage / relative)
    if not candidates:
        raise PackageError(f"native runtime library for {marker} was not packaged")
    return min(candidates, key=lambda path: (len(path.name), path.name.casefold()))


def probe_native_versions(stage: Path, native_files: list[str]) -> dict[str, str]:
    stage = stage.resolve()
    dll_scope = os.add_dll_directory(str(stage)) if sys.platform == "win32" else nullcontext()
    with dll_scope:
        eccodes = ctypes.CDLL(
            str(native_library_path(stage, native_files, "eccodes", excluded=("memfs",)))
        )
        eccodes.codes_get_api_version.restype = ctypes.c_long
        encoded = int(eccodes.codes_get_api_version())
        if encoded <= 0:
            raise PackageError(f"ecCodes returned invalid API version: {encoded}")
        eccodes_version = f"{encoded // 10_000}.{encoded // 100 % 100}.{encoded % 100}"

        netcdf = ctypes.CDLL(str(native_library_path(stage, native_files, "netcdf")))
        netcdf.nc_inq_libvers.restype = ctypes.c_char_p
        netcdf_text = netcdf.nc_inq_libvers()
        if netcdf_text is None:
            raise PackageError("netCDF-C returned no library version")
        match = re.search(r"\d+\.\d+\.\d+", netcdf_text.decode("ascii", errors="replace"))
        if match is None:
            raise PackageError(f"netCDF-C returned an unparseable version: {netcdf_text!r}")

        hdf5 = ctypes.CDLL(
            str(
                native_library_path(
                    stage,
                    native_files,
                    "hdf5",
                    excluded=("_hl", "-hl", "cpp", "fortran", "tools"),
                )
            )
        )
        hdf5.H5get_libversion.argtypes = [ctypes.POINTER(ctypes.c_uint)] * 3
        major, minor, release = ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint()
        if hdf5.H5get_libversion(
            ctypes.byref(major), ctypes.byref(minor), ctypes.byref(release)
        ) != 0:
            raise PackageError("HDF5 failed to report its library version")

    return {
        "ecCodes": eccodes_version,
        "netCDF-C": match.group(0),
        "HDF5": f"{major.value}.{minor.value}.{release.value}",
    }


def verify_native_component_versions(
    components: list[dict[str, str]], actual: dict[str, str]
) -> None:
    declared = {component["name"]: component["version"] for component in components}
    if declared != actual:
        raise PackageError(
            f"declared native component versions do not match runtime probes: {declared} != {actual}"
        )


def copy_tree_strict(source: Path, destination: Path) -> None:
    if not source.is_dir():
        raise PackageError(f"required package directory is missing: {source}")
    for path in source.rglob("*"):
        if path.is_symlink():
            raise PackageError(f"package source contains a symlink: {path}")
    shutil.copytree(source, destination)


def copy_definitions_tree(source: Path, destination: Path) -> None:
    if not source.is_dir():
        raise PackageError(f"required package directory is missing: {source}")
    root = source.resolve()
    for path in source.rglob("*"):
        if not path.is_symlink():
            continue
        try:
            target = path.resolve(strict=True)
            target.relative_to(root)
        except (OSError, RuntimeError, ValueError) as error:
            raise PackageError(
                f"ecCodes definition link escapes its source root or is invalid: {path}"
            ) from error
        if not target.is_file():
            raise PackageError(
                f"ecCodes definition link must resolve to a regular file: {path}"
            )
    shutil.copytree(source, destination, symlinks=False)


def payload_role(relative: str, binary_name: str) -> str:
    if relative == binary_name:
        return "binary"
    if relative == "LICENSE":
        return "license"
    if relative == "README.md":
        return "documentation"
    if relative == SBOM_NAME:
        return "sbom"
    if relative == LICENSE_INVENTORY_NAME:
        return "license_inventory"
    if relative.startswith("examples/minimal/"):
        return "example"
    if relative.startswith("share/eccodes/definitions/"):
        return "native_data"
    if relative.startswith("lib/") or relative.lower().endswith(".dll"):
        return "native_library"
    raise PackageError(f"unclassified package payload: {relative}")


def payload_inventory(stage: Path, binary_name: str) -> list[dict[str, Any]]:
    payload = []
    for path in sorted(stage.rglob("*"), key=lambda item: item.relative_to(stage).as_posix()):
        if path.is_symlink():
            raise PackageError(f"package staging contains a symlink: {path}")
        if not path.is_file() or path.name == MANIFEST_NAME:
            continue
        relative = normalized_relative(path.relative_to(stage).as_posix())
        identity = file_identity(path, relative)
        identity["role"] = payload_role(relative, binary_name)
        payload.append(identity)
    return payload


def copy_product_payload(
    binary: Path,
    stage: Path,
    platform_contract: dict[str, str],
    native_library_directories: list[Path],
    definitions: Path | None,
) -> list[str]:
    binary_destination = stage / platform_contract["binary_product"]
    shutil.copy2(binary, binary_destination)
    if platform_contract["label"] == "linux-x86_64":
        binary_destination.chmod(0o755)
    shutil.copy2(ROOT / "LICENSE", stage / "LICENSE")
    shutil.copy2(ROOT / "packaging" / "README.md", stage / "README.md")
    copy_tree_strict(ROOT / "packaging" / "examples", stage / "examples")
    if platform_contract["label"] == "windows-x86_64":
        if not native_library_directories:
            raise PackageError("Windows packaging requires --native-library-dir")
        native_files = collect_windows_libraries(binary_destination, stage, native_library_directories)
    else:
        native_files = collect_linux_libraries(binary_destination, stage)
    definitions_root = stage / "share" / "eccodes" / "definitions"
    if definitions is not None:
        copy_definitions_tree(definitions, definitions_root)
    else:
        if not any("eccodes_memfs" in name.lower() for name in native_files):
            raise PackageError("embedded ecCodes definitions require an eccodes_memfs runtime library")
        definitions_root.mkdir(parents=True)
        (definitions_root / "EMBEDDED-MEMFS.txt").write_text(
            "ecCodes definitions are embedded in the packaged eccodes_memfs runtime.\n",
            encoding="utf-8",
            newline="\n",
        )
    return native_files


def zip_datetime(epoch: int) -> tuple[int, int, int, int, int, int]:
    import datetime

    value = datetime.datetime.fromtimestamp(max(epoch, 315532800), datetime.UTC)
    return (value.year, value.month, value.day, value.hour, value.minute, value.second)


def create_zip(stage: Path, archive: Path, root_name: str, epoch: int) -> None:
    timestamp = zip_datetime(epoch)
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as output:
        for path in [stage, *sorted(stage.rglob("*"), key=lambda item: item.relative_to(stage).as_posix())]:
            relative = path.relative_to(stage).as_posix()
            member = root_name + (f"/{relative}" if relative != "." else "")
            if path.is_dir():
                member = member.rstrip("/") + "/"
                info = zipfile.ZipInfo(member, timestamp)
                info.external_attr = (stat.S_IFDIR | 0o755) << 16
                output.writestr(info, b"")
                continue
            mode = 0o755 if path.name in {"trajecta", "trajecta.exe"} else 0o644
            info = zipfile.ZipInfo(member, timestamp)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = (stat.S_IFREG | mode) << 16
            output.writestr(info, path.read_bytes())


def tar_info(path: Path, name: str, epoch: int) -> tarfile.TarInfo:
    info = tarfile.TarInfo(name)
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = epoch
    if path.is_dir():
        info.type = tarfile.DIRTYPE
        info.mode = 0o755
        info.size = 0
    else:
        info.type = tarfile.REGTYPE
        info.mode = 0o755 if path.name == "trajecta" else 0o644
        info.size = path.stat().st_size
    return info


def create_tar_gz(stage: Path, archive: Path, root_name: str, epoch: int) -> None:
    with archive.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=epoch, compresslevel=9) as zipped:
            with tarfile.open(fileobj=zipped, mode="w", format=tarfile.PAX_FORMAT) as output:
                for path in [stage, *sorted(stage.rglob("*"), key=lambda item: item.relative_to(stage).as_posix())]:
                    relative = path.relative_to(stage).as_posix()
                    member = root_name + (f"/{relative}" if relative != "." else "")
                    info = tar_info(path, member, epoch)
                    if path.is_file():
                        with path.open("rb") as stream:
                            output.addfile(info, stream)
                    else:
                        output.addfile(info)


def create_archive(stage: Path, archive: Path, root_name: str, archive_format: str, epoch: int) -> None:
    if archive_format == "zip":
        create_zip(stage, archive, root_name, epoch)
    elif archive_format == "tar.gz":
        create_tar_gz(stage, archive, root_name, epoch)
    else:
        raise PackageError(f"unsupported archive format: {archive_format}")


def linux_release_compatible(allow_nonformal: bool) -> None:
    if not sys.platform.startswith("linux"):
        return
    release = Path("/etc/os-release")
    values: dict[str, str] = {}
    if release.is_file():
        for line in release.read_text(encoding="utf-8").splitlines():
            if "=" in line:
                key, value = line.split("=", 1)
                values[key] = value.strip().strip('"')
    formal = values.get("ID") == "ubuntu" and values.get("VERSION_ID") == "24.04"
    if not formal and not allow_nonformal:
        raise PackageError(
            "formal Linux package must be built on Ubuntu 24.04; use --allow-nonformal-linux only for local preflight"
        )


def build_package(arguments: argparse.Namespace) -> dict[str, Any]:
    platform_contract = host_platform_contract()
    linux_release_compatible(arguments.allow_nonformal_linux)
    version = workspace_version()
    source = source_tree_identity()
    source_date_epoch = arguments.source_date_epoch
    if source_date_epoch is None:
        source_date_epoch = int(command_output(["git", "show", "-s", "--format=%ct", "HEAD"]))
    if source_date_epoch < 0:
        raise PackageError("SOURCE_DATE_EPOCH must be non-negative")
    native_components = validate_native_components(arguments.native_component)
    output_root = arguments.output_root.resolve()
    output_root.mkdir(parents=True, exist_ok=True)
    root_name = f"trajecta-{version}-{platform_contract['label']}"
    stage = output_root / root_name
    extension = ".zip" if platform_contract["archive_format"] == "zip" else ".tar.gz"
    archive = output_root / f"{root_name}{extension}"
    for path in [stage, archive, archive.with_name(archive.name + ".sha256")]:
        if path.exists():
            raise PackageError(f"refusing to overwrite existing package artifact: {path}")
    target_dir = arguments.target_dir.resolve()
    binary, environment = build_release_binary(target_dir, source_date_epoch, platform_contract)
    metadata = cargo_metadata(platform_contract, environment)
    packages, edges = dependency_closure(metadata)
    stage.mkdir()
    native_files = copy_product_payload(
        binary,
        stage,
        platform_contract,
        [path.resolve() for path in arguments.native_library_dir],
        None
        if arguments.eccodes_definitions.casefold() == "embedded"
        else Path(arguments.eccodes_definitions).resolve(),
    )
    native_versions = probe_native_versions(stage, native_files)
    verify_native_component_versions(native_components, native_versions)
    write_supply_chain_files(stage, packages, edges, native_components, version)
    payload = payload_inventory(stage, platform_contract["binary_product"])
    binary_identity = file_identity(
        stage / platform_contract["binary_product"], platform_contract["binary_product"]
    )
    sbom_identity = file_identity(stage / SBOM_NAME, SBOM_NAME)
    sbom_identity["spec_version"] = "CycloneDX 1.5"
    license_identity = file_identity(stage / LICENSE_INVENTORY_NAME, LICENSE_INVENTORY_NAME)
    manifest = {
        "schema_version": "trajecta.build-manifest/v1",
        "product": "trajecta",
        "version": version,
        "platform": {
            key: platform_contract[key] for key in ["label", "target_triple", "minimum_os"]
        },
        "source": source,
        "build": {
            "cargo_profile": "release",
            "cargo": command_output(["cargo", "-V"]),
            "rustc": command_output(["rustc", "-Vv"]),
            "features": FEATURES,
            "default_reader_backend": "rust",
            "native_reader_included": True,
            "source_date_epoch": source_date_epoch,
        },
        "archive": {
            "file_name": archive.name,
            "format": platform_contract["archive_format"],
            "root_directory": root_name,
        },
        "binary": binary_identity,
        "sbom": sbom_identity,
        "license_inventory": license_identity,
        "native_components": native_components,
        "payload": payload,
    }
    write_json(stage / MANIFEST_NAME, manifest)
    create_archive(stage, archive, root_name, platform_contract["archive_format"], source_date_epoch)
    archive_sha = sha256(archive)
    checksum = archive.with_name(archive.name + ".sha256")
    checksum.write_text(f"{archive_sha}  {archive.name}\n", encoding="ascii", newline="\n")
    result = {
        "schema_version": "trajecta.m5-a4-package-build-result/v1",
        "status": "passed",
        "platform": platform_contract["label"],
        "archive": str(archive),
        "archive_sha256": archive_sha,
        "checksum": str(checksum),
        "build_manifest": str(stage / MANIFEST_NAME),
        "build_manifest_sha256": sha256(stage / MANIFEST_NAME),
        "binary_sha256": binary_identity["sha256"],
        "source_tree_sha256": source["source_tree_sha256"],
        "payload_files": len(payload),
        "native_runtime_files": native_files,
        "native_version_probe": native_versions,
    }
    write_json(output_root / BUILD_RESULT_NAME, result)
    return result


def archive_checksum_path(archive: Path, explicit: Path | None) -> Path:
    return explicit.resolve() if explicit is not None else archive.with_name(archive.name + ".sha256")


def verify_archive_checksum(archive: Path, checksum_path: Path) -> str:
    if not checksum_path.is_file():
        raise PackageError(f"archive checksum file is missing: {checksum_path}")
    fields = checksum_path.read_text(encoding="ascii").strip().split()
    if len(fields) != 2 or fields[1].lstrip("*") != archive.name or not SHA256_PATTERN.fullmatch(fields[0]):
        raise PackageError("archive checksum file has an invalid shape or filename")
    actual = sha256(archive)
    if actual != fields[0]:
        raise PackageError("archive SHA-256 does not match the adjacent checksum")
    return actual


def safe_member_name(name: str) -> str:
    normalized = name.rstrip("/")
    if normalized == "":
        raise PackageError("archive contains an empty member name")
    pure = PurePosixPath(normalized)
    if pure.is_absolute() or ".." in pure.parts or "\\" in name:
        raise PackageError(f"archive member escapes extraction root: {name}")
    return pure.as_posix()


def inspect_zip(archive: Path) -> str:
    roots: set[str] = set()
    with zipfile.ZipFile(archive) as source:
        for member in source.infolist():
            name = safe_member_name(member.filename)
            roots.add(PurePosixPath(name).parts[0])
            mode = member.external_attr >> 16
            if stat.S_IFMT(mode) == stat.S_IFLNK:
                raise PackageError(f"ZIP contains a symlink: {member.filename}")
    if len(roots) != 1:
        raise PackageError(f"archive must contain exactly one root directory, got {sorted(roots)}")
    return next(iter(roots))


def inspect_tar(archive: Path) -> str:
    roots: set[str] = set()
    with tarfile.open(archive, mode="r:gz") as source:
        for member in source.getmembers():
            name = safe_member_name(member.name)
            roots.add(PurePosixPath(name).parts[0])
            if not (member.isdir() or member.isreg()):
                raise PackageError(f"tar contains a non-file member: {member.name}")
    if len(roots) != 1:
        raise PackageError(f"archive must contain exactly one root directory, got {sorted(roots)}")
    return next(iter(roots))


def extract_archive(archive: Path, destination: Path) -> Path:
    if destination.exists() and any(destination.iterdir()):
        raise PackageError(f"clean extraction destination is not empty: {destination}")
    destination.mkdir(parents=True, exist_ok=True)
    if archive.name.endswith(".zip"):
        root_name = inspect_zip(archive)
        with zipfile.ZipFile(archive) as source:
            source.extractall(destination)
    elif archive.name.endswith(".tar.gz"):
        root_name = inspect_tar(archive)
        with tarfile.open(archive, mode="r:gz") as source:
            source.extractall(destination, filter="data")
    else:
        raise PackageError("product archive must be .zip or .tar.gz")
    root = destination / root_name
    if not root.is_dir():
        raise PackageError("archive root directory was not extracted")
    return root


def validate_build_manifest(root: Path) -> dict[str, Any]:
    manifest_path = root / MANIFEST_NAME
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise PackageError(f"read build manifest: {error}") from error
    required = {
        "schema_version",
        "product",
        "version",
        "platform",
        "source",
        "build",
        "archive",
        "binary",
        "sbom",
        "license_inventory",
        "native_components",
        "payload",
    }
    if set(manifest) != required:
        raise PackageError("build manifest top-level fields drifted")
    if manifest["schema_version"] != "trajecta.build-manifest/v1":
        raise PackageError("build manifest schema identity drifted")
    version = manifest["version"]
    if (
        manifest["product"] != "trajecta"
        or not isinstance(version, str)
        or VERSION_PATTERN.fullmatch(version) is None
    ):
        raise PackageError("build manifest product or version is invalid")
    platform_label = manifest["platform"].get("label")
    expected_root = f"trajecta-{version}-{platform_label}"
    if root.name != expected_root:
        raise PackageError("build manifest version/platform does not match archive root")
    build = manifest["build"]
    if build.get("features") != FEATURES or build.get("default_reader_backend") != "rust":
        raise PackageError("build manifest feature/default reader contract drifted")
    if build.get("native_reader_included") is not True:
        raise PackageError("build manifest does not declare the native reader")
    names = {component.get("name") for component in manifest["native_components"]}
    if names != REQUIRED_NATIVE_COMPONENTS:
        raise PackageError("build manifest native component set drifted")
    payload = manifest["payload"]
    paths = [entry.get("path") for entry in payload]
    if paths != sorted(paths) or len(paths) != len(set(paths)):
        raise PackageError("build manifest payload is not sorted and unique")
    required_roles = {
        "binary",
        "documentation",
        "license",
        "sbom",
        "license_inventory",
        "example",
        "native_library",
        "native_data",
    }
    if not required_roles <= {entry.get("role") for entry in payload}:
        raise PackageError("build manifest is missing a required payload role")
    listed = set(paths)
    actual = {
        path.relative_to(root).as_posix()
        for path in root.rglob("*")
        if path.is_file() and path.name != MANIFEST_NAME
    }
    if listed != actual:
        missing = sorted(listed - actual)
        extra = sorted(actual - listed)
        raise PackageError(f"package payload coverage mismatch: missing={missing}, extra={extra}")
    for entry in payload:
        relative = normalized_relative(entry["path"])
        path = root / Path(*PurePosixPath(relative).parts)
        identity = file_identity(path, relative)
        if identity["sha256"] != entry.get("sha256") or identity["bytes"] != entry.get("bytes"):
            raise PackageError(f"package payload identity mismatch: {relative}")
    binary = manifest["binary"]
    binary_path = root / Path(*PurePosixPath(normalized_relative(binary["path"])).parts)
    if not binary_path.is_file() or sha256(binary_path) != binary.get("sha256"):
        raise PackageError("package binary identity mismatch")
    if sys.platform.startswith("linux") and not os.access(binary_path, os.X_OK):
        raise PackageError("Linux package binary is not executable")
    sbom = json.loads((root / SBOM_NAME).read_text(encoding="utf-8"))
    if sbom.get("bomFormat") != "CycloneDX" or sbom.get("specVersion") != "1.5":
        raise PackageError("package SBOM is not CycloneDX 1.5")
    inventory = json.loads((root / LICENSE_INVENTORY_NAME).read_text(encoding="utf-8"))
    if inventory.get("schema_version") != "trajecta.third-party-license-inventory/v1":
        raise PackageError("package license inventory identity drifted")
    for path in root.rglob("*"):
        if path.is_symlink():
            raise PackageError(f"extracted package contains a symlink: {path}")
    return manifest


def verify_package(arguments: argparse.Namespace) -> dict[str, Any]:
    archive = arguments.archive.resolve()
    if not archive.is_file():
        raise PackageError(f"product archive is missing: {archive}")
    archive_sha = verify_archive_checksum(archive, archive_checksum_path(archive, arguments.checksum))
    temporary: tempfile.TemporaryDirectory[str] | None = None
    if arguments.extract_root is None:
        temporary = tempfile.TemporaryDirectory(prefix="trajecta-m5-a4-extract-")
        extraction = Path(temporary.name)
    else:
        extraction = arguments.extract_root.resolve()
    try:
        root = extract_archive(archive, extraction)
        manifest = validate_build_manifest(root)
        if manifest["archive"]["file_name"] != archive.name:
            raise PackageError("archive filename differs from build manifest")
        if manifest["archive"]["root_directory"] != root.name:
            raise PackageError("archive root differs from build manifest")
        native_files = [
            entry["path"]
            for entry in manifest["payload"]
            if entry["role"] == "native_library"
        ]
        native_versions = probe_native_versions(root, native_files)
        verify_native_component_versions(manifest["native_components"], native_versions)
        result = {
            "schema_version": "trajecta.m5-a4-package-verify-result/v1",
            "status": "passed",
            "archive": str(archive),
            "archive_sha256": archive_sha,
            "extracted_root": str(root),
            "build_manifest_sha256": sha256(root / MANIFEST_NAME),
            "binary_sha256": manifest["binary"]["sha256"],
            "source_tree_sha256": manifest["source"]["source_tree_sha256"],
            "payload_files": len(manifest["payload"]),
            "native_version_probe": native_versions,
        }
        if arguments.result is not None:
            write_json(arguments.result.resolve(), result)
        return result
    finally:
        if temporary is not None:
            temporary.cleanup()


def parser() -> argparse.ArgumentParser:
    product = argparse.ArgumentParser(description=__doc__)
    commands = product.add_subparsers(dest="command", required=True)
    build = commands.add_parser("build", help="build one host-native release package")
    build.add_argument("--output-root", type=Path, required=True)
    build.add_argument("--target-dir", type=Path, required=True)
    build.add_argument("--native-library-dir", type=Path, action="append", default=[])
    build.add_argument(
        "--eccodes-definitions",
        required=True,
        help="definitions directory or the literal 'embedded' for a verified ecCodes MEMFS build",
    )
    build.add_argument(
        "--native-component",
        type=parse_native_component,
        action="append",
        required=True,
        help="NAME|VERSION|LICENSE|SOURCE; specify ecCodes, netCDF-C, and HDF5 exactly once",
    )
    build.add_argument("--source-date-epoch", type=int)
    build.add_argument("--allow-nonformal-linux", action="store_true")
    verify = commands.add_parser("verify", help="verify and safely clean-extract one archive")
    verify.add_argument("--archive", type=Path, required=True)
    verify.add_argument("--checksum", type=Path)
    verify.add_argument("--extract-root", type=Path)
    verify.add_argument("--result", type=Path)
    return product


def main() -> int:
    arguments = parser().parse_args()
    try:
        result = build_package(arguments) if arguments.command == "build" else verify_package(arguments)
    except PackageError as error:
        print(f"m5-a4 package failed: {error}", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError as error:
        print(f"m5-a4 command failed ({error.returncode}): {error.cmd}", file=sys.stderr)
        if error.output:
            print(error.output, file=sys.stderr)
        return 1
    except OSError as error:
        print(f"m5-a4 operating-system failure: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
