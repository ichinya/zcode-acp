"""Package a previously built binary; no downloads or compilation."""
import hashlib
import os
import pathlib
import sys
import zipfile

root = pathlib.Path(__file__).resolve().parent.parent
target = os.environ["ZCODE_BUILD_TARGET"]
binary = "zcode-acp.exe" if sys.platform == "win32" else "zcode-acp"
source = root / "target" / target / "release" / binary
stage = root / "artifacts"
stage.mkdir(exist_ok=True)
archive = stage / f"zcode-acp-{target}.zip"
with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as z:
    z.write(source, binary)
    z.write(root / "LICENSE", "LICENSE")
    z.write(root / "NOTICE", "NOTICE")
    z.write(root / "LICENSES" / "bb-zcode-MIT.txt", "LICENSES/bb-zcode-MIT.txt")
digest = hashlib.sha256(archive.read_bytes()).hexdigest()
archive.with_suffix(".zip.sha256").write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
print(archive.name)
