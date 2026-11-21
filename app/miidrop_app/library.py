"""The folder of saved Miis, one 96-byte record per .cfsd file."""

from pathlib import Path

import miidrop

SUFFIX = ".cfsd"


class Library:
    def __init__(self, folder):
        self.folder = Path(folder)

    def entries(self):
        """Returns (path, record) pairs sorted by name, skipping unreadable files."""
        if not self.folder.is_dir():
            return []
        out = []
        for path in self.folder.glob("*" + SUFFIX):
            try:
                out.append((path, miidrop.Record(path.read_bytes())))
            except (OSError, ValueError):
                continue
        out.sort(key=lambda e: (e[1].name.casefold(), e[0].name))
        return out

    def add(self, record):
        """Saves a record under its Mii ID, never overwriting another file."""
        self.folder.mkdir(parents=True, exist_ok=True)
        base = f"{record.mii_id:08x}"
        path = self.folder / (base + SUFFIX)
        n = 2
        while path.exists():
            if path.read_bytes() == bytes(record):
                return path
            path = self.folder / f"{base}-{n}{SUFFIX}"
            n += 1
        path.write_bytes(bytes(record))
        return path

    def import_file(self, source, keys_text=None):
        record = miidrop.read(Path(source).read_bytes(), keys_text)
        return self.add(record), record

    @staticmethod
    def export(record, target):
        Path(target).write_bytes(bytes(record))

    @staticmethod
    def delete(path):
        Path(path).unlink(missing_ok=True)
