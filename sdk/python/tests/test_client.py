import json
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer

from purrcode import PurrCodeClient


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.headers.get("Authorization") != "Bearer " + ("x" * 64):
            self.send_response(401)
            self.end_headers()
            return
        payload = json.dumps(
            [{"id": "fixture", "status_code": "completed", "event_count": 1}]
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *_args):
        pass


class ClientTest(unittest.TestCase):
    def test_authenticated_session_list(self):
        server = HTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            client = PurrCodeClient(
                "http://127.0.0.1:{}".format(server.server_port), "x" * 64
            )
            self.assertEqual(client.sessions()[0]["status_code"], "completed")
        finally:
            server.shutdown()
            thread.join()
            server.server_close()


if __name__ == "__main__":
    unittest.main()
