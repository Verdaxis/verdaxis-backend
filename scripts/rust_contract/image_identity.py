"""Exact pinned image identity across Docker and Podman inspect output."""


def matches_pinned_image(actual: str, expected: str) -> bool:
    repository_and_tag, digest = expected.rsplit("@", 1)
    repository = repository_and_tag.rsplit(":", 1)[0]
    return actual in (expected, f"docker.io/{repository}@{digest}")
