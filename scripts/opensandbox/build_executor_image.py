#!/usr/bin/env python3
"""Operator-only Docker image builder using an already downloaded Codex package."""
import argparse
import io
import json
from pathlib import Path
import platform
import tarfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--package-dir", type=Path, required=True)
    parser.add_argument("--dockerfile", type=Path, required=True)
    parser.add_argument("--tag", default="feynman-codex-executor:0.157.1")
    parser.add_argument("--codex-version", default="0.157.1", help="Explicitly pinned version, matched to the local app-server")
    parser.add_argument("--base-image", default="python:3.12-slim", help="Operator-selected Python base, preferably pinned by digest")
    parser.add_argument("--build-network", choices=["default", "host"], default="default")
    args = parser.parse_args()
    import docker
    arch = {"aarch64": "aarch64", "arm64": "aarch64", "x86_64": "x86_64"}[platform.machine()]
    vendor = args.package_dir / "vendor" / (arch + "-unknown-linux-musl")
    package = json.loads((args.package_dir / "package.json").read_text())
    expected = args.codex_version + "-linux-" + {"aarch64": "arm64", "x86_64": "x64"}[arch]
    if package["version"] != expected or not (vendor / "bin/codex").is_file():
        raise RuntimeError("the explicitly pinned, architecture-matched Codex Linux package is required")
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode="w") as tar:
        tar.add(args.dockerfile, arcname="Dockerfile")
        for path in vendor.iterdir():
            tar.add(path, arcname="codex/" + path.name)
    archive.seek(0)
    client = docker.from_env()
    image, _ = client.images.build(fileobj=archive, custom_context=True, tag=args.tag, rm=True, pull=False,
                                   network_mode=args.build_network, buildargs={"BASE_IMAGE":args.base_image})
    print(json.dumps({"image": args.tag, "image_id": image.id, "codex_version": package["version"]}))


if __name__ == "__main__":
    main()
