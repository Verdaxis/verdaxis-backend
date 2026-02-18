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
- **Trigger:** python-jose 3.5.0 is unmaintained (last release 2022) with known CVEs
- **Rule:** Prefer actively maintained libraries. Use PyJWT or joserfc instead of python-jose.
- **Why:** Unmaintained JWT libraries are a security liability. Known vulnerabilities won't be patched.

- **Date:** 2026-02-18
- **Trigger:** passlib 1.7.4 is unmaintained (last release 2020), uses deprecated crypt module
- **Rule:** Use bcrypt directly or argon2-cffi instead of passlib for password hashing.
- **Why:** passlib's crypt dependency will be removed in Python 3.13, breaking the library.

- **Date:** 2026-02-18
- **Trigger:** postgres_data/ directory not in .gitignore
- **Rule:** Always gitignore data directories, especially database files. Add them BEFORE they can be accidentally committed.
- **Why:** Committing raw database files exposes data and bloats the repo permanently.

## Python

- **Date:** 2026-02-18
- **Trigger:** 27 occurrences of datetime.utcnow() across models, routers, core, and tests
- **Rule:** Always use datetime.now(timezone.utc) instead of datetime.utcnow().
- **Why:** Deprecated in Python 3.12, removed in 3.14. Returns naive datetimes causing timezone bugs.

## Architecture

- **Date:** 2026-02-18
- **Trigger:** Dual auth system — two different get_current_user implementations (core/auth.py and routers/auth_simple.py)
- **Rule:** Maintain a single auth implementation. When refactoring auth, consolidate before adding new patterns.
- **Why:** Dual implementations cause inconsistent security behavior and maintenance hazards.

- **Date:** 2026-02-18
- **Trigger:** Redis container running but no code references Redis anywhere
- **Rule:** Don't provision infrastructure you're not using. Remove unused services from docker-compose.yml.
- **Why:** Unused services consume memory, create attack surface, and confuse new developers.

## Dependencies

- **Date:** 2026-02-18
- **Trigger:** Most dependencies use unpinned >= specifiers in requirements.txt
- **Rule:** Pin all production dependencies to exact versions. Use pip-compile for lockfiles.
- **Why:** Unpinned deps can break silently when upstream releases incompatible changes.
