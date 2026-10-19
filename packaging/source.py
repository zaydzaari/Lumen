#!/usr/bin/env python3
"""Build GPL corresponding source from tracked files and locked Cargo sources."""
from pathlib import Path
import subprocess
import tarfile
import tempfile
import tomllib

root = Path(__file__).resolve().parents[1]
version = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
output = root / "dist" / f"lumen-{version}-source.tar.gz"
output.parent.mkdir(exist_ok=True)
tracked = subprocess.check_output(["git", "ls-files", "-z"], cwd=root).split(b"\0")
with tempfile.TemporaryDirectory(prefix="lumen-source-") as temporary:
    temporary = Path(temporary)
    dependencies = temporary / "third-party"
    config = subprocess.check_output(
        ["cargo", "vendor", "--locked", "--versioned-dirs", str(dependencies)], cwd=root, text=True
    )
    config = config.replace(str(dependencies), "third-party")
    config_file = temporary / "config.toml"
    config_file.write_text(config, encoding="utf-8", newline="\n")
    top = f"lumen-{version}-source"
    with tarfile.open(output, "w:gz") as archive:
        for item in tracked:
            if not item:
                continue
            relative = Path(item.decode("utf-8"))
            if relative.as_posix() == ".cargo/config.toml":
                raise RuntimeError("Merge the existing Cargo config before creating a source archive")
            if relative.is_absolute() or ".." in relative.parts:
                raise RuntimeError("Invalid tracked source path")
            archive.add(root / relative, arcname=f"{top}/{relative.as_posix()}", recursive=False)
        archive.add(config_file, arcname=f"{top}/.cargo/config.toml")
        archive.add(dependencies, arcname=f"{top}/third-party")
print(output)
