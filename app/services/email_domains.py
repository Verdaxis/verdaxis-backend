"""Deterministic email-domain policy for registration tenant boundaries.

The datasets are release artifacts loaded entirely from installed wheels;
there are no DNS or network lookups in the registration path. If either
dataset is unavailable, domain-based matching is disabled for every address.
"""

from __future__ import annotations


DOMAIN_DATASET_VERSIONS = {
    "disposable-email-domains": "0.0.225",
    "free-email-domains": "1.0.2",
}

try:
    from disposable_email_domains import blocklist as _disposable_domains
    from free_email_domains import whitelist as _free_email_domains
except ImportError:
    # Fail safe: a missing packaging artifact must never turn an unknown mail
    # domain into a shared tenant boundary.
    DISPOSABLE_EMAIL_DOMAINS: frozenset[str] = frozenset()
    PUBLIC_EMAIL_DOMAINS: frozenset[str] = frozenset()
    DOMAIN_DATASETS_AVAILABLE = False
else:
    DISPOSABLE_EMAIL_DOMAINS = frozenset(_disposable_domains)
    PUBLIC_EMAIL_DOMAINS = frozenset(_free_email_domains)
    DOMAIN_DATASETS_AVAILABLE = True


def canonical_email_domain(email: str) -> str:
    return email.rsplit("@", 1)[-1].strip().lower().strip(".")


def _domain_or_parent_is_listed(domain: str, dataset: frozenset[str]) -> bool:
    labels = domain.split(".")
    return any(
        ".".join(labels[index:]) in dataset
        for index in range(max(len(labels) - 1, 1))
    )


def is_public_email_domain(domain_or_email: str) -> bool:
    value = domain_or_email.strip().lower()
    domain = canonical_email_domain(value) if "@" in value else value.strip(".")
    if not domain or "." not in domain or not DOMAIN_DATASETS_AVAILABLE:
        return True
    return _domain_or_parent_is_listed(
        domain, PUBLIC_EMAIL_DOMAINS
    ) or _domain_or_parent_is_listed(domain, DISPOSABLE_EMAIL_DOMAINS)


def registration_organization_domain(email: str) -> str | None:
    domain = canonical_email_domain(email)
    return None if is_public_email_domain(domain) else domain
