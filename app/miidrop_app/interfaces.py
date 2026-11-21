"""Finding the wireless cards miidrop can use."""

from pathlib import Path


def wireless_interfaces():
    """Returns (name, connected) for each wireless interface, unconnected ones first."""
    out = []
    for dev in sorted(Path("/sys/class/net").glob("*")):
        if not (dev / "phy80211").exists() or dev.name.startswith("miidrop"):
            continue
        try:
            carrier = (dev / "carrier").read_text().strip() == "1"
        except OSError:
            carrier = False
        out.append((dev.name, carrier))
    out.sort(key=lambda e: (e[1], e[0]))
    return out
