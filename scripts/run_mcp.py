#!/usr/bin/env python3
"""Launch the Rust server with the project .env token, independent of client cwd."""
import os
import argparse
from pathlib import Path
import shutil
import sys

from env_config import load_token


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--docker", nargs="?", const="start-infinity-mcp:local", metavar="IMAGE",
                        help="Run the Docker image instead of the native executable")
    args = parser.parse_args()
    project = Path(__file__).resolve().parent.parent
    token = load_token(project / ".env")
    env = {**os.environ, "INFINITY_API_TOKEN": token}
    if args.docker:
        docker = shutil.which("docker")
        if docker is None:
            raise RuntimeError("Docker executable is missing")
        os.execve(docker, [docker, "run", "--rm", "-i", "--read-only", "--cap-drop=ALL",
                          "--security-opt=no-new-privileges", "--env", "INFINITY_API_TOKEN",
                          args.docker], env)
    binary = project / "target" / "release" / "start-infinity-mcp"
    if not binary.is_file():
        raise RuntimeError("Release executable is missing; run cargo build --release --locked")
    os.execve(str(binary), [str(binary)], env)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ValueError):
        # Do not include exception contents: parser errors can contain file values.
        print("Infinity MCP startup failed: check the executable/image, Docker access and .env token", file=sys.stderr)
        raise SystemExit(1)
