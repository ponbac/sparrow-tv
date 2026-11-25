#!/usr/bin/env python3
"""Control an isolated Android TV emulator and capture visual evidence."""
import argparse
import math
import os
import re
import subprocess
import tempfile
import time
from pathlib import Path

AVD = "sparrow-tv-api36"
PACKAGE = "xyz.ponbac.sparrow.shield"
ACTIVITY = f"{PACKAGE}/.ShieldActivity"
APK = Path(__file__).resolve().parents[2] / "app/src-shield/android/app/build/outputs/apk/release/app-release.apk"


def dump(adb):
    output = subprocess.check_output([*adb, "shell", "dumpsys", "activity", ACTIVITY,
                                      "--sparrow-guide"], text=True)
    return next((line.strip() for line in output.splitlines() if "sparrow-shield-v3 " in line), "")


def start(sdk, adb, port, gpu):
    log = Path(tempfile.gettempdir()) / f"sparrow-emulator-{port}.log"
    if subprocess.run([*adb, "get-state"], capture_output=True).returncode == 0:
        raise SystemExit(f"Emulator port {port} already in use")
    with log.open("w") as output:
        process = subprocess.Popen([sdk / "emulator/emulator", "-avd", AVD, "-port", str(port),
                                    "-gpu", gpu, "-no-window", "-read-only", "-no-snapshot",
                                    "-no-boot-anim"], stdout=output, stderr=subprocess.STDOUT,
                                   start_new_session=True)
    try:
        for _ in range(150):
            if process.poll() is not None:
                raise SystemExit(f"Emulator exited. See {log}")
            boot = subprocess.run([*adb, "shell", "getprop", "sys.boot_completed"],
                                  capture_output=True, text=True, timeout=10)
            if boot.stdout.strip() == "1":
                return
            time.sleep(2)
        raise SystemExit(f"Emulator boot timed out. See {log}")
    except BaseException:
        process.terminate()
        raise


def frames(video, directory, fps):
    if not math.isfinite(fps) or fps <= 0:
        raise SystemExit("fps must be positive")
    directory.mkdir(parents=True, exist_ok=True)
    if any(directory.iterdir()):
        raise SystemExit("Choose an empty frame directory")
    subprocess.run(["ffmpeg", "-loglevel", "error", "-i", video, "-vf", f"fps={fps}",
                    directory / "frame-%05d.png"], check=True)
    images = sorted(directory.glob("frame-*.png"))
    # Keep the sheet readable even for long recordings.
    samples = images[::max(1, math.ceil(len(images) / 24))]
    subprocess.run(["montage", *samples, "-thumbnail", "320x180", "-tile", "4x",
                    "-geometry", "+2+2", directory / "contact.png"], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", help="explicit emulator-NNNN serial")
    sub = parser.add_subparsers(dest="command", required=True)
    for command in ("create", "seed", "reverse", "dump", "stop"):
        sub.add_parser(command)
    sub.add_parser("start").add_argument("--gpu", default="host", choices=["host", "swiftshader"])
    sub.add_parser("install").add_argument("apk", nargs="?", default=APK)
    key = sub.add_parser("key")
    key.add_argument("keys", nargs="+", choices=("UP", "DOWN", "LEFT", "RIGHT", "CENTER", "BACK",
                                                "CHANNEL_UP", "CHANNEL_DOWN"))
    key.add_argument("--delay", type=float, default=0.25, help="seconds between keys")
    sub.add_parser("shot").add_argument("png", type=Path)
    record = sub.add_parser("record")
    record.add_argument("mp4", type=Path)
    record.add_argument("seconds", type=int, choices=range(1, 181), metavar="1..180")
    strip = sub.add_parser("frames")
    strip.add_argument("video", type=Path)
    strip.add_argument("directory", type=Path)
    strip.add_argument("fps", type=float)
    args = parser.parse_args()
    if args.command == "frames":
        return frames(args.video, args.directory, args.fps)
    sdk = Path(os.environ["ANDROID_HOME"])
    config_home = Path(os.environ.get("XDG_CONFIG_HOME", Path.home()))
    android_user_home = os.environ.setdefault("ANDROID_USER_HOME", str(config_home / ".android"))
    avds = Path(os.environ.setdefault("ANDROID_AVD_HOME", str(Path(android_user_home) / "avd")))
    if args.command == "create":
        if (avds / f"{AVD}.ini").is_file():
            return
        manager = sorted((sdk / "cmdline-tools").glob("*/bin/avdmanager"))[-1]
        subprocess.run([manager, "create", "avd", "-n", AVD, "-k",
                        "system-images;android-36;android-tv;x86_64", "-d", "tv_1080p"],
                       input="no\n", text=True, check=True)
        return
    match = re.fullmatch(r"emulator-([0-9]{4})", args.serial or "")
    if not match or int(match[1]) not in range(5554, 5586, 2):
        parser.error("supply --serial emulator-NNNN with an even console port from 5554 to 5584")
    adb = [sdk / "platform-tools/adb", "-s", args.serial]

    def run(*command, **kwargs):
        subprocess.run([*adb, *command], check=True, **kwargs)

    if args.command == "start":
        start(sdk, adb, int(match[1]), args.gpu)
    elif args.command == "install":
        run("install", "-r", args.apk)
    elif args.command == "reverse":
        run("reverse", "tcp:18108", "tcp:18108")
    elif args.command == "seed":
        run("shell", "am", "start", "-S", "-W", "-n", ACTIVITY)
        run("shell", "am", "broadcast", "-n", f"{PACKAGE}/.FixtureReceiver", "-a",
            f"{PACKAGE}.SEED_FIXTURE")
        for _ in range(60):
            state = dump(adb)
            if "seed=Complete" in state:
                print(state)
                return
            if "seed=Failed" in state:
                break
            time.sleep(1)
        raise SystemExit(f"Seed did not complete. Last snapshot: {state}")
    elif args.command == "dump":
        print(dump(adb) or "No Guide snapshot available")
    elif args.command == "key":
        for i, key in enumerate(args.keys):
            if i:
                time.sleep(args.delay)
            run("shell", "input", "keyevent", key if key.startswith("CHANNEL_") or key == "BACK"
                else f"DPAD_{key}")
    elif args.command == "shot":
        with args.png.open("wb") as output:
            run("exec-out", "screencap", "-p", stdout=output)
    elif args.command == "record":
        remote = f"/data/local/tmp/sparrow-{os.getpid()}.mp4"
        try:
            run("shell", "screenrecord", "--time-limit", str(args.seconds), remote)
            run("pull", remote, args.mp4)
        finally:
            run("shell", "rm", "-f", remote)
    elif args.command == "stop":
        run("emu", "kill")


if __name__ == "__main__":
    main()
