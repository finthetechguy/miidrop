"""Settings kept as JSON in the user's config folder."""

import json
import os
from dataclasses import asdict, dataclass, field
from pathlib import Path

from .render import DEFAULT_SERVER


def _config_dir():
    return Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config") / "miidrop"


def _default_library():
    data = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local" / "share")
    return str(data / "miidrop" / "library")


@dataclass
class Settings:
    keys_path: str = field(default_factory=lambda: os.environ.get("MIIDROP_KEYS", ""))
    library: str = field(default_factory=_default_library)
    interface: str = ""
    name: str = "miidrop"
    pictures: bool = False
    picture_server: str = DEFAULT_SERVER

    @classmethod
    def load(cls):
        try:
            data = json.loads((_config_dir() / "settings.json").read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return cls()
        defaults = asdict(cls())
        known = {k: v for k, v in data.items() if k in defaults and type(v) is type(defaults[k])}
        return cls(**known)

    def save(self):
        d = _config_dir()
        d.mkdir(parents=True, exist_ok=True)
        tmp = d / "settings.json.tmp"
        tmp.write_text(json.dumps(asdict(self), indent=2), encoding="utf-8")
        tmp.replace(d / "settings.json")

    def keys_text(self):
        """Returns the key file's text, or None if it is unset or unreadable."""
        if not self.keys_path:
            return None
        try:
            return Path(self.keys_path).read_text(encoding="utf-8")
        except OSError:
            return None
