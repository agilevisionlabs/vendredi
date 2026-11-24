"""Archive one executable, preserve its Unix mode and verify the extracted bytes."""
import argparse
import hashlib
from pathlib import Path
import tarfile
import zipfile


def package(binary, platform, output):
    output.mkdir(parents=True, exist_ok=True)
    if binary.suffix == ".exe":
        path = output / f"vendredi-{platform}.zip"
        with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.write(binary, binary.name)
        with zipfile.ZipFile(path) as archive:
            assert archive.namelist() == [binary.name]
            result = archive.read(binary.name)
    else:
        path = output / f"vendredi-{platform}.tar.gz"
        with tarfile.open(path, "w:gz") as archive:
            archive.add(binary, arcname=binary.name, recursive=False)
        with tarfile.open(path) as archive:
            assert archive.getnames() == [binary.name]
            assert archive.getmember(binary.name).mode & 0o111
            result = archive.extractfile(binary.name).read()
    assert result == binary.read_bytes()
    print(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("platform")
    parser.add_argument("--output", type=Path, default=Path("target/dist"))
    args = parser.parse_args()
    package(args.binary, args.platform, args.output)
