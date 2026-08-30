#!/usr/bin/env python3

import argparse
import datetime
import http.server


class FixtureHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        host = self.headers.get("Host", "fixture.invalid")
        if self.path == "/catalog.m3u":
            body = (
                "#EXTM3U\n"
                '#EXTINF:-1 tvg-id="fixture" group-title="Fixture",Fixture Channel\n'
                f"http://{host}/live.ts\n"
            ).encode()
            self._send("application/vnd.apple.mpegurl", body)
            return
        if self.path == "/guide.xml":
            now = datetime.datetime.now(datetime.UTC)
            start = (now - datetime.timedelta(hours=1)).strftime("%Y%m%d%H%M%S +0000")
            stop = (now + datetime.timedelta(hours=1)).strftime("%Y%m%d%H%M%S +0000")
            body = (
                '<?xml version="1.0" encoding="UTF-8"?>'
                '<tv><channel id="fixture"><display-name>Fixture Channel</display-name></channel>'
                f'<programme start="{start}" stop="{stop}" channel="fixture">'
                "<title>Fixture Programme</title></programme></tv>"
            ).encode()
            self._send("application/xml", body)
            return
        if self.path == "/live.ts":
            self._send("video/mp2t", bytes([0x47]) + bytes(187) * 64)
            return
        self.send_error(404)

    def log_message(self, format: str, *args: object) -> None:
        return

    def _send(self, content_type: str, body: bytes) -> None:
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", required=True, type=int)
    arguments = parser.parse_args()
    server = http.server.ThreadingHTTPServer(("0.0.0.0", arguments.port), FixtureHandler)
    server.serve_forever()


if __name__ == "__main__":
    main()
