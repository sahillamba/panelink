#!/usr/bin/env python3
"""Generate a locked/offline Cargo dependency and license inventory."""

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tomllib
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
RAW_DIR = ROOT / "docs" / "progress" / "p6-raw"
REPORT_PATH = ROOT / "docs" / "progress" / "P6-DEPENDENCY-INVENTORY.md"
JSON_PATH = RAW_DIR / "dependency-inventory.json"
ALLOWED_LICENSES = {
    "MIT",
    "Apache-2.0",
    "BSD-0-Clause",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSD-4-Clause",
    "LLVM-exception",
}


def _package_id(package: dict[str, Any]) -> str:
    return str(package["id"])


def _dependency_edges(metadata: dict[str, Any]) -> dict[str, set[str]]:
    """Return the normal/build package graph from Cargo metadata."""
    edges: dict[str, set[str]] = {}
    for node in metadata.get("resolve", {}).get("nodes", []):
        children: set[str] = set()
        for dependency in node.get("deps", []):
            kinds = dependency.get("dep_kinds", [])
            if any(kind.get("kind") in (None, "normal", "build") for kind in kinds):
                children.add(str(dependency["pkg"]))
        # Older Cargo metadata shapes expose only a package-ID dependency list.
        if not children:
            children.update(str(item) for item in node.get("dependencies", []))
        edges[str(node["id"])] = children
    return edges


def direct_dependency_roots(metadata: dict[str, Any]) -> dict[str, set[str]]:
    """Map each direct package ID to its own package name."""
    resolve = metadata.get("resolve") or {}
    root_id = resolve.get("root")
    nodes = {str(node["id"]): node for node in resolve.get("nodes", [])}
    if root_id is None:
        members = metadata.get("workspace_members", [])
        root_id = str(members[0]) if members else None
    root_node = nodes.get(str(root_id)) if root_id is not None else None
    root_package_id = str(root_id) if root_id is not None else None
    if root_node is None:
        package_by_id = {
            _package_id(package): package for package in metadata.get("packages", [])
        }
        root_package = package_by_id.get(str(root_package_id))
        if root_package is None:
            return {}
        manifest_path = Path(str(root_package.get("manifest_path", "")))
        try:
            manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError):
            return {}
        by_name = {str(package["name"]): _package_id(package) for package in package_by_id.values()}
        roots: dict[str, set[str]] = {}
        sections = ("dependencies", "build-dependencies")
        entries: list[tuple[str, Any]] = []
        for section in sections:
            entries.extend(manifest.get(section, {}).items())
        for target in manifest.get("target", {}).values():
            for section in sections:
                entries.extend(target.get(section, {}).items())
        for alias, declaration in entries:
            name = declaration.get("package", alias) if isinstance(declaration, dict) else alias
            package_id = by_name.get(str(name))
            if package_id:
                roots[package_id] = {str(name)}
        return roots
    packages = {_package_id(package): package for package in metadata.get("packages", [])}
    roots: dict[str, set[str]] = {}
    for dependency in root_node.get("deps", []):
        kinds = dependency.get("dep_kinds", [])
        if not any(kind.get("kind") in (None, "normal", "build") for kind in kinds):
            continue
        package_id = str(dependency["pkg"])
        package = packages.get(package_id)
        if package:
            roots[package_id] = {str(package["name"])}
    # Older Cargo metadata: direct names come from workspace package declarations.
    return roots


def license_flags(expression: str | None) -> list[str]:
    if not expression or not expression.strip():
        return ["unknown-license"]
    tokens = re.findall(r"[A-Za-z0-9][A-Za-z0-9.-]*", expression)
    identifiers = [token for token in tokens if token not in {"AND", "OR", "WITH"}]
    unknown = sorted({token for token in identifiers if token not in ALLOWED_LICENSES})
    return [f"non-permissive-or-unreviewed-license:{token}" for token in unknown]


def _source(package: dict[str, Any], root: Path = ROOT) -> str:
    source = package.get("source")
    if source:
        return "registry" if str(source).startswith("registry+") else "git"
    manifest = Path(str(package.get("manifest_path", "")))
    try:
        relative = manifest.resolve().relative_to(root.resolve())
    except (OSError, ValueError):
        return "path"
    return "vendored" if relative.parts[:1] == ("vendor",) else "path"


def _review_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError:
        return ""


def _pulling_direct_dependencies(
    package_id: str,
    roots: dict[str, set[str]],
    edges: dict[str, set[str]],
) -> list[str]:
    pullers: set[str] = set()
    for root_id, names in roots.items():
        seen: set[str] = set()
        stack = [root_id]
        while stack:
            current = stack.pop()
            if current in seen:
                continue
            seen.add(current)
            if current == package_id:
                pullers.update(names)
                break
            stack.extend(edges.get(current, ()))
    return sorted(pullers)


def build_inventory(metadata: dict[str, Any], cargo_tree: str, root: Path = ROOT) -> dict[str, Any]:
    """Purely aggregate metadata and tree observations into stable package rows."""
    del cargo_tree  # Kept as an explicit input because the report records its raw evidence.
    packages = metadata.get("packages", [])
    roots = direct_dependency_roots(metadata)
    edges = _dependency_edges(metadata)
    root_id = (metadata.get("resolve") or {}).get("root")
    package_rows = []
    for package in packages:
        package_id = _package_id(package)
        if root_id is not None and package_id == str(root_id):
            continue
        targets = package.get("targets", [])
        build_script = any("custom-build" in target.get("kind", []) for target in targets)
        proc_macro = any("proc-macro" in target.get("kind", []) for target in targets)
        direct = package_id in roots
        license_expression = package.get("license")
        flags = license_flags(license_expression)
        if build_script:
            flags.append("build-script-review")
        if proc_macro:
            flags.append("proc-macro-review")
        package_rows.append(
            {
                "name": str(package.get("name", "unknown")),
                "version": str(package.get("version", "unknown")),
                "source": _source(package, root),
                "license_expression": license_expression,
                "role": "direct" if direct else "transitive",
                "build_script": build_script,
                "proc_macro": proc_macro,
                "pulled_by": [str(package["name"])] if direct else _pulling_direct_dependencies(package_id, roots, edges),
                "review_flags": sorted(flags),
            }
        )
    package_rows.sort(key=lambda row: (row["name"].casefold(), row["version"], row["source"]))

    notices = _review_text(root / "THIRD_PARTY_NOTICES.md")
    replacement = _review_text(root / "docs" / "progress" / "P2-REPLACEMENT.md")
    vendor_rows = []
    for package in package_rows:
        if package["source"] != "vendored":
            continue
        name = package["name"]
        vendor_dir = root / "vendor" / name
        notice_match = name.casefold() in notices.casefold()
        replacement_match = name.casefold() in replacement.casefold()
        package["review_flags"].extend(
            [] if notice_match and replacement_match else ["vendored-notice-cross-check"]
        )
        package["review_flags"].sort()
        vendor_rows.append(
            {
                "name": name,
                "vendor_directory_present": vendor_dir.is_dir(),
                "third_party_notices_mentions_component": notice_match,
                "p2_replacement_mentions_component": replacement_match,
                "cross_check": "matched" if vendor_dir.is_dir() and notice_match and replacement_match else "review-needed",
            }
        )

    return {
        "schema_version": 1,
        "commands": {
            "metadata": "cargo metadata --locked --offline --format-version 1",
            "tree": "cargo tree --locked --offline -e normal,build --prefix none",
        },
        "packages": package_rows,
        "package_count": len(package_rows),
        "direct_count": sum(row["role"] == "direct" for row in package_rows),
        "transitive_count": sum(row["role"] == "transitive" for row in package_rows),
        "vendored_cross_checks": vendor_rows,
        "advisory_scan": {
            "status": "blocked",
            "cargo_audit_installed": shutil.which("cargo-audit") is not None,
            "cargo_deny_installed": shutil.which("cargo-deny") is not None,
            "note": "Advisory scanning was not performed: cargo-audit and cargo-deny are not installed; no network or advisory database was used.",
        },
    }


def build_lock_fallback(lock_text: str, manifest_text: str, root: Path = ROOT) -> dict[str, Any]:
    """Keep a complete package list when offline metadata lacks a cached crate."""
    lock = tomllib.loads(lock_text)
    manifest = tomllib.loads(manifest_text)
    dependency_names: set[str] = set()
    for section_name in ("dependencies", "build-dependencies"):
        for alias, declaration in manifest.get(section_name, {}).items():
            dependency_names.add(str(declaration.get("package", alias)) if isinstance(declaration, dict) else str(alias))
    for target in manifest.get("target", {}).values():
        for section_name in ("dependencies", "build-dependencies"):
            for alias, declaration in target.get(section_name, {}).items():
                dependency_names.add(str(declaration.get("package", alias)) if isinstance(declaration, dict) else str(alias))

    packages = lock.get("package", [])
    by_id = {
        (str(package["name"]), str(package["version"]), package.get("source")): package
        for package in packages
    }
    edges: dict[tuple[str, str, str | None], set[tuple[str, str, str | None]]] = {}
    for package in packages:
        parent = (str(package["name"]), str(package["version"]), package.get("source"))
        children = set()
        for declaration in package.get("dependencies", []):
            parts = str(declaration).split()
            if not parts:
                continue
            name = parts[0]
            version = parts[1] if len(parts) >= 2 and not parts[1].startswith("(") else None
            source = None
            source_token = next((part for part in parts[1:] if part.startswith("(")), None)
            if source_token is not None:
                source = source_token[1:].removesuffix(")")
            matches = [
                package_id
                for package_id in by_id
                if package_id[0] == name
                and (version is None or package_id[1] == version)
                and (source is None or (package_id[2] or "").startswith(source))
            ]
            children.update(matches)
        edges[parent] = children

    package_table = tomllib.loads(manifest_text).get("package", {})
    manifest_path = root / "Cargo.toml"
    dependency_by_id = direct_dependency_roots(
        {
            "resolve": {"root": f"panelink {package_table.get('version', '0.1.0')} ({manifest_path.as_uri()})"},
            "workspace_members": [f"panelink {package_table.get('version', '0.1.0')} ({manifest_path.as_uri()})"],
            "packages": [
                {"id": f"panelink {package_table.get('version', '0.1.0')} ({manifest_path.as_uri()})", "name": "panelink", "manifest_path": str(manifest_path)},
                *[
                    {"id": f"{name} {version}" + (f" ({source})" if source else ""), "name": name}
                    for name, version, source in by_id
                ],
            ],
        }
    )
    direct_names = {name for names in dependency_by_id.values() for name in names} or dependency_names
    direct_ids = {package_id for package_id in by_id if package_id[0] in direct_names}
    rows = []
    registry_roots = sorted(Path.home().glob(".cargo/registry/src/*"))
    for package_id, package in by_id.items():
        name, version, source = package_id
        if name == "panelink":
            continue
        pullers: set[str] = set()
        for direct_id in direct_ids:
            seen: set[tuple[str, str, str | None]] = set()
            stack = [direct_id]
            while stack:
                current = stack.pop()
                if current in seen:
                    continue
                seen.add(current)
                if current == package_id:
                    pullers.add(direct_id[0])
                    break
                stack.extend(edges.get(current, ()))
        local_vendor = (root / "vendor" / name).is_dir()
        if source:
            package_source = "registry" if source.startswith("registry+") else "git"
        else:
            package_source = "vendored" if local_vendor else "path"
        manifest_path = (root / "vendor" / name / "Cargo.toml") if local_vendor else None
        if source:
            for registry_root in registry_roots:
                candidate = registry_root / f"{name}-{version}" / "Cargo.toml"
                if candidate.is_file():
                    manifest_path = candidate
                    break
        license_expression: str | None = None
        build_script: bool | None = None
        proc_macro: bool | None = None
        if manifest_path is not None and manifest_path.is_file():
            try:
                package_manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
                package_metadata = package_manifest.get("package", {})
                license_expression = package_metadata.get("license")
                build_script = package_metadata.get("build") is not False and (
                    package_metadata.get("build") is True
                    or isinstance(package_metadata.get("build"), str)
                    or (manifest_path.parent / "build.rs").is_file()
                )
                proc_macro = bool(package_manifest.get("lib", {}).get("proc-macro", False))
            except (OSError, tomllib.TOMLDecodeError):
                pass
        review_flags = license_flags(license_expression)
        if build_script is None or proc_macro is None:
            review_flags.append("build-targets-unknown")
        if build_script:
            review_flags.append("build-script-review")
        if proc_macro:
            review_flags.append("proc-macro-review")
        rows.append(
            {
                "name": name,
                "version": version,
                "source": package_source,
                "license_expression": license_expression,
                "role": "direct" if package_id in direct_ids else "transitive",
                "build_script": build_script,
                "proc_macro": proc_macro,
                "pulled_by": sorted(pullers),
                "review_flags": sorted(set(review_flags + ["metadata-unavailable"])),
            }
        )
    rows.sort(key=lambda row: (row["name"].casefold(), row["version"], row["source"]))

    notices = _review_text(root / "THIRD_PARTY_NOTICES.md")
    replacement = _review_text(root / "docs" / "progress" / "P2-REPLACEMENT.md")
    vendored = []
    for row in rows:
        if row["source"] != "vendored":
            continue
        notice_match = row["name"].casefold() in notices.casefold()
        replacement_match = row["name"].casefold() in replacement.casefold()
        vendored.append(
            {
                "name": row["name"],
                "vendor_directory_present": (root / "vendor" / row["name"]).is_dir(),
                "third_party_notices_mentions_component": notice_match,
                "p2_replacement_mentions_component": replacement_match,
                "cross_check": "matched" if notice_match and replacement_match else "review-needed",
            }
        )
    return {
        "schema_version": 1,
        "packages": rows,
        "package_count": len(rows),
        "direct_count": sum(row["role"] == "direct" for row in rows),
        "transitive_count": sum(row["role"] == "transitive" for row in rows),
        "vendored_cross_checks": vendored,
        "metadata_status": "blocked; package rows are from Cargo.lock; any unavailable local package manifest retains unknown license/build metadata",
        "advisory_scan": {
            "status": "blocked",
            "cargo_audit_installed": shutil.which("cargo-audit") is not None,
            "cargo_deny_installed": shutil.which("cargo-deny") is not None,
            "note": "Advisory scanning was not performed: cargo-audit and cargo-deny are not installed; no network or advisory database was used.",
        },
    }


def _run(command: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=False)


def _markdown(inventory: dict[str, Any], metadata_exit: int, tree_exit: int) -> str:
    lines = [
        "# Phase 6 dependency and license inventory",
        "",
        "Status: generated from Cargo metadata and a normal/build dependency tree using locked offline commands.",
        "",
        f"Package rows: {inventory['package_count']} ({inventory['direct_count']} direct, {inventory['transitive_count']} transitive).",
        f"Metadata command exit: {metadata_exit}; tree command exit: {tree_exit}.",
        "",
        "| Package | Version | Source | License expression | Role | Build script | Proc macro | Pulled by direct dependency | Review flags |",
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- |",
    ]
    for package in inventory["packages"]:
        escape = lambda value: str(value).replace("|", "&#124;").replace("\n", " ")
        lines.append(
            "| "
            + " | ".join(
                [
                    escape(package["name"]),
                    escape(package["version"]),
                    escape(package["source"]),
                    escape(package["license_expression"] or "UNKNOWN"),
                    escape(package["role"]),
                    "yes" if package["build_script"] else "no",
                    "yes" if package["proc_macro"] else "no",
                    escape(", ".join(package["pulled_by"]) or "unresolved"),
                    escape(", ".join(package["review_flags"]) or "none"),
                ]
            )
            + " |"
        )
    lines.extend(["", "## Vendored component notice cross-check", ""])
    if inventory["vendored_cross_checks"]:
        lines.extend(
            [
                "| Component | Vendor directory | `THIRD_PARTY_NOTICES.md` | `P2-REPLACEMENT.md` | Result |",
                "| --- | --- | --- | --- | --- |",
            ]
        )
        for row in inventory["vendored_cross_checks"]:
            lines.append(
                f"| {row['name']} | {'present' if row['vendor_directory_present'] else 'missing'} | "
                f"{'mentioned' if row['third_party_notices_mentions_component'] else 'missing'} | "
                f"{'mentioned' if row['p2_replacement_mentions_component'] else 'missing'} | {row['cross_check']} |"
            )
    else:
        lines.append("No vendored package rows were returned by Cargo metadata.")
    lines.extend(
        [
            "",
            "## Advisory scan",
            "",
            "**Blocked:** cargo-audit and cargo-deny are not installed. Advisory scanning was not performed; no tools or advisory database were fetched.",
            "",
            "Raw Cargo stdout/stderr, exit codes, package rows and command strings are retained in [p6-raw/dependency-inventory.json](p6-raw/dependency-inventory.json).",
            "",
        ]
    )
    return "\n".join(lines)


def main() -> int:
    metadata = _run(["cargo", "metadata", "--locked", "--offline", "--format-version", "1"])
    tree = _run(["cargo", "tree", "--locked", "--offline", "-e", "normal,build", "--prefix", "none"])
    metadata_value: dict[str, Any] = {}
    if metadata.returncode == 0:
        try:
            metadata_value = json.loads(metadata.stdout)
        except json.JSONDecodeError:
            metadata_value = {}
    if metadata_value:
        inventory = build_inventory(metadata_value, tree.stdout)
    else:
        inventory = build_lock_fallback(
            (ROOT / "Cargo.lock").read_text(encoding="utf-8"),
            (ROOT / "Cargo.toml").read_text(encoding="utf-8"),
        )
    raw = {
        "schema_version": 1,
        "commands": [
            {
                "command": "cargo metadata --locked --offline --format-version 1",
                "exit_code": metadata.returncode,
                "stdout": metadata.stdout,
                "stderr": metadata.stderr,
            },
            {
                "command": "cargo tree --locked --offline -e normal,build --prefix none",
                "exit_code": tree.returncode,
                "stdout": tree.stdout,
                "stderr": tree.stderr,
            },
        ],
        "inventory": inventory,
    }
    RAW_DIR.mkdir(parents=True, exist_ok=True)
    JSON_PATH.write_text(json.dumps(raw, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    REPORT_PATH.write_text(_markdown(inventory, metadata.returncode, tree.returncode), encoding="utf-8")
    print(
        f"dependency_inventory: metadata_exit={metadata.returncode} tree_exit={tree.returncode} "
        f"packages={inventory['package_count']} direct={inventory['direct_count']} "
        f"transitive={inventory['transitive_count']} advisory_scan=blocked"
    )
    print(f"JSON: {JSON_PATH.relative_to(ROOT)}")
    print(f"Markdown: {REPORT_PATH.relative_to(ROOT)}")
    if metadata.returncode != 0 or tree.returncode != 0:
        print("inventory status: BLOCKED; Cargo metadata/tree commands did not both complete", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
