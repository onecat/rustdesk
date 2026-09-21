#!/usr/bin/env python3
import argparse
import base64
import datetime as dt
import hashlib
import json
import os
from pathlib import Path

from nacl.signing import SigningKey


def load_seed() -> bytes:
    value = os.environ.get("RUSTDESK_UPDATE_SIGNING_SEED", "").strip()
    if not value:
        raise SystemExit("RUSTDESK_UPDATE_SIGNING_SEED is required")
    try:
        seed = base64.b64decode(value, validate=True)
    except Exception as exc:
        raise SystemExit(f"RUSTDESK_UPDATE_SIGNING_SEED must be valid base64: {exc}")
    if len(seed) != 32:
        raise SystemExit("RUSTDESK_UPDATE_SIGNING_SEED must decode to exactly 32 bytes")
    return seed


def public_key() -> None:
    signing_key = SigningKey(load_seed())
    print(base64.b64encode(bytes(signing_key.verify_key)).decode("ascii"))


def sign_manifest(args: argparse.Namespace) -> None:
    msi = Path(args.msi)
    if not msi.is_file():
        raise SystemExit(f"MSI not found: {msi}")

    data = msi.read_bytes()
    sha256 = hashlib.sha256(data).hexdigest()
    size = len(data)

    payload = {
        "schema": 1,
        "channel": args.channel,
        "version": args.version,
        "build": args.build,
        "published_at": args.published_at
        or dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
        "package": {
            "url": args.url,
            "sha256": sha256,
            "size": size,
        },
        "rollout": args.rollout,
        "mandatory": args.mandatory,
    }

    payload_bytes = json.dumps(
        payload, separators=(",", ":"), sort_keys=True, ensure_ascii=False
    ).encode("utf-8")

    signed = SigningKey(load_seed()).sign(payload_bytes)
    wrapper = {
        "signed": base64.b64encode(bytes(signed)).decode("ascii"),
    }

    output = Path(args.output)
    output.write_text(
        json.dumps(wrapper, separators=(",", ":"), sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(json.dumps(payload, indent=2, ensure_ascii=False))
    print(f"manifest={output}")
    print(f"sha256={sha256}")
    print(f"size={size}")


def main() -> None:
    parser = argparse.ArgumentParser(description="RustDesk Managed update signing helper")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("public-key")

    sign = sub.add_parser("sign-manifest")
    sign.add_argument("--msi", required=True)
    sign.add_argument("--version", required=True)
    sign.add_argument("--build", required=True, type=int)
    sign.add_argument("--channel", required=True, choices=("stable", "test"))
    sign.add_argument("--url", required=True)
    sign.add_argument("--rollout", type=int, default=100)
    sign.add_argument("--mandatory", action="store_true")
    sign.add_argument("--published-at", default="")
    sign.add_argument("--output", required=True)

    args = parser.parse_args()
    if args.command == "public-key":
        public_key()
    elif args.command == "sign-manifest":
        if not 0 <= args.rollout <= 100:
            raise SystemExit("--rollout must be between 0 and 100")
        sign_manifest(args)


if __name__ == "__main__":
    main()
