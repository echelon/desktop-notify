#!/usr/bin/env python3
"""Build the Rust service and Tauri tray app, then assemble a macOS app bundle."""
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "target/desktop/Desktop Notify.app"


def build():
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    subprocess.run([cargo, "build", "--locked", "--workspace"], cwd=ROOT, check=True)
    if sys.platform != "darwin":
        return
    executable = APP / "Contents/MacOS/desktop-notify-app"
    source = ROOT / "target/debug/desktop-notify-app"
    if executable.exists() and source.stat().st_mtime <= executable.stat().st_mtime and Path(__file__).stat().st_mtime <= executable.stat().st_mtime:
        return
    executable.parent.mkdir(parents=True, exist_ok=True)
    resources = APP / "Contents/Resources"
    resources.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, executable)
    executable.chmod(0o755)
    shutil.copy2(ROOT / "crates/desktop_notify_app/icons/icon.icns", resources / "icon.icns")
    info = {
        "CFBundleIdentifier": "io.echelon.desktop-notify.tauri", "CFBundleName": "Desktop Notify",
        "CFBundleDisplayName": "Desktop Notify", "CFBundleExecutable": "desktop-notify-app",
        "CFBundlePackageType": "APPL", "CFBundleVersion": "1", "CFBundleShortVersionString": "0.0.1",
        "CFBundleIconFile": "icon.icns", "LSMinimumSystemVersion": "11.0", "LSUIElement": True,
        "NSHighResolutionCapable": True,
    }
    (APP / "Contents/Info.plist").write_bytes(plistlib.dumps(info))
    subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", str(APP)], check=True)


if __name__ == "__main__":
    build()
