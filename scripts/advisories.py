"""Check locked registry crates against OSV advisories. Network access is explicit.

Cargo already enforces lock checksums and the publish-age gate in .cargo/config.toml.
"""
import datetime as dt
import json
import pathlib
import tomllib
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]


def fetch(url):
    req = urllib.request.Request(url, headers={"User-Agent": "llm-history-rs-policy/0.1"})
    with urllib.request.urlopen(req, timeout=30) as response:
        return response.read()


def main():
    packages = [p for p in tomllib.loads((ROOT / "Cargo.lock").read_text())["package"] if "source" in p]
    if any(not p["source"].startswith("registry+") for p in packages):
        raise ValueError("non-registry dependencies require explicit policy review")
    payload = {"queries": [{"package": {"ecosystem": "crates.io", "name": p["name"]}, "version": p["version"]} for p in packages]}
    req = urllib.request.Request("https://api.osv.dev/v1/querybatch", data=json.dumps(payload).encode(), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as response:
        advisories = json.load(response)
    policy=json.loads((ROOT/"advisory-policy.json").read_text())["reviewed"]
    findings=[]
    for package,result in zip(packages,advisories["results"]):
        for vuln in result.get("vulns",[]):
            details=json.loads(fetch("https://api.osv.dev/v1/vulns/"+vuln["id"]))
            categories={affected.get("database_specific",{}).get("informational") for affected in details.get("affected",[])}
            reviewed=any(p["id"]==vuln["id"] and p["package"]==package["name"] and p["version"]==package["version"] and p["category"] in categories for p in policy)
            findings.append({"package":package["name"],"version":package["version"],"id":vuln["id"],"summary":details.get("summary"),"reviewed":reviewed})
    receipt={"checked_at":dt.datetime.now(dt.UTC).isoformat(),"packages":len(packages),"findings":findings}
    (ROOT/"target/advisories.json").write_text(json.dumps(receipt,indent=2)+"\n")
    if any(not f["reviewed"] for f in findings):raise ValueError("unreviewed advisories: "+json.dumps(findings))
    print("Advisory review:",json.dumps(findings))


if __name__ == "__main__":
    main()
