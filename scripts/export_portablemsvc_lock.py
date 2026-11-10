"""Export a path-free Windows x64 toolchain pin from a PortableMSVC install."""

import argparse
import json
import re
from pathlib import Path, PureWindowsPath
from urllib.parse import urlsplit


def select(record, fields):
    return {key: record[key] for key in fields.split() if key in record}


def export(installed):
    if installed["lockfile_version"] != "1.0":
        raise ValueError("Review the new PortableMSVC lockfile format before exporting it.")
    if installed["host"] != "x64" or installed["targets"] != ["x64"]:
        raise ValueError("Select a PortableMSVC installation for x64 host and target only.")

    result = select(installed, "lockfile_version channel host targets")
    result["requested"] = select(installed["requested"], "msvc_version sdk_version")
    resolved = installed["resolved"]
    result["resolved"] = {
        "msvc": select(resolved["msvc"], "toolset_version package_version vctools_version package_id"),
        "sdk": select(resolved["sdk"], "build_number version package_id"),
    }
    result["sources"] = {
        key: select(installed["sources"][key], "url sha256 channel_declared_sha256 downloaded_sha256")
        for key in ("channel_manifest", "vs_manifest")
    }
    files = []
    names = set()
    for source in installed["files"]:
        entry = select(source, "id filename url sha256 type package_ref parent")
        name = entry["filename"]
        url = urlsplit(entry["url"])
        if not name or name in (".", "..") or name != PureWindowsPath(name).name or name.casefold() in names:
            raise ValueError(f"Invalid or duplicate package filename: {name!r}")
        if url.scheme != "https" or url.netloc != "download.visualstudio.microsoft.com" or url.query or url.fragment:
            raise ValueError(f"Review the download URL for {name!r} before exporting it.")
        if not re.fullmatch(r"[a-fA-F0-9]{64}", entry["sha256"]):
            raise ValueError(f"Expected a SHA-256 package hash for {name!r}.")
        names.add(name.casefold())
        files.append(dict(entry, downloaded_path=None, extracted_paths=[]))
    if not files:
        raise ValueError("The installed lockfile contains no packages.")
    result.update(files=files, extraction_sequence=[], removed_files=[])
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("installed_lock", type=Path)
    parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parents[1] / "portablemsvc.lock")
    args = parser.parse_args()
    if args.installed_lock.resolve() == args.output.resolve():
        parser.error("Choose an output path separate from the installed lockfile.")
    installed = json.loads(args.installed_lock.read_text(encoding="utf-8-sig"))
    public = json.dumps(export(installed), indent=2) + "\n"
    args.output.write_bytes(public.encode("utf-8"))


if __name__ == "__main__":
    main()
