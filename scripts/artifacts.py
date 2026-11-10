"""Bootstrap pinned llama.cpp runtime files; no scripts from remote sources are executed."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import tarfile
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]
CACHE = Path(os.environ.get("LLM_HISTORY_RS_CACHE") or Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "llm-history-rs")


def download(url):
    request = urllib.request.Request(url, headers={"User-Agent": "llm-history-rs/0.1"})
    with urllib.request.urlopen(request, timeout=60) as response:
        return response.read()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    global CACHE
    parser = argparse.ArgumentParser()
    parser.add_argument("--record", action="store_true")
    parser.add_argument("--fetch", action="store_true")
    parser.add_argument("--cache", type=Path, default=CACHE, help="Inference runtime cache (same default as the executable).")
    args = parser.parse_args()
    CACHE = args.cache.expanduser()
    manifest_path = ROOT / "artifacts.json"
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {"format_version": 1, "artifacts": []}
    if args.record:
        items = []
        # Record official release asset hashes, then hash extracted executable bytes too.
        asset_url = "https://github.com/ggml-org/llama.cpp/releases/download/b11869/llama-b11869-bin-macos-arm64.tar.gz"
        cached_archive = CACHE / "llama-b11869-bin-macos-arm64.tar.gz"
        archive = cached_archive.read_bytes() if cached_archive.exists() else download(asset_url)
        expected = "0a24eb567528ee3265f0fcf11d9ef5d83f9d8f5bd18be760895b853eae82508f"
        assert digest(archive) == expected
        items.append({"name": "llama-b11869-bin-macos-arm64.tar.gz", "url": asset_url, "sha256": expected, "license": "MIT", "revision": "d327bf1f0780f87b8c01b367b61853b0c291d7c4"})
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as tar:
            files = []
            for member in tar.getmembers():
                if (member.isfile() or member.issym()) and (member.name.endswith("/llama-server") or member.name.endswith(".dylib")):
                    data = tar.extractfile(member).read()
                    files.append({"name": Path(member.name).name, "sha256": digest(data), "member": member.name})
            manifest["llama_files"] = files
        manifest["artifacts"] = items
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
        print("Recorded inference binary and extension pins")
    if args.fetch:
        host={("Darwin","arm64"):"macos-arm64",("Darwin","x86_64"):"macos-x64",("Linux","x86_64"):"linux-x64",("Windows","AMD64"):"windows-x64"}.get((platform.system(),platform.machine()))
        if host is None:raise SystemExit("No native artifact pin for this platform; use a compatible endpoint")
        manifest=manifest.get("platforms",{}).get(host,manifest)
        CACHE.mkdir(parents=True, exist_ok=True)
        for artifact in manifest["artifacts"]:
            path = CACHE / artifact["name"]
            outputs = [CACHE / f["name"] for f in manifest["llama_files"]]
            if outputs and all(output.is_file() for output in outputs):
                continue
            if path.is_file():
                data = path.read_bytes()
            else:
                data = download(artifact["url"])
                assert digest(data) == artifact["sha256"], artifact["name"]
                path.write_bytes(data)
            if artifact["name"].endswith(".tar.gz"):
                with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
                    for file in manifest["llama_files"]:
                        if (CACHE / file["name"]).is_file():
                            continue
                        raw = tar.extractfile(file["member"]).read()
                        assert digest(raw) == file["sha256"]
                        output = CACHE / file["name"]
                        output.write_bytes(raw)
                        output.chmod(0o755)
            elif artifact["name"].endswith(".zip"):
                with zipfile.ZipFile(io.BytesIO(data)) as archive:
                    for file in manifest["llama_files"]:
                        if (CACHE / file["name"]).is_file():
                            continue
                        raw=archive.read(file["member"]);assert digest(raw)==file["sha256"]
                        output=CACHE/file["name"];output.write_bytes(raw);output.chmod(0o755)
        print("Native artifacts available in", CACHE)


if __name__ == "__main__":
    main()
