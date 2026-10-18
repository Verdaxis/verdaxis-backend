"""Kernel ephemeral-port authority shared by disposable producers and consumers."""
import subprocess
import sys
from pathlib import Path


class EphemeralRangeError(ValueError):
    """The kernel allocator range could not be established."""


def ephemeral_range() -> tuple[int, int]:
    """Read the current kernel range; never cache it or change host settings."""
    try:
        if sys.platform.startswith("linux"):
            raw = Path("/proc/sys/net/ipv4/ip_local_port_range").read_text()
        elif sys.platform == "darwin":
            raw = subprocess.run(
                ["sysctl", "-n", "net.inet.ip.portrange.first", "net.inet.ip.portrange.last"],
                capture_output=True, text=True, check=True,
            ).stdout
        else:
            raise EphemeralRangeError("unsupported kernel ephemeral range interface")
        fields = raw.split()
        if len(fields) != 2 or not all(field.isascii() and field.isdecimal() for field in fields):
            raise ValueError("invalid range")
        first, last = map(int, fields)
        if not 1 <= first <= last <= 65535:
            raise ValueError("invalid range bounds")
        return first, last
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        raise EphemeralRangeError("cannot read kernel ephemeral port range") from error


def is_ephemeral_port(port: int) -> bool:
    first, last = ephemeral_range()
    return first <= port <= last
