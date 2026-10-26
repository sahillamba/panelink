#!/usr/bin/env python3
"""Build disposable native OpenCode choices for managed UI transition tests.

Session identifiers and paths are returned only to the live worker in memory.
The evidence projection contains fixed checks and SHA-256 values only.
"""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
from pathlib import Path
from typing import Any

try:
    from scripts import probe_opencode_v2_remaining_identity as identity
    from scripts import probe_panelink_manual_recovery as recovery
except ModuleNotFoundError:
    import probe_opencode_v2_remaining_identity as identity
    import probe_panelink_manual_recovery as recovery


NEW_ROOT_TITLE = "Panelink Managed Transition New Root"
FORK_ROOT_TITLE = "Panelink Managed Transition Imported Fork"
# The global open-menu has narrower rows than the current-project picker.
# Keep the complete exact fixture title visible in both query and result.
FOREIGN_ROOT_TITLE = "Panelink Foreign Root"
FOREIGN_PROJECT_NAME = "managed-transition-foreign-project"


@dataclass(frozen=True)
class ManagedTransitionFixtures:
    """In-memory fixture handles; never serialize this object to probe output."""

    project: Path
    new_root_id: str
    new_root_title: str
    fork_root_id: str
    fork_parent_id: str
    fork_root_title: str
    foreign_project: Path
    foreign_root_id: str
    foreign_root_title: str
    new_root_sha256: str
    fork_root_sha256: str
    fork_parent_sha256: str
    foreign_root_sha256: str
    project_sha256: str
    foreign_project_sha256: str
    fork_history_sha256: str

    def __repr__(self) -> str:
        return "ManagedTransitionFixtures(<redacted>)"


def _id_hash(identifier: str) -> str:
    return hashlib.sha256(identifier.encode("ascii")).hexdigest()


def _path_hash(path: Path) -> str:
    return hashlib.sha256(str(path.resolve(strict=True)).encode("utf-8")).hexdigest()


def _create_root(context: dict[str, Any], title: str, project: Path) -> str | None:
    status, payload = identity._request(context, "POST", "/api/session", {
        "title": title, "location": {"directory": str(project)},
    })
    identifier = identity._session_id(payload, top_level=True) if status == 200 else None
    if identifier is None or not identity._lookup(context, identifier, project, top_level=True):
        return None
    status, payload = identity._request(context, "GET", f"/api/session/{identifier}")
    info = identity._data(payload) if status == 200 else None
    if (not isinstance(info, dict) or info.get("id") != identifier
            or info.get("title") != title
            or not identity._session_row_matches(info, identifier, project, top_level=True)):
        return None
    return identifier


def _foreign_project(context: dict[str, Any], supplied: Path | None) -> Path | None:
    root = Path(context["root"]).resolve(strict=True)
    candidate = supplied if supplied is not None else root / FOREIGN_PROJECT_NAME
    try:
        # Authorize the canonical parent before creating anything. Resolving
        # the parent also prevents a supplied symlinked path from redirecting
        # mkdir outside the disposable fixture root.
        parent = candidate.parent.resolve(strict=True)
        parent.relative_to(root)
        canonical = parent / candidate.name
        canonical.mkdir(mode=0o700, parents=False, exist_ok=False)
        if canonical.stat().st_mode & 0o077:
            return None
        return canonical.resolve(strict=True)
    except (OSError, ValueError, KeyError, TypeError):
        return None


def create_managed_transition_fixtures(
    context: dict[str, Any], project: Path, *, foreign_project: Path | None = None,
) -> tuple[ManagedTransitionFixtures | None, dict[str, Any]]:
    """Create verified new-root, full-history fork, and foreign-root choices.

    Context is the existing private OpenCode fixture worker context. The helper
    never starts a UI selection; the managed picker test owns that step. The
    returned evidence is safe for allowlisted JSON output.
    """
    checks: dict[str, bool] = {
        "project_verified": False,
        "new_root_created": False,
        "new_root_project_and_top_level_verified": False,
        "new_root_has_no_user_messages": False,
        "imported_fork_created": False,
        "fork_parent_and_nonempty_history_verified": False,
        "fork_has_no_user_messages": False,
        "foreign_project_created": False,
        "foreign_root_project_and_top_level_verified": False,
        "foreign_root_has_no_user_messages": False,
    }
    try:
        fixture_root = Path(context["root"]).resolve(strict=True)
        canonical_project = project.resolve(strict=True)
        if (not canonical_project.is_dir() or canonical_project == fixture_root):
            return None, checks
        try:
            canonical_project.relative_to(fixture_root)
        except ValueError:
            return None, checks
        checks["project_verified"] = True
        new_root_id = _create_root(context, NEW_ROOT_TITLE, canonical_project)
        checks["new_root_created"] = new_root_id is not None
        checks["new_root_project_and_top_level_verified"] = new_root_id is not None
        if new_root_id is None:
            return None, checks
        checks["new_root_has_no_user_messages"] = identity._empty_user_messages(
            context, new_root_id)[0]
        if not checks["new_root_has_no_user_messages"]:
            return None, checks

        fork_checks: dict[str, bool] = {}
        fork_root_id, fork_parent_id = recovery._forked_root_choice(
            context, canonical_project, fork_checks)
        checks["imported_fork_created"] = fork_root_id is not None
        checks["fork_parent_and_nonempty_history_verified"] = bool(
            fork_root_id is not None and fork_parent_id is not None
            and fork_checks.get("root_choice_info_verified") is True
            and fork_checks.get("fork_parent_link_verified") is True
            and fork_checks.get("fork_child_is_top_level") is True
            and fork_checks.get("fork_history_hash_verified") is True
            and fork_checks.get("fork_import_roundtrip_exact") is True)
        if not checks["fork_parent_and_nonempty_history_verified"]:
            return None, checks

        status, _body = identity._request(
            context, "PATCH", f"/api/session/{fork_root_id}", {"title": FORK_ROOT_TITLE})
        if status not in (200, 204):
            checks["fork_parent_and_nonempty_history_verified"] = False
            return None, checks
        if not identity._lookup(context, fork_root_id, canonical_project, top_level=True):
            checks["fork_parent_and_nonempty_history_verified"] = False
            return None, checks
        status, payload = identity._request(
            context, "GET", f"/api/session/{fork_root_id}")
        fork_info = identity._data(payload) if status == 200 else None
        if (not isinstance(fork_info, dict) or fork_info.get("id") != fork_root_id
                or fork_info.get("title") != FORK_ROOT_TITLE
                or not identity._session_row_matches(
                    fork_info, fork_root_id, canonical_project, top_level=True)):
            checks["fork_parent_and_nonempty_history_verified"] = False
            return None, checks
        fork_export = identity._export(context, fork_root_id)
        fork_messages = fork_export.get("messages") if fork_export else None
        fork_history_hash = identity._fork_history_digest(fork_messages)
        if not isinstance(fork_history_hash, str):
            checks["fork_parent_and_nonempty_history_verified"] = False
            return None, checks
        checks["fork_has_no_user_messages"] = identity._empty_user_messages(
            context, fork_root_id)[0]
        if not checks["fork_has_no_user_messages"]:
            return None, checks

        second_project = _foreign_project(context, foreign_project)
        checks["foreign_project_created"] = second_project is not None
        if second_project is None:
            return None, checks
        foreign_root_id = _create_root(context, FOREIGN_ROOT_TITLE, second_project)
        checks["foreign_root_project_and_top_level_verified"] = foreign_root_id is not None
        if foreign_root_id is None:
            return None, checks
        checks["foreign_root_has_no_user_messages"] = identity._empty_user_messages(
            context, foreign_root_id)[0]
        if not checks["foreign_root_has_no_user_messages"]:
            return None, checks

        fixtures = ManagedTransitionFixtures(
            project=canonical_project,
            new_root_id=new_root_id,
            new_root_title=NEW_ROOT_TITLE,
            fork_root_id=fork_root_id,
            fork_parent_id=fork_parent_id,
            fork_root_title=FORK_ROOT_TITLE,
            foreign_project=second_project,
            foreign_root_id=foreign_root_id,
            foreign_root_title=FOREIGN_ROOT_TITLE,
            new_root_sha256=_id_hash(new_root_id),
            fork_root_sha256=_id_hash(fork_root_id),
            fork_parent_sha256=_id_hash(fork_parent_id),
            foreign_root_sha256=_id_hash(foreign_root_id),
            project_sha256=_path_hash(canonical_project),
            foreign_project_sha256=_path_hash(second_project),
            fork_history_sha256=fork_history_hash,
        )
        return fixtures, checks
    except (OSError, KeyError, TypeError, ValueError):
        return None, checks


def safe_evidence(fixtures: ManagedTransitionFixtures | None,
                  checks: dict[str, Any]) -> dict[str, Any]:
    """Return the bounded, non-identifying fixture evidence schema."""
    allowed = (
        "project_verified", "new_root_created",
        "new_root_project_and_top_level_verified", "new_root_has_no_user_messages",
        "imported_fork_created",
        "fork_parent_and_nonempty_history_verified", "foreign_project_created",
        "fork_has_no_user_messages", "foreign_root_project_and_top_level_verified",
        "foreign_root_has_no_user_messages",
    )
    result: dict[str, Any] = {key: checks.get(key) is True for key in allowed}
    result["fixture_set_ready"] = fixtures is not None and all(result.values())
    if fixtures is not None:
        result.update({
            "new_root_sha256": fixtures.new_root_sha256,
            "fork_root_sha256": fixtures.fork_root_sha256,
            "fork_parent_sha256": fixtures.fork_parent_sha256,
            "foreign_root_sha256": fixtures.foreign_root_sha256,
            "project_sha256": fixtures.project_sha256,
            "foreign_project_sha256": fixtures.foreign_project_sha256,
            "fork_history_sha256": fixtures.fork_history_sha256,
        })
    return result
