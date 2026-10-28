"""Recorded immutable test runtime references; never fall back to a tag."""
IMAGES = {
    'amd64': 'sha256:e0605c4e3b06d7bb3ac182d9d6dbb184647625c5e21b88f3eaca5230d64f1efb',
    'arm64': 'sha256:5165bc002d5ec7a9fe44d6e560c0a9aea36e5bbcea7aa48f8cd3a072f59fcf02',
}


def runtime_image(architecture):
    if architecture not in IMAGES:
        raise ValueError('Unrecorded test runtime architecture')
    return IMAGES[architecture], 'linux/' + architecture
