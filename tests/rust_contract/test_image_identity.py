from scripts.rust_contract.image_identity import matches_pinned_image
from scripts.rust_contract.run_disposable import IMAGE


def test_exact_docker_and_podman_image_names():
    normalized = "docker.io/postgis/postgis@" + IMAGE.rsplit("@", 1)[1]
    assert matches_pinned_image(IMAGE, IMAGE)
    assert matches_pinned_image(normalized, IMAGE)
    for invalid in (
        "postgis/postgis:17-3.6-alpine",
        normalized.replace("docker.io", "untrusted.invalid"),
        normalized.replace("postgis/postgis", "other/postgis"),
        normalized[:-1] + ("0" if normalized[-1] != "0" else "1"),
    ):
        assert not matches_pinned_image(invalid, IMAGE)
