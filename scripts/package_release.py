"""Package a native release and check the contents of the archive."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request
import zipfile


def package_macos(binary, path):
    repo = Path(__file__).resolve().parents[1]
    metadata = subprocess.check_output(
        ["cargo", "metadata", "--format-version=1", "--no-deps", "--locked"], cwd=repo
    )
    version = next(p["version"] for p in json.loads(metadata)["packages"] if p["name"] == "vendredi")
    with tempfile.TemporaryDirectory() as tmp:
        app = Path(tmp) / "Vendredi.app"
        contents = app / "Contents"
        (contents / "MacOS").mkdir(parents=True)
        (contents / "Resources").mkdir()
        executable = contents / "MacOS/vendredi"
        shutil.copy2(binary, executable)
        shutil.copyfile(repo / "packaging/macos/Vendredi.icns", contents / "Resources/Vendredi.icns")
        (contents / "Info.plist").write_bytes(plistlib.dumps({
            "CFBundleDevelopmentRegion": "en",
            "CFBundleExecutable": executable.name,
            "CFBundleIconFile": "Vendredi.icns",
            "CFBundleIdentifier": "com.agilevision.vendredi",
            "CFBundleInfoDictionaryVersion": "6.0",
            "CFBundleName": "Vendredi",
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": version,
            "CFBundleVersion": version,
            "NSHighResolutionCapable": True,
        }))
        # A Developer ID identity produces the public release: signed, notarized and stapled.
        # Without one the bundle is ad-hoc signed, with the same hardened runtime, for testing.
        identity = os.environ.get("MACOS_SIGNING_IDENTITY")
        signer = ["--sign", identity, "--timestamp"] if identity else ["--sign", "-", "--timestamp=none"]
        subprocess.run(["codesign", "--force", "--options", "runtime", *signer, app], check=True)
        archive = ["ditto", "-c", "-k", "--keepParent", "--norsrc", "--noextattr", app, path]
        subprocess.run(archive, check=True)
        if identity:
            # CI passes the App Store Connect key itself; a developer's Mac names a
            # profile saved with `xcrun notarytool store-credentials`.
            if profile := os.environ.get("MACOS_NOTARY_PROFILE"):
                credentials = ["--keychain-profile", profile]
            else:
                credentials = [
                    "--key", os.environ["MACOS_NOTARY_KEY"],
                    "--key-id", os.environ["MACOS_NOTARY_KEY_ID"],
                    "--issuer", os.environ["MACOS_NOTARY_ISSUER"],
                ]
            result = json.loads(subprocess.check_output([
                "xcrun", "notarytool", "submit", path, "--wait", "--output-format", "json", *credentials,
            ]))
            assert result["status"] == "Accepted", result
            subprocess.run(["xcrun", "stapler", "staple", app], check=True)
            path.unlink()
            subprocess.run(archive, check=True)
        # Check the app a user gets by extracting the finished archive.
        extracted = Path(tmp) / "extracted"
        subprocess.run(["ditto", "-x", "-k", path, extracted], check=True)
        shipped = extracted / app.name
        subprocess.run(["codesign", "--verify", "--strict", shipped], check=True)
        assert (shipped / executable.relative_to(app)).read_bytes() == executable.read_bytes()
        if identity:
            subprocess.run(["xcrun", "stapler", "validate", shipped], check=True)
            subprocess.run(["spctl", "--assess", "--type", "execute", shipped], check=True)


# The runtime is the small executable at the front of an AppImage that mounts or
# extracts the application image appended to it.
APPIMAGE_RUNTIME = "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-"
APPIMAGE_RUNTIME_SHA256 = {
    "x86_64": "2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d",
    "aarch64": "00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444",
}


def package_appimage(binary, platform, path):
    repo = Path(__file__).resolve().parents[1]
    arch = {"linux-x64": "x86_64", "linux-arm64": "aarch64"}[platform]
    with urllib.request.urlopen(APPIMAGE_RUNTIME + arch) as response:
        runtime = response.read()
    assert hashlib.sha256(runtime).hexdigest() == APPIMAGE_RUNTIME_SHA256[arch]
    with tempfile.TemporaryDirectory() as tmp:
        appdir = Path(tmp) / "Vendredi.AppDir"
        (appdir / "usr/bin").mkdir(parents=True)
        shutil.copy2(binary, appdir / "usr/bin/vendredi")
        # Starting the executable under its own name gives the window the class "vendredi",
        # which desktops use to match it to the launcher entry and icon.
        (appdir / "AppRun").write_text('#!/bin/sh\nexec "$(dirname "$(readlink -f "$0")")/usr/bin/vendredi" "$@"\n')
        (appdir / "AppRun").chmod(0o755)
        shutil.copyfile(repo / "packaging/linux/vendredi.png", appdir / "vendredi.png")
        (appdir / ".DirIcon").symlink_to("vendredi.png")
        (appdir / "vendredi.desktop").write_text(
            "[Desktop Entry]\nType=Application\nName=Vendredi\n"
            "Comment=EDIFACT trading partner simulator\nExec=vendredi\nIcon=vendredi\n"
            "Categories=Development;\nTerminal=false\nStartupWMClass=vendredi\n"
        )
        image = Path(tmp) / "image.squashfs"
        subprocess.run(
            ["mksquashfs", appdir, image, "-root-owned", "-noappend", "-comp", "zstd", "-quiet", "-no-progress"],
            check=True,
        )
        path.write_bytes(runtime + image.read_bytes())
        path.chmod(0o755)
        # Run the finished AppImage. Extraction works on hosts without FUSE, such as CI runners.
        workspace = Path(tmp) / "workspace"
        subprocess.run([path, "init", workspace], check=True, env={**os.environ, "APPIMAGE_EXTRACT_AND_RUN": "1"})
        assert (workspace / "vendredi.toml").is_file()
    print(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}")


def package(binary, platform, output):
    output.mkdir(parents=True, exist_ok=True)
    if platform.startswith("macos-"):
        path = output / f"vendredi-{platform}.zip"
        path.unlink(missing_ok=True)
        package_macos(binary, path)
    elif binary.suffix == ".exe":
        path = output / f"vendredi-{platform}.zip"
        with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.write(binary, binary.name)
        with zipfile.ZipFile(path) as archive:
            assert archive.namelist() == [binary.name]
            assert archive.read(binary.name) == binary.read_bytes()
    else:
        path = output / f"vendredi-{platform}.tar.gz"
        with tarfile.open(path, "w:gz") as archive:
            archive.add(binary, arcname=binary.name, recursive=False)
        with tarfile.open(path) as archive:
            assert archive.getnames() == [binary.name]
            assert archive.getmember(binary.name).mode & 0o111
            assert archive.extractfile(binary.name).read() == binary.read_bytes()
        package_appimage(binary, platform, output / f"vendredi-{platform}.AppImage")
    print(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("platform")
    parser.add_argument("--output", type=Path, default=Path("target/dist"))
    args = parser.parse_args()
    package(args.binary, args.platform, args.output)
