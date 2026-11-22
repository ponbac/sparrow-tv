#!/usr/bin/env python3
"""Benchmark the production Shield Now guide from the first visible launch transition.

Run with ANDROID_SERIAL set, an existing configured catalog, and the Now guide
as the app's start screen. Uses Python 3 + Pillow, adb, ffmpeg and ffprobe. Cold
means force-stopped process with catalog, OS and disk caches retained. Use
--warm with the Guide open to time returning to the same process from Home. Records
private screen video transiently, deletes it on exit and prints only timings.
The first visible change from the settled launcher is the video time origin.
Activity TotalTime (system launch to Activity drawn) is reported independently.
"""
import argparse
import io
import json
import os
import re
import shlex
import subprocess
import tempfile
import time
import uuid
from pathlib import Path

from PIL import Image, ImageChops, ImageStat

SIZE = (320, 180)
# Normalized regions of the 16:9 Shield Now layout. The header deliberately
# excludes the clock. The grid excludes the live picture and standby card.
TOOLBAR = (69, 3, 302, 17)
GRID = (6, 87, 315, 179)
PACKAGE = 'xyz.ponbac.sparrow'


def adb(*args):
    if not os.environ.get('ANDROID_SERIAL'):
        raise SystemExit('Set ANDROID_SERIAL to the intended Shield before benchmarking.')
    return subprocess.check_output([os.environ.get('ADB', 'adb'), *args],
                                   stderr=subprocess.PIPE)


def decode(path):
    pts = json.loads(subprocess.check_output([
        'ffprobe', '-v', 'error', '-select_streams', 'v:0', '-show_frames',
        '-show_entries', 'frame=best_effort_timestamp_time', '-of', 'json', path
    ]))
    stamps = [float(f['best_effort_timestamp_time']) for f in pts['frames']]
    pixels = subprocess.check_output([
        'ffmpeg', '-loglevel', 'error', '-i', path, '-vf', 'scale=320:180',
        '-fps_mode', 'passthrough', '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'
    ])
    stride = SIZE[0] * SIZE[1] * 3
    if len(pixels) != len(stamps) * stride:
        raise ValueError('video timestamps and decoded frames differ')
    return [(stamp, Image.frombytes('RGB', SIZE, pixels[n*stride:(n+1)*stride]))
            for n, stamp in enumerate(stamps)]


def difference(left, right, mask=None):
    return ImageStat.Stat(ImageChops.difference(left, right), mask).mean[0]


def focus_mask(image):
    pixels = image.crop(GRID).tobytes()
    return bytes(g > r + 28 and b > 180 and g > 150
                 for r, g, b in zip(pixels[::3], pixels[1::3], pixels[2::3]))


def analyze(frames, home, guide):
    # Compare edge/text positions against the actual terminal layout. A mostly
    # blank dark image would otherwise look close to a dark Sparrow screen.
    def region(image, box):
        return image.crop(box).convert('L')

    toolbar = region(guide, TOOLBAR)
    grid = region(guide, GRID)
    toolbar_ink = toolbar.point(lambda v: 255 if v > 53 else 0)
    if toolbar_ink.getbbox() is None:
        raise ValueError('final guide toolbar could not be identified')
    grid_ink = grid.point(lambda v: 255 if v > 83 else 0)
    final_focus = focus_mask(guide)
    focused_area = sum(final_focus)
    if focused_area < 100 or sum(v > 83 for v in grid.tobytes()) < 350:
        raise ValueError('final screen lacks the populated, focused Now guide')

    flags = []
    for stamp, frame in frames:
        luma = frame.convert('L')
        toolbar_ok = difference(region(frame, TOOLBAR), toolbar, toolbar_ink) < 16
        grid_ok = difference(region(frame, GRID), grid, grid_ink) < 13
        focus = focus_mask(frame)
        overlap = sum(a & b for a, b in zip(final_focus, focus))
        focus_ok = overlap / max(1, focused_area + sum(focus) - overlap) >= 0.9
        flags.append((stamp, difference(home, luma) >= 9, toolbar_ok, grid_ok, focus_ok))
    # Home may animate subtly. Origin is the first change that persists, with
    # two preceding recorded launcher frames unnecessary on static displays.
    def settled(index):
        misses = [n for n, row in enumerate(flags) if not row[index]]
        first = (misses[-1] + 1) if misses else 0
        if first == len(flags):
            raise ValueError('video ended before the guide settled')
        return flags[first][0]

    origin = settled(1)
    if origin == flags[0][0]:
        raise ValueError('recorder missed the unchanged launcher before launch')
    toolbar_at, rows_at, focus_at = [settled(i) for i in (2, 3, 4)]
    return {
        'transition_to_toolbar_ms': round(1000 * (toolbar_at - origin)),
        'transition_to_grid_ms': round(1000 * (rows_at - origin)),
        'transition_to_final_focus_ms': round(1000 * (focus_at - origin)),
        'transition_to_stable_guide_ms': round(1000 * (max(toolbar_at, rows_at, focus_at) - origin))
    }


def screenshot():
    data = adb('exec-out', 'screencap', '-p')
    return Image.open(io.BytesIO(data)).convert('RGB').resize(SIZE, Image.Resampling.BILINEAR)


def run(warm):
    original_pid = adb('shell', 'pidof', '-s', PACKAGE).strip() if warm else None
    adb('shell', 'input', 'keyevent', 'KEYCODE_HOME')
    if not warm:
        adb('shell', 'am', 'force-stop', PACKAGE)
    time.sleep(1.0)
    remote = '/data/local/tmp/sparrow-startup-' + uuid.uuid4().hex + '.mp4'
    # Screenrecord and am start share one device shell. Frame PTS and all
    # visual milestones share one recording clock, independent of ADB delay.
    cmd = ('screenrecord --size 640x360 --bit-rate 4000000 --time-limit 8 '
           + shlex.quote(remote) + ' & rec=$!; sleep 1.5; '
           + 'am start -W -n ' + PACKAGE + '/.MainActivity; wait "$rec"')
    with tempfile.TemporaryDirectory(prefix='sparrow-startup-') as tmp:
        movie = Path(tmp) / 'private.mp4'
        try:
            result = adb('shell', cmd).decode()
            adb('pull', remote, str(movie))
        finally:
            adb('shell', 'rm', '-f', remote)
        frames = decode(movie)
        if not frames:
            raise ValueError('recorder returned no frames')
        # Read terminal pixels from the recording so camera and reference use
        # the same color conversion, resolution and encoder. A separate live
        # capture checks the terminal state survived the end of recording.
        guide = frames[-1][1]
        if difference(screenshot().crop(GRID).convert('L'), guide.crop(GRID).convert('L')) > 12:
            raise ValueError('guide changed after recording ended')
        measured = analyze(frames, frames[0][1].convert('L'), guide)
        if warm and adb('shell', 'pidof', '-s', PACKAGE).strip() != original_pid:
            raise ValueError('process changed during warm resume')
        total = re.search(r'^TotalTime:\s*(\d+)', result, re.MULTILINE)
        measured['activity_drawn_ms'] = int(total.group(1)) if total else None
        return measured


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runs', type=int, default=3)
    parser.add_argument('--warm', action='store_true')
    args = parser.parse_args()
    for sample in range(args.runs):
        print(json.dumps({'run': sample + 1, 'mode': 'warm' if args.warm else 'cold',
                          **run(args.warm)}), flush=True)
