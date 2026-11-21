import os
import queue
import shlex
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from miidrop_app.helper import Helper, helper_command  # noqa: E402

FAKE = Path(__file__).with_name("fake_helper.py")


class HelperTest(unittest.TestCase):
    def setUp(self):
        os.environ["MIIDROP_HELPER"] = f"{shlex.quote(sys.executable)} {shlex.quote(str(FAKE))}"
        self.events = queue.Queue()
        self.helper = Helper(self.events.put)

    def tearDown(self):
        self.helper.close()
        del os.environ["MIIDROP_HELPER"]

    def next(self, kind):
        while True:
            e = self.events.get(timeout=5)
            if e["event"] == kind:
                return e

    def test_command_uses_override(self):
        argv = helper_command("wlan1")
        self.assertEqual(argv[-3:], ["serve", "-i", "wlan1"])

    def test_session(self):
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as f:
            f.write("udsBeaconKey=00\n")
        try:
            self.helper.start("wlan1", f.name)
        finally:
            os.unlink(f.name)
        self.assertTrue(self.next("hello")["active_monitor"])
        self.assertTrue(self.next("keys")["beacon"])
        self.helper.join()
        self.assertEqual(self.next("connected")["peer"], "Console")
        self.next("ready")
        self.helper.send_mii(bytes(96))
        self.assertTrue(self.next("verdict")["accepted"])
        self.helper.close()
        self.assertEqual(self.next("exit")["code"], 0)


if __name__ == "__main__":
    unittest.main()
