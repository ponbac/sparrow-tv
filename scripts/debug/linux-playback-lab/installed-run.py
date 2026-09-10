"""Measure an opt-in installed candidate using a private, single-channel M3U.

The catalog is served to Rust on loopback. Provider URLs never enter process
arguments or the probe. Raw player output/screenshots remain in a private
directory; review them before sharing. Samples are diagnostic, not acceptance.
"""
import argparse
import hashlib
import http.server
import json
import os
import pathlib
import signal
import socket
import subprocess
import tempfile
import threading
import time


def mpv_sample(path):
    properties = ['time-pos', 'decoder-frame-drop-count', 'frame-drop-count',
                  'estimated-vf-fps', 'hwdec-current', 'video-codec', 'audio-codec-name',
                  'avsync', 'paused-for-cache', 'idle-active']
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(1)
        connection.connect(str(path))
        with connection.makefile('rwb') as stream:
            result = {}
            for sequence, prop in enumerate(properties):
                stream.write((json.dumps({'command': ['get_property', prop],
                                          'request_id': sequence}) + '\n').encode())
                stream.flush()
                while line := stream.readline():
                    reply = json.loads(line)
                    if reply.get('request_id') == sequence:
                        result[prop] = reply.get('data')
                        break
            return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app', type=pathlib.Path, required=True)
    parser.add_argument('--catalog', type=pathlib.Path, required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--seconds', type=int, default=65)
    parser.add_argument('--switch-players', action='store_true', help='Exercise Open in mpv at 20s and Play in app at 50s')
    parser.add_argument('--only', default='x11-mse-disable,x11-mse-shm,x11-mpv-shm')
    args = parser.parse_args()
    if not 10 <= args.seconds <= 900:
        parser.error('--seconds must be between 10 and 900')
    variants = []
    for name in args.only.split(','):
        parts = name.split('-')
        if len(parts) != 3 or parts[0] not in ['x11', 'wayland'] or parts[1] not in ['default', 'mse', 'mpv'] or parts[2] not in ['disable', 'shm']:
            parser.error('Variants must be x11|wayland-default|mse|mpv-disable|shm')
        variants.append((name, *parts))
    app = args.app.resolve(strict=True)
    catalog = args.catalog.read_bytes()
    if sum(line.startswith(b'#EXTINF:') for line in catalog.splitlines()) != 1:
        parser.error('Use exactly one Channel with a private sample alias')
    out = args.output.resolve()
    out.mkdir(mode=0o700, parents=True, exist_ok=True)
    out.chmod(0o700)

    class Catalog(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            if self.path != '/sample.m3u':
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header('Content-Length', str(len(catalog)))
            self.end_headers()
            self.wfile.write(catalog)

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Catalog)
    thread = threading.Thread(target=server.serve_forever)
    thread.start()
    summaries = []
    try:
        for name, backend, engine, policy in variants:
            print(f'RUN {name}', flush=True)
            with tempfile.TemporaryDirectory(prefix='sp-lab-') as temporary:
                profile = pathlib.Path(temporary)
                private = profile / 'xyz.ponbac.sparrow/private-v1'
                private.mkdir(mode=0o700, parents=True)
                config = private / 'source-configuration.json'
                config.write_text(json.dumps({'version': 1, 'm3uLocation':
                    f'http://127.0.0.1:{server.server_port}/sample.m3u', 'epgLocation': None}))
                config.chmod(0o600)
                env = os.environ.copy()
                for key in ['LIBGL_ALWAYS_SOFTWARE', 'WEBKIT_DISABLE_DMABUF_RENDERER',
                            'WEBKIT_DMABUF_RENDERER_FORCE_SHM', 'SPARROW_LINUX_PLAYBACK_ENGINE', 'SPARROW_PLAYBACK_LAB_SWITCH_PLAYERS']:
                    env.pop(key, None)
                # External mpv explicitly uses Wayland even when the GTK app is X11.
                if backend == 'wayland': env.pop('DISPLAY', None)
                env.update(XDG_DATA_HOME=str(profile), GDK_BACKEND=backend,
                           SPARROW_PLAYBACK_LAB_SECONDS=str(args.seconds))
                if args.switch_players: env['SPARROW_PLAYBACK_LAB_SWITCH_PLAYERS'] = '1'
                if engine != 'default': env['SPARROW_LINUX_PLAYBACK_ENGINE'] = engine
                env['WEBKIT_DISABLE_DMABUF_RENDERER' if policy == 'disable' else 'WEBKIT_DMABUF_RENDERER_FORCE_SHM'] = '1'
                log_path = out / (name + '.log')
                measurements = []
                windows = []
                with log_path.open('w') as log:
                    proc = subprocess.Popen([str(app)], env=env, stdout=log,
                                            stderr=subprocess.STDOUT, start_new_session=True)
                    started = time.monotonic()
                    timed_out = False
                    try:
                        captured = False
                        while proc.poll() is None and time.monotonic() - started < args.seconds + 45:
                            elapsed = time.monotonic() - started
                            for ipc in private.glob('mpv-v1/*.sock'):
                                try:
                                    measurements.append({'elapsed': round(elapsed, 2), **mpv_sample(ipc)})
                                except (OSError, ValueError):
                                    pass  # A socket disappearing during stop is expected.
                            if elapsed >= 15 and not captured and os.environ.get('HYPRLAND_INSTANCE_SIGNATURE'):
                                clients = json.loads(subprocess.check_output(['hyprctl', 'clients', '-j'], text=True))
                                windows = [{key: c[key] for key in ['pid', 'at', 'size', 'xwayland', 'fullscreen']}
                                           for c in clients if c['pid'] == proc.pid]
                                for window in windows:
                                    x, y = window['at']; width, height = window['size']
                                    subprocess.run(['grim', '-g', f'{x},{y} {width}x{height}',
                                                    str(out / (name + '.png'))], check=True)
                                captured = True
                                # Record actual loaded media/graphics libraries, including WebKit subprocesses.
                                libraries = set()
                                for candidate in pathlib.Path('/proc').glob('[0-9]*'):
                                    try:
                                        if os.getpgid(int(candidate.name)) == proc.pid:
                                            for line in (candidate / 'maps').read_text().splitlines():
                                                path = line.split()[-1]
                                                if path.startswith('/') and any(s in path.lower() for s in ['webkit', 'gstreamer', '/gstreamer-', 'libgst', 'libmpv', 'libegl', 'libglx', '/dri/']):
                                                    libraries.add(path)
                                    except (OSError, ProcessLookupError):
                                        pass
                                (out / (name + '-libraries.json')).write_text(json.dumps(sorted(libraries), indent=2) + '\n')
                            time.sleep(1)
                        if proc.poll() is None:
                            timed_out = True
                            proc.terminate()
                            proc.wait(timeout=5)
                    finally:
                        # Stop the process group too if the app or WebKit failed to tear down.
                        try: os.killpg(proc.pid, signal.SIGTERM)
                        except ProcessLookupError: pass
                        try: proc.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            os.killpg(proc.pid, signal.SIGKILL); proc.wait()
                lines = log_path.read_text(errors='replace').splitlines()
                samples = [json.loads(line.removeprefix('PLAYBACK_LAB ')) for line in lines
                           if line.startswith('PLAYBACK_LAB {')]
                (out / (name + '-samples.json')).write_text(json.dumps(samples, indent=2) + '\n')
                (out / (name + '-mpv.json')).write_text(json.dumps(measurements, indent=2) + '\n')
                summary = {'variant': name, 'exit_code': proc.returncode,
                           'finished': 'PLAYBACK_LAB finished' in lines, 'samples': len(samples),
                           'timed_out': timed_out,
                           'playing_samples': sum(sample['playing'] for sample in samples),
                           'failed_samples': sum(sample['failed'] for sample in samples),
                           'windows': windows, 'remaining_mpv_sockets': len(list(private.glob('mpv-v1/*.sock')))}
                summaries.append(summary)
                print(json.dumps(summary), flush=True)
                (out / 'summary.json').write_text(json.dumps(summaries, indent=2) + '\n')
            time.sleep(2)
    finally:
        server.shutdown(); server.server_close(); thread.join()
    with app.open('rb') as binary:
        digest = hashlib.file_digest(binary, 'sha256').hexdigest()
    (out / 'artifact.json').write_text(json.dumps({'sha256': digest,
                                                'seconds': args.seconds}, indent=2) + '\n')
    return 1 if any(s['exit_code'] or not s['finished'] or not s['samples'] or s['remaining_mpv_sockets'] for s in summaries) else 0


if __name__ == '__main__':
    raise SystemExit(main())
