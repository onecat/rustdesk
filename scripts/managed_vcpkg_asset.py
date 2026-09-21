#!/usr/bin/env python3
import hashlib
import os
import subprocess
import sys
import time
from pathlib import Path


def sha512_file(path: Path) -> str:
    h = hashlib.sha512()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def download(url: str, dst: Path) -> bool:
    for attempt in range(1, 4):
        try:
            dst.unlink(missing_ok=True)
        except Exception:
            pass

        print(f"asset download attempt {attempt}: {url}", flush=True)
        proc = subprocess.run(
            [
                "curl.exe",
                "--fail",
                "--location",
                "--retry",
                "2",
                "--retry-all-errors",
                "--retry-delay",
                "3",
                "--connect-timeout",
                "15",
                "--max-time",
                "900",
                "--output",
                str(dst),
                url,
            ],
            check=False,
        )
        if proc.returncode == 0 and dst.is_file():
            return True
        time.sleep(min(attempt * 5, 15))
    return False


def main() -> int:
    if len(sys.argv) != 4:
        print("usage: managed_vcpkg_asset.py <url> <sha512> <dst>", file=sys.stderr)
        return 2

    url, expected, dst_arg = sys.argv[1:]
    dst = Path(dst_arg)
    dst.parent.mkdir(parents=True, exist_ok=True)

    sources = [url]
    if url.lower().startswith("https://github.com/"):
        sources.append("https://gh.catmak.name/" + url)

    for source in sources:
        if not download(source, dst):
            continue

        actual = sha512_file(dst)
        if actual.lower() == expected.lower():
            print(f"asset verified: {dst}", flush=True)
            return 0

        print(
            f"SHA512 mismatch for {source}: expected {expected}, got {actual}",
            file=sys.stderr,
            flush=True,
        )
        try:
            dst.unlink(missing_ok=True)
        except Exception:
            pass

    print(f"failed to download verified asset: {url}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
