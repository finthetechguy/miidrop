import queue
import sys
import tempfile
import threading
import unittest
import urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from typing import ClassVar

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from miidrop_app.render import Renderer, image_url

PNG = b"\x89PNG\r\n\x1a\n" + bytes(16)


class Server(BaseHTTPRequestHandler):
    requests: ClassVar[list] = []

    def do_GET(self):
        url = urllib.parse.urlparse(self.path)
        Server.requests.append((url.path, urllib.parse.parse_qs(url.query)))
        ok = len(urllib.parse.parse_qs(url.query)["data"][0]) == 192
        self.send_response(200 if ok else 400)
        self.end_headers()
        self.wfile.write(PNG if ok else b"data length should be between 46-96 bytes")

    def log_message(self, *_):
        pass


class RendererTest(unittest.TestCase):
    def setUp(self):
        Server.requests = []
        self.http = HTTPServer(("127.0.0.1", 0), Server)
        threading.Thread(target=self.http.serve_forever, daemon=True).start()
        self.cache = tempfile.TemporaryDirectory()
        self.renderer = Renderer(f"http://127.0.0.1:{self.http.server_port}/", self.cache.name)

    def tearDown(self):
        self.renderer.close()
        self.http.shutdown()
        self.cache.cleanup()

    def fetch(self, data):
        got = queue.Queue()
        self.renderer.request(data, 96, got.put)
        return got.get(timeout=5)

    def test_url(self):
        url = image_url("https://example.org/", bytes([1, 2]), 96)
        self.assertTrue(url.startswith("https://example.org/miis/image.png?"))
        self.assertIn("data=0102", url)
        self.assertIn("type=face", url)

    def test_fetches_then_caches(self):
        data = bytes(96)
        self.assertIsNone(self.renderer.cached(data, 96))
        self.assertEqual(self.fetch(data), PNG)
        path, query = Server.requests[0]
        self.assertEqual(path, "/miis/image.png")
        self.assertEqual(query["width"], ["96"])
        self.assertEqual(self.renderer.cached(data, 96), PNG)
        self.assertEqual(self.fetch(data), PNG)
        self.assertEqual(len(Server.requests), 1)

    def test_errors_give_none(self):
        self.assertIsNone(self.fetch(bytes(10)))
        self.assertIsNone(self.renderer.cached(bytes(10), 96))


if __name__ == "__main__":
    unittest.main()
