#!/usr/bin/env python3
import argparse
import datetime as dt
import hashlib
import json
from pathlib import Path


def write_manifest(args: argparse.Namespace) -> None:
    msi = Path(args.msi)
    if not msi.is_file():
        raise SystemExit(f"MSI not found: {msi}")

    data = msi.read_bytes()
    manifest = {
        "schema": 1,
        "channel": args.channel,
        "version": args.version,
        "build": args.build,
        "published_at": args.published_at
        or dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
        "package": {
            "url": args.url,
            "sha256": hashlib.sha256(data).hexdigest(),
            "size": len(data),
        },
        "rollout": args.rollout,
        "mandatory": False,
    }

    output = Path(args.output)
    output.write_text(
        json.dumps(manifest, separators=(",", ":"), sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(json.dumps(manifest, indent=2, ensure_ascii=False))


def main() -> None:
    parser = argparse.ArgumentParser(description="RustDesk Managed update manifest helper")
    parser.add_argument("--msi", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--build", required=True, type=int)
    parser.add_argument("--channel", required=True, choices=("stable", "test"))
    parser.add_argument("--url", required=True)
    parser.add_argument("--rollout", type=int, default=100)
    parser.add_argument("--published-at", default="")
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    if not 0 <= args.rollout <= 100:
        raise SystemExit("--rollout must be between 0 and 100")

    write_manifest(args)


if __name__ == "__main__":
    main()
