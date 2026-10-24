#!/usr/bin/env python3
"""Bounded development-fixture observer-health and reconciliation probe.

This is diagnostic glue for the private OpenCode TUI fixture. It does not
provide a production bridge, durable acknowledgment, or crash recovery.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import time
from typing import Any

try:
    from scripts import probe_opencode_v2_tui_identity as tui
except ModuleNotFoundError:
    import probe_opencode_v2_tui_identity as tui


PINNED_VERSION = "2.0.32"
MAX_RECORD_BYTES = 2 * 1024 * 1024
MAX_RECORDS = 1000
LEASE_SECONDS = 1.0
CASE_SECONDS = 6.0
POLL_SECONDS = 0.1


class ObserverHealth:
    """Track one scoped observer using only host-side receipt time."""

    def __init__(self, *, nonce: str, project_hash: str, version: str = PINNED_VERSION):
        self.nonce = nonce
        self.project_hash = project_hash
        self.version = version
        self.instance: str | None = None
        self.sequence = 0
        self.received_at: float | None = None
        self.identity: tuple[str, str] | None = None
        self.state = "unobserved"
        self.ambiguous = False
        self.reconciliation_watermark: int | None = None
        self.durable = False
        self.advances = 0
        self.invalid = False
        self.last_record: tuple[Any, ...] | None = None
        self.identity_established = False

    def expire(self, now: float) -> bool:
        if self.received_at is None:
            return True
        if now - self.received_at >= LEASE_SECONDS:
            self.state = "stale"
            return True
        return False

    def accept_stream(self, payload: bytes, *, now: float,
                      version: str = PINNED_VERSION) -> bool:
        """Consume all new contiguous rows; duplicate rereads never renew."""
        self.expire(now)
        if version != self.version or len(payload) > MAX_RECORD_BYTES:
            self._lose_scope()
            return False
        if not payload:
            return False
        records, rejected = tui.parse_client_records(
            payload, client_nonce=self.nonce, project_hash=self.project_hash,
            enforce_gap_bound=False)
        if rejected or not records:
            self._lose_scope()
            return False
        # The parser validates the entire file, including one stable instance,
        # before this raw field is read. Raw rows are never returned or reported.
        try:
            final_raw = json.loads(payload.splitlines()[-1])
            instance = final_raw.get("tui_instance")
        except (json.JSONDecodeError, UnicodeDecodeError, AttributeError, IndexError):
            self._lose_scope()
            return False
        latest = records[-1]
        latest_tuple = self._record_tuple(latest)
        seq = latest["seq"]
        if (not isinstance(instance, str) or len(instance) != 32
                or latest["project_hash"] != self.project_hash
                or (self.instance is not None and instance != self.instance)
                or (self.sequence and seq < self.sequence)):
            self._lose_scope()
            return False
        new_records = [record for record in records if record["seq"] > self.sequence]
        if not new_records:
            if self.sequence and seq == self.sequence and latest_tuple != self.last_record:
                self._lose_scope()
            return False
        if new_records[0]["seq"] != self.sequence + 1:
            self._lose_scope()
            return False
        # The parser has validated the entire file as one contiguous sequence;
        # consume every row after our previous watermark, including batches.
        self.instance = instance
        self.sequence = seq
        self.received_at = now
        self.advances += len(new_records)
        self.last_record = latest_tuple
        lost_identity = False
        for row in new_records:
            selected = (row["route_name"] == "session" and row["session_top_level"] is True
                        and row["project_hash"] == self.project_hash
                        and isinstance(row["session_id_hash"], str))
            if not selected and self.identity_established:
                lost_identity = True
            elif selected:
                self.identity_established = True
        if lost_identity:
            self.lose()
            self.reconciliation_watermark = None
        if (latest["route_name"] == "session" and latest["session_top_level"] is True
                and latest["project_hash"] == self.project_hash
                and isinstance(latest["session_id_hash"], str)):
            self.identity = (latest["session_id_hash"], self.project_hash)
        else:
            self.identity = None
        if (not lost_identity and self.reconciliation_watermark is not None
                and seq > self.reconciliation_watermark):
            self.state = "observed"
            self.ambiguous = False
            self.invalid = False
            self.reconciliation_watermark = None
        elif self.state == "unobserved":
            self.state = "observed"
        elif self.state == "stale":
            # Fresh polling after loss renews receipt age but cannot revive the
            # current identity until an explicit reconciliation request gates
            # a later, distinct sequence.
            self.ambiguous = True
        return True

    def lose(self) -> None:
        self.state = "stale"
        self.ambiguous = True

    def _lose_scope(self) -> None:
        self.state = "stale"
        self.ambiguous = True
        self.invalid = True

    @staticmethod
    def _record_tuple(record: dict[str, Any]) -> tuple[Any, ...]:
        return tuple(record.get(field) for field in (
            "seq", "monotonic_ms", "route_name", "session_id_hash",
            "session_top_level", "project_hash", "kind"))

    def request_reconciliation(self) -> int:
        """Require a later fresh observation before clearing ambiguity."""
        self.state = "stale"
        self.ambiguous = True
        self.reconciliation_watermark = self.sequence
        return self.sequence

    def snapshot(self, now: float) -> dict[str, Any]:
        self.expire(now)
        eligible = self.state == "observed" and not self.ambiguous and self.identity is not None
        return {
            "state": self.state,
            "current_identity_eligible": eligible,
            "durable": self.durable,
            "ambiguous": self.ambiguous,
            "sequence": self.sequence,
            "advance_count": self.advances,
        }


def bounded_read(path: Path) -> bytes | None:
    try:
        with path.open("rb") as stream:
            payload = stream.read(MAX_RECORD_BYTES + 1)
        return payload if len(payload) <= MAX_RECORD_BYTES else None
    except OSError:
        return None


def _monitor(path: Path, nonce: str, project_hash: str, *, version: str = PINNED_VERSION
             ) -> tuple[ObserverHealth, bool]:
    monitor = ObserverHealth(nonce=nonce, project_hash=project_hash, version=version)
    payload = bounded_read(path)
    accepted = payload is not None and monitor.accept_stream(
        payload, now=time.monotonic(), version=version)
    return monitor, bool(accepted)


def _alive(tuis: list[Any]) -> bool:
    try:
        return len(tuis) == 2 and all(tui_process.alive() for tui_process in tuis)
    except (AttributeError, OSError, RuntimeError):
        return False


def _server_owned(context: dict[str, Any]) -> bool:
    try:
        pid = int(context["server_pid"])
        port = int(context["port"])
        if pid <= 0 or port <= 0:
            return False
        return bool(tui.bootstrap.server_owns_port(port, pid))
    except (AttributeError, KeyError, TypeError, ValueError, OSError):
        return False


def after_routes(context: dict[str, Any]) -> dict[str, Any]:
    """Exercise bounded loss, expiry, and explicit fresh reconciliation."""
    started = time.monotonic()
    result: dict[str, Any] = {
        "classification": "not_proven", "reason": "health_case_not_completed",
        "host_receipt_expiry_observed": False,
        "passive_observer_remained_fresh": False,
        "both_tuis_remained_alive": False,
        "owned_server_remained_alive": False,
        "automatic_promotion_after_resume": False,
        "explicit_fresh_reconciliation_observed": False,
        "current_identity_eligible_after_reconciliation": False,
        "durable": False,
        "active_record_advances": 0, "passive_record_advances": 0,
        "elapsed_ms": 0.0,
    }
    control_path: Path | None = None
    paused = False
    try:
        record_paths = context["record_paths"]
        nonces = context["nonces"]
        project_hash = context["project_hash"]
        tuis = context["tuis"]
        hashes = context["hashes"]
        control_paths = context["control_paths"]
        if (len(record_paths) != 2 or len(nonces) != 2 or len(tuis) != 2
                or len(control_paths) < 1 or len(hashes) != 2
                or context.get("version", PINNED_VERSION) != PINNED_VERSION):
            result["reason"] = "fixture_scope_unavailable"
            return result
        control_path = Path(control_paths[0])
        active, active_ready = _monitor(Path(record_paths[0]), nonces[0], project_hash)
        passive, passive_ready = _monitor(Path(record_paths[1]), nonces[1], project_hash)
        if (not active_ready or not passive_ready or active.identity != (hashes[0], project_hash)
                or passive.identity != (hashes[1], project_hash)):
            result["reason"] = "initial_fresh_scoped_observations_missing"
            return result

        # Fail closed if the requested control target is not the first private
        # fixture path or is not a regular, private file prepared by the driver.
        parent_metadata = control_path.parent.stat()
        if parent_metadata.st_mode & 0o077 or control_path.exists():
            result["reason"] = "private_control_path_unavailable"
            return result
        control_fd = os.open(control_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        paused = True
        try:
            if os.write(control_fd, b"pause") != 5:
                result["reason"] = "private_control_write_failed"
                return result
        finally:
            os.close(control_fd)
        deadline = min(started + CASE_SECONDS, context["deadline"])
        passive_advanced_at = passive.sequence
        active_expired = False
        while time.monotonic() < deadline:
            context["pump"]()
            now = time.monotonic()
            active_payload = bounded_read(Path(record_paths[0]))
            passive_payload = bounded_read(Path(record_paths[1]))
            if active_payload is None or passive_payload is None:
                active.lose()
                passive.lose()
                result["reason"] = "bounded_record_read_failed"
                return result
            active.accept_stream(active_payload, now=now)
            passive.accept_stream(passive_payload, now=now)
            if active.invalid:
                result["reason"] = "active_observer_data_invalid_during_loss_case"
                return result
            if passive.invalid:
                result["reason"] = "passive_observer_data_invalid"
                return result
            if (passive.expire(now) or passive.state != "observed"
                    or passive.identity != (hashes[1], project_hash)):
                result["reason"] = "passive_observer_stale_or_ambiguous"
                return result
            if passive.sequence > passive_advanced_at:
                passive_advanced_at = passive.sequence
            both_alive = _alive(tuis)
            server_alive = _server_owned(context)
            if not both_alive or not server_alive:
                result["reason"] = "fixture_process_or_listener_lost"
                return result
            if active.expire(now):
                active_expired = True
                break
            time.sleep(min(POLL_SECONDS, max(0.0, deadline - time.monotonic())))
        if not active_expired:
            result["reason"] = "active_observer_did_not_expire_within_bound"
            return result
        result["host_receipt_expiry_observed"] = True
        result["both_tuis_remained_alive"] = _alive(tuis)
        result["owned_server_remained_alive"] = _server_owned(context)
        result["passive_observer_remained_fresh"] = (
            passive.state == "observed" and not passive.ambiguous
            and passive.identity == (hashes[1], project_hash))
        if not all((result["both_tuis_remained_alive"], result["owned_server_remained_alive"],
                    result["passive_observer_remained_fresh"])):
            result["reason"] = "health_loss_not_isolated_to_active_observer"
            return result

        control_path.unlink()
        paused = False
        # The first post-resume row proves delivery resumed but leaves identity
        # stale. Reconciliation is requested only after that row is observed.
        resumed = False
        while time.monotonic() < deadline:
            context["pump"]()
            now = time.monotonic()
            payload = bounded_read(Path(record_paths[0]))
            passive_payload = bounded_read(Path(record_paths[1]))
            if payload is None or passive_payload is None:
                result["reason"] = "resume_record_read_failed"
                return result
            advanced = active.accept_stream(payload, now=now)
            passive.accept_stream(passive_payload, now=now)
            if active.invalid or passive.invalid:
                result["reason"] = "observer_data_invalid_after_resume"
                return result
            if (passive.expire(now) or passive.state != "observed"
                    or passive.identity != (hashes[1], project_hash) or not _alive(tuis)
                    or not _server_owned(context)):
                result["reason"] = "passive_or_process_health_lost_after_resume"
                return result
            if advanced:
                resumed = True
                break
            time.sleep(min(POLL_SECONDS, max(0.0, deadline - time.monotonic())))
        result["automatic_promotion_after_resume"] = bool(
            resumed and active.snapshot(time.monotonic())["current_identity_eligible"])
        if not resumed:
            result["reason"] = "fresh_active_observation_after_resume_missing"
            return result
        watermark = active.request_reconciliation()
        reconciled = False
        while time.monotonic() < deadline:
            context["pump"]()
            now = time.monotonic()
            payload = bounded_read(Path(record_paths[0]))
            passive_payload = bounded_read(Path(record_paths[1]))
            if payload is None or passive_payload is None:
                result["reason"] = "reconciliation_record_read_failed"
                return result
            active.accept_stream(payload, now=now)
            passive.accept_stream(passive_payload, now=now)
            if active.invalid or passive.invalid:
                result["reason"] = "observer_data_invalid_during_reconciliation"
                return result
            if (passive.expire(now) or passive.state != "observed"
                    or passive.identity != (hashes[1], project_hash) or not _alive(tuis)
                    or not _server_owned(context)):
                result["reason"] = "passive_or_process_health_lost_during_reconciliation"
                return result
            if active.state == "observed" and active.sequence > watermark:
                reconciled = True
                break
            time.sleep(min(POLL_SECONDS, max(0.0, deadline - time.monotonic())))
        state = active.snapshot(time.monotonic())
        result.update({
            "explicit_fresh_reconciliation_observed": reconciled,
            "current_identity_eligible_after_reconciliation": state["current_identity_eligible"],
            "passive_observer_remained_fresh": (
                passive.state == "observed" and not passive.ambiguous
                and passive.identity == (hashes[1], project_hash)),
            "active_record_advances": active.advances,
            "passive_record_advances": passive.advances,
        })
        if reconciled and state["current_identity_eligible"] and not state["durable"]:
            result.update(classification="proven",
                          reason="development_observer_expiry_and_manual_reconciliation_observed")
        else:
            result["reason"] = "explicit_fresh_reconciliation_not_observed"
        return result
    except (KeyError, OSError, RuntimeError, TypeError, ValueError, TimeoutError):
        result["reason"] = "bounded_health_case_failed_closed"
        return result
    finally:
        if paused and control_path is not None:
            try:
                control_path.unlink(missing_ok=True)
            except OSError:
                pass
        result["elapsed_ms"] = round((time.monotonic() - started) * 1000.0, 3)
