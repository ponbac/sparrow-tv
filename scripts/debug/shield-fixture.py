#!/usr/bin/env python3
"""Serve a loopback-only synthetic Shield Channel Catalog, guide, icons and live TV."""
import colorsys
import os
import shutil
import subprocess
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from xml.etree import ElementTree as ET

GROUPS = {
    "Observatory": ("Amber Orbit", "Lunar Drift", "Violet Zenith"),
    "Coast": ("Coral Sound", "Tidal Glass", "Beacon Bay"),
    "Studio": ("Ochre Canvas", "Silver Loom", "Indigo Ink"),
    "Gardens": ("Mint Grove", "Moss Terrace", "Fern Hollow"),
    "City": ("Neon Crossing", "Copper Quarter", "Cobalt Tram"),
    "Summits": ("Alpine Echo", "Slate Ridge", "Glacier Gate"),
}
CHANNELS = [(group, name) for group, names in GROUPS.items() for name in names]
BASE = "http://127.0.0.1:18108"
CACHE = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "sparrow/shield-fixture-v1"


def generate_media():
    CACHE.mkdir(parents=True, exist_ok=True)
    for i, (_, name) in enumerate(CHANNELS):
        color = colorsys.hsv_to_rgb((i * 0.618 + 0.08) % 1, 0.8, 0.8)
        rgb = "".join(f"{round(value * 255):02x}" for value in color)
        for ext in ("png", "mp4"):
            path = CACHE / f"{i}.{ext}"
            if path.exists():
                continue
            temp = path.with_stem(f"{i}-partial")
            scene = f"color=c=0x{rgb}:s=640x360:r=24"
            args = ["-f", "lavfi", "-i", scene]
            if ext == "png":
                initials = "".join(word[0] for word in name.split())
                args += ["-vf", f"scale=128:128,drawtext=text={initials}:fontsize=54:"
                         "fontcolor=white:x=(w-tw)/2:y=(h-th)/2", "-frames:v", "1"]
            else:
                args += ["-f", "lavfi", "-i", f"sine=frequency={220 + i * 16}",
                         "-vf", f"drawtext=text='{name}':fontcolor=white:fontsize=46:"
                         "x=(w-tw)/2+60*sin(t*PI/4):y=(h-th)/2+40*cos(t*PI/4)",
                         "-t", "8", "-c:v", "libx264", "-threads", "2", "-g", "24",
                         "-pix_fmt", "yuv420p", "-c:a", "aac"]
            subprocess.run(["ffmpeg", "-loglevel", "error", "-y", *args, temp], check=True)
            temp.replace(path)


def catalog():
    lines = ["#EXTM3U"]
    for i, (group, name) in enumerate(CHANNELS):
        lines.extend([f'#EXTINF:-1 tvg-id="c{i}" tvg-logo="{BASE}/{i}.png" '
                      f'group-title="{group}",{name}', f"{BASE}/{i}.ts"])
    return "\n".join(lines) + "\n"


def guide():
    tv = ET.Element("tv")
    now = datetime.now(timezone.utc).replace(second=0, microsecond=0)
    for i, (_, name) in enumerate(CHANNELS):
        channel = ET.SubElement(tv, "channel", id=f"c{i}")
        ET.SubElement(channel, "display-name").text = name
    for i, (_, name) in enumerate(CHANNELS):
        start = now - timedelta(minutes=30)
        stop = now + timedelta(minutes=2 + i)
        for title in ("Daybreak", "Field Notes", "Panorama", "After Hours", "Night Watch"):
            programme = ET.SubElement(tv, "programme", channel=f"c{i}",
                                      start=start.strftime("%Y%m%d%H%M%S +0000"),
                                      stop=stop.strftime("%Y%m%d%H%M%S +0000"))
            ET.SubElement(programme, "title").text = f"{name} · {title}"
            start, stop = stop, stop + timedelta(minutes=30)
    return ET.tostring(tv, encoding="utf-8")


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/catalog.m3u":
            data, mime = catalog().encode(), "audio/x-mpegurl"
        elif self.path == "/guide.xml":
            data, mime = guide(), "application/xml"
        else:
            for i in range(len(CHANNELS)):
                if self.path == f"/{i}.ts":
                    return self.stream(i)
                if self.path == f"/{i}.png":
                    data, mime = (CACHE / f"{i}.png").read_bytes(), "image/png"
                    break
            else:
                return self.send_error(404)
        self.send_response(200)
        self.send_header("Content-Type", mime)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def stream(self, i):
        self.send_response(200)
        self.send_header("Content-Type", "video/mp2t")
        self.end_headers()
        with subprocess.Popen(["ffmpeg", "-loglevel", "error", "-stream_loop", "-1", "-re",
                               "-i", CACHE / f"{i}.mp4", "-c", "copy", "-f", "mpegts", "-"],
                              stdout=subprocess.PIPE) as ffmpeg:
            try:
                shutil.copyfileobj(ffmpeg.stdout, self.wfile, 4096)
            except (BrokenPipeError, ConnectionResetError):
                pass
            finally:
                ffmpeg.terminate()

    def log_request(self, *args):
        pass


if __name__ == "__main__":
    with ThreadingHTTPServer(("127.0.0.1", 18108), Handler) as server:
        generate_media()
        print("Shield fixture ready on loopback port 18108", flush=True)
        server.serve_forever()
