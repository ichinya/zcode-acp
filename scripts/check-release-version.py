"""Reject a release whose tag does not identify the checked-out Cargo version."""
import os
import pathlib
import tomllib

root = pathlib.Path(__file__).resolve().parent.parent
with (root / "Cargo.toml").open("rb") as manifest:
    version = tomllib.load(manifest)["package"]["version"]
expected = f"v{version}"
actual = os.environ.get("ZCODE_RELEASE_TAG", "")
if actual != expected:
    raise SystemExit(f"Release tag {actual!r} must match Cargo version: {expected}")
print(f"Release version verified: {expected}")
