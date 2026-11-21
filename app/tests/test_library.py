import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

try:
    import miidrop
except ImportError:
    miidrop = None


def sample_bytes():
    """A blank record with a valid CRC, built without the native module."""
    data = bytearray(96)
    name = "Test".encode("utf-16le")
    data[0x1A : 0x1A + len(name)] = name
    crc = 0
    for x in data[:0x5E]:
        crc ^= x << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    data[0x5E:0x60] = crc.to_bytes(2, "big")
    return bytes(data)


@unittest.skipIf(miidrop is None, "native module not built")
class LibraryTest(unittest.TestCase):
    def setUp(self):
        from miidrop_app.library import Library

        self.dir = tempfile.TemporaryDirectory()
        self.library = Library(Path(self.dir.name) / "lib")

    def tearDown(self):
        self.dir.cleanup()

    def test_add_list_delete(self):
        record = miidrop.Record(sample_bytes())
        self.assertEqual(self.library.entries(), [])
        path = self.library.add(record)
        self.assertEqual(self.library.add(record), path)
        changed = bytearray(sample_bytes())
        changed[0x30] = 1
        other = self.library.add(miidrop.Record(bytes(changed)))
        self.assertNotEqual(other, path)
        self.assertEqual([r.name for _p, r in self.library.entries()], ["Test", "Test"])
        self.library.delete(path)
        self.assertEqual(len(self.library.entries()), 1)

    def test_skips_unreadable_files(self):
        self.library.folder.mkdir(parents=True)
        (self.library.folder / "bad.cfsd").write_bytes(b"short")
        self.assertEqual(self.library.entries(), [])

    def test_protected_files_need_keys(self):
        source = Path(self.dir.name) / "wrapped.bin"
        source.write_bytes(bytes(112))
        with self.assertRaises(ValueError):
            self.library.import_file(source)


class SettingsTest(unittest.TestCase):
    def test_round_trip(self):
        from miidrop_app.settings import Settings

        with tempfile.TemporaryDirectory() as d:
            os.environ["XDG_CONFIG_HOME"] = d
            try:
                s = Settings.load()
                s.name = "PC"
                s.keys_path = str(Path(d) / "missing.txt")
                s.save()
                loaded = Settings.load()
                self.assertEqual(loaded.name, "PC")
                self.assertIsNone(loaded.keys_text())
            finally:
                del os.environ["XDG_CONFIG_HOME"]


if __name__ == "__main__":
    unittest.main()
