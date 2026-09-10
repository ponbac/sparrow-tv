"""Run playback experiments on virtual displays or the current desktop.

Usage: python3 scripts/debug/linux-playback-lab/run.py [--output /tmp/sparrow-playback-lab]
System dependencies: gcc, pkg-config, GTK3/WebKitGTK4.1/libmpv/epoxy development
packages, GStreamer codec + GL plugins, ffmpeg, Xvfb, Weston, ImageMagick.
"""
import argparse
import json
import os
import pathlib
import shlex
import shutil
import subprocess
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=pathlib.Path, default=pathlib.Path('/tmp/sparrow-playback-lab'))
parser.add_argument('--only', help='Comma-separated variant names to run')
parser.add_argument('--desktop', action='store_true', help='Use the current desktop and its GL implementation (no Xvfb/Weston)')
parser.add_argument('--prepare-only', action='store_true', help='Download/generate the fixture and compile, without opening windows')
args = parser.parse_args()
out = args.output.resolve()
out.mkdir(parents=True, exist_ok=True)
runtime = out / 'runner-runtime'
runtime.mkdir(mode=0o700, exist_ok=True)

if not (out / 'mpegts.js').exists():
    urllib.request.urlretrieve('https://cdn.jsdelivr.net/npm/mpegts.js@1.7.3/dist/mpegts.js', out / 'mpegts.js')
if not (out / 'fixture.ts').exists():
    subprocess.run(['ffmpeg', '-nostdin', '-hide_banner', '-loglevel', 'error',
        '-f', 'lavfi', '-i', 'testsrc2=size=1280x720:rate=30',
        '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000', '-t', '45',
        '-c:v', 'libx264', '-preset', 'ultrafast', '-pix_fmt', 'yuv420p',
        '-g', '30', '-b:v', '2500k', '-c:a', 'aac', '-b:a', '128k',
        '-f', 'mpegts', str(out / 'fixture.ts')], check=True)
flags = shlex.split(subprocess.check_output(['pkg-config', '--cflags', '--libs',
    'gtk+-3.0', 'webkit2gtk-4.1', 'mpv', 'epoxy'], text=True))
subprocess.run(['gcc', '-Wall', '-Wextra', '-O2', str(ROOT / 'player.c'),
    '-o', str(out / 'player'), *flags], check=True)
if args.prepare_only:
    print(f'Prepared: {out}', flush=True)
    raise SystemExit(0)

def wait_path(path, process):
    for _ in range(100):
        if path.exists(): return
        if process.poll() is not None: raise RuntimeError(f'{path}: process exited')
        time.sleep(.1)
    raise RuntimeError(f'{path}: startup timeout')

children = []
logs = []
def launch(command, name, env=None):
    log = (out / name).open('w')
    logs.append(log)
    proc = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, env=env)
    children.append(proc)
    return proc

variants = [
    ('x11-mse-disable', 'x11', 'mse', 'disable'),
    ('x11-mse-shm', 'x11', 'mse', 'shm'),
    ('x11-embed-shm', 'x11', 'embed', 'shm'),
    ('wayland-mse-disable', 'wayland', 'mse', 'disable'),
    ('wayland-mse-shm', 'wayland', 'mse', 'shm'),
    ('wayland-embed-disable', 'wayland', 'embed', 'disable'),
    ('wayland-embed-shm', 'wayland', 'embed', 'shm'),
]
if args.only:
    selected = args.only.split(',')
    unknown = set(selected) - {variant[0] for variant in variants}
    if unknown:
        parser.error(f'Unknown variants: {", ".join(sorted(unknown))}')
    variants = [variant for variant in variants if variant[0] in selected]
if args.desktop:
    for _, backend, _, _ in variants:
        key = 'WAYLAND_DISPLAY' if backend == 'wayland' else 'DISPLAY'
        if not os.environ.get(key):
            parser.error(f'--desktop {backend} requires {key}; select available variants with --only')

def capture(name, proc, base):
    if not args.desktop:
        subprocess.run(['import', '-display', base['DISPLAY'], '-window', 'root',
            str(out / (name + '.png'))], env=base, check=True)
        return {'capture': name + '.png'}
    # Crop to this experiment, never save unrelated desktop windows.
    if os.environ.get('HYPRLAND_INSTANCE_SIGNATURE') and shutil.which('grim'):
        clients = json.loads(subprocess.check_output(['hyprctl', 'clients', '-j'], text=True))
        client = next((client for client in clients if client['pid'] == proc.pid), None)
        if client:
            details = {key: client[key] for key in ['at', 'size', 'xwayland', 'monitor', 'fullscreen']}
            (out / (name + '-window.json')).write_text(json.dumps(details, indent=2) + '\n')
            x, y = client['at']
            width, height = client['size']
            subprocess.run(['grim', '-g', f'{x},{y} {width}x{height}',
                str(out / (name + '.png'))], check=True)
            return {'capture': name + '.png', 'xwayland': client['xwayland']}
    return {'capture': None}

results = []
try:
    base = os.environ.copy()
    for key in ['LIBGL_ALWAYS_SOFTWARE', 'WEBKIT_DISABLE_DMABUF_RENDERER', 'WEBKIT_DMABUF_RENDERER_FORCE_SHM']:
        base.pop(key, None)
    if not args.desktop:
        base.pop('WAYLAND_DISPLAY', None)
        display_number = next(n for n in range(90, 120) if not pathlib.Path(f'/tmp/.X11-unix/X{n}').exists())
        display = f':{display_number}'
        xvfb = launch(['Xvfb', display, '-screen', '0', '1280x800x24', '-nolisten', 'tcp'], 'runner-xvfb.log')
        wait_path(pathlib.Path(f'/tmp/.X11-unix/X{display_number}'), xvfb)
        base.update(DISPLAY=display, LIBGL_ALWAYS_SOFTWARE='1', XDG_RUNTIME_DIR=str(runtime))
    # Let the server bind a free port and announce it, avoiding another local server.
    server = launch(['python3', str(ROOT / 'server.py'), str(out), '0'], 'runner-server.log')
    for _ in range(100):
        if server.poll() is not None:
            raise RuntimeError('Fixture server exited; see runner-server.log')
        lines = (out / 'runner-server.log').read_text().splitlines()
        ready = next((line for line in lines if line.startswith('SERVER_READY port=')), None)
        if ready:
            port = int(ready.split('=')[1])
            break
        time.sleep(.1)
    else:
        raise RuntimeError('Fixture server startup timeout')
    weston = None
    for name, backend, mode, policy in variants:
        if not args.desktop and backend == 'wayland' and weston is None:
            # Native Wayland client, nested compositor output captured through Xvfb.
            weston = launch(['weston', '--backend=x11', '--renderer=gl', '--socket=sparrow-runner',
                '--idle-time=0', '--width=1200', '--height=740', '--no-config'], 'runner-weston.log', base)
            wait_path(runtime / 'sparrow-runner', weston)
            time.sleep(1)
        env = base.copy()
        env['GDK_BACKEND'] = backend
        if backend == 'wayland':
            env.pop('DISPLAY', None)
            if not args.desktop: env['WAYLAND_DISPLAY'] = 'sparrow-runner'
        else:
            env.pop('WAYLAND_DISPLAY', None)
        env['WEBKIT_DISABLE_DMABUF_RENDERER' if policy == 'disable' else 'WEBKIT_DMABUF_RENDERER_FORCE_SHM'] = '1'
        url = f'http://127.0.0.1:{port}/'
        print(f'RUN {name}', flush=True)
        proc = launch([str(out / 'player'), mode, url + ('?embed' if mode == 'embed' else ''), url + 'stream.ts'], name + '.log', env)
        time.sleep(8)
        captured = capture(name, proc, base) if proc.poll() is None else {'capture': None}
        try:
            exit_code = proc.wait(timeout=30)
        except subprocess.TimeoutExpired:
            proc.kill(); proc.wait(); exit_code = -1
        log = (out / (name + '.log')).read_text()
        result = {'variant': name, 'desktop': args.desktop, **captured,
            'exit_code': exit_code, 'cleanup': 'CLEANUP complete' in log,
            'samples': sum('sample' in line or 'MPV_SAMPLE' in line for line in log.splitlines())}
        results.append(result)
        print(json.dumps(result), flush=True)
        (out / 'summary.json').write_text(json.dumps(results, indent=2) + '\n')
        time.sleep(1)
    (out / 'summary.json').write_text(json.dumps(results, indent=2) + '\n')
finally:
    for proc in reversed(children):
        if proc.poll() is None:
            proc.terminate()
            try: proc.wait(timeout=3)
            except subprocess.TimeoutExpired: proc.kill(); proc.wait()
    for log in logs: log.close()
print(f'Artifacts: {out}', flush=True)
raise SystemExit(1 if any(result['exit_code'] != 0 or not result['cleanup'] for result in results) else 0)
