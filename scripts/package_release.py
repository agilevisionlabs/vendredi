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
    print(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("platform")
    parser.add_argument("--output", type=Path, default=Path("target/dist"))
    args = parser.parse_args()
    package(args.binary, args.platform, args.output)
