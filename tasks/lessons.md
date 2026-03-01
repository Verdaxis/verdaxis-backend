# Lessons — Verdaxis Backend
<!-- Self-improvement-loop: Add corrections here as Trigger → Rule → Why -->
<!-- Read at session start. Write after ANY user correction. -->

## Format
- **Date:** YYYY-MM-DD
- **Trigger:** What happened
- **Rule:** What to do instead
- **Why:** Root cause

## Security

- **Date:** 2026-02-18
- **Trigger:** python-jose 3.5.0 is unmaintained (last release 2022) with known CVEs.
- **Rule:** Use PyJWT or joserfc instead of python-jose.
- **Why:** Unmaintained JWT libraries carry unpatched CVEs.

- **Date:** 2026-02-18
- **Trigger:** passlib 1.7.4 is unmaintained (last release 2020) and uses the deprecated crypt module.
- **Rule:** Use bcrypt directly or argon2-cffi for password hashing instead of passlib.
- **Why:** passlib's crypt dependency is removed in Python 3.13.

- **Date:** 2026-02-18
- **Trigger:** postgres_data/ directory was not in .gitignore.
- **Rule:** Gitignore data directories before they can be accidentally committed.
- **Why:** Committed database files expose data and permanently bloat the repo.

## Python

- **Date:** 2026-02-18
- **Trigger:** Multiple uses of datetime.utcnow() across the codebase.
- **Rule:** Always use datetime.now(timezone.utc) instead of datetime.utcnow().
- **Why:** Deprecated in Python 3.12, removed in 3.14; returns naive datetimes causing timezone bugs.

## Architecture

- **Date:** 2026-02-18
- **Trigger:** Two different get_current_user implementations existed in core/auth.py and routers/auth_simple.py.
- **Rule:** Maintain a single auth implementation; consolidate before adding new patterns.
- **Why:** Dual auth implementations cause inconsistent security behavior and maintenance hazards.

- **Date:** 2026-02-18
- **Trigger:** Redis container was running but no code referenced Redis anywhere.
- **Rule:** Remove unused services from docker-compose.yml.
- **Why:** Unused services consume memory, create attack surface, and confuse developers.

## Dependencies

- **Date:** 2026-02-18
- **Trigger:** Most dependencies used unpinned >= specifiers in requirements.txt.
- **Rule:** Pin all production dependencies to exact versions and use pip-compile for lockfiles.
- **Why:** Unpinned deps can break silently on incompatible upstream releases.
