"""Local paced MPEG-TS fixture server. Never point this harness at provider URLs."""
import http.server
import pathlib
import subprocess
import sys
import threading
import urllib.parse

ROOT = pathlib.Path(__file__).parent
ASSETS = pathlib.Path(sys.argv[1])
lock = threading.Lock()
active = 0
peak = 0

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        global active, peak
        path = urllib.parse.urlsplit(self.path).path
        if path == '/stream.ts':
            process = None
            with lock:
                active += 1
                peak = max(peak, active)
                print(f'STREAM_OPEN active={active} peak={peak}', flush=True)
            try:
                self.send_response(200)
                self.send_header('Content-Type', 'video/mp2t')
                self.end_headers()
                process = subprocess.Popen([
                    'ffmpeg', '-nostdin', '-hide_banner', '-loglevel', 'error', '-re',
                    '-i', str(ASSETS / 'fixture.ts'), '-map', '0', '-c', 'copy',
                    '-f', 'mpegts', 'pipe:1'
                ], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
                while data := process.stdout.read(188 * 64):
                    self.wfile.write(data)
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                pass
            finally:
                if process:
                    process.stdout.close()
                    process.terminate()
                    try:
                        process.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
                with lock:
                    active -= 1
                    print(f'STREAM_CLOSE active={active} peak={peak}', flush=True)
        else:
            file = ASSETS / 'mpegts.js' if path == '/mpegts.js' else ROOT / 'index.html'
            data = file.read_bytes()
            self.send_response(200)
            self.send_header('Content-Type', 'application/javascript' if path == '/mpegts.js' else 'text/html')
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)

http.server.ThreadingHTTPServer(('127.0.0.1', int(sys.argv[2]) if len(sys.argv) > 2 else 18765), Handler).serve_forever()
