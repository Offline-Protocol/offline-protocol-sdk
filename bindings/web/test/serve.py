# The test server: a static server whose every response is no-store, so a
# page offline can load only what a service worker cached (W15). Python's
# own server sends Last-Modified and nothing else, which lets a browser
# answer from its HTTP cache offline and pass a page the service worker
# would have failed.
#
# Usage: python3 test/serve.py <port> <directory>
import functools
import http.server
import sys


class NoStore(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def log_message(self, *args):
        pass


port, directory = int(sys.argv[1]), sys.argv[2]
handler = functools.partial(NoStore, directory=directory)
http.server.ThreadingHTTPServer(("127.0.0.1", port), handler).serve_forever()
