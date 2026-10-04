"""Run Uvicorn with staging worker-exit diagnostics.

Uvicorn 0.51.0 has no public hook for its multiprocess worker class. The
version is pinned in ``constraints.txt``. This module replaces only that
private class, then delegates all CLI parsing and supervisor behavior to
Uvicorn.
"""

from __future__ import annotations

import logging

from uvicorn.main import main as uvicorn_main
from uvicorn.supervisors import multiprocess as uvicorn_multiprocess


logger = logging.getLogger("uvicorn.error")


class DiagnosticProcess(uvicorn_multiprocess.Process):
    """Record why a worker failed its supervisor health check."""

    def is_alive(self, timeout: float = 5) -> bool:
        if not self.process.is_alive():
            reason = "exited_before_healthcheck"
        elif self.ping(timeout):
            return True
        elif self.process.is_alive():
            reason = "alive_after_failed_healthcheck"
        else:
            reason = "exited_during_healthcheck"

        self._healthcheck_failure_reason = reason
        self._exitcode_before_parent_kill = self.process.exitcode
        logger.warning(
            "Worker healthcheck failed: reason=%s pid=%s "
            "exitcode_before_parent_kill=%s",
            reason,
            self.pid,
            self._exitcode_before_parent_kill,
        )
        return False

    def join(self) -> None:
        super().join()
        reason = getattr(self, "_healthcheck_failure_reason", None)
        if reason is not None:
            logger.warning(
                "Worker exit recorded: reason=%s pid=%s "
                "exitcode_before_parent_kill=%s final_exitcode=%s",
                reason,
                self.pid,
                self._exitcode_before_parent_kill,
                self.exitcode,
            )


def main() -> None:
    """Install the diagnostic worker class and run Uvicorn's CLI."""
    uvicorn_multiprocess.Process = DiagnosticProcess
    uvicorn_main()


if __name__ == "__main__":
    main()
