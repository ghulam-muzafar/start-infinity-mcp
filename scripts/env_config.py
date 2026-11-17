"""Load only the Infinity token from a dotenv file, without shell evaluation."""
import os
import shlex


def load_token(path):
    token = os.environ.get("INFINITY_API_TOKEN", "")
    if path.exists():
        for line in path.read_text().splitlines():
            key, separator, value = line.removeprefix("export ").partition("=")
            if separator and key.strip() == "INFINITY_API_TOKEN":
                parts = shlex.split(value, comments=True)
                if len(parts) != 1:
                    raise RuntimeError("Invalid INFINITY_API_TOKEN entry in environment file")
                token = parts[0]
    if not token.strip():
        raise RuntimeError("INFINITY_API_TOKEN is missing or empty")
    return token
