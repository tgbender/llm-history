//! Fixture materialization only; no parser, SQL schema or storage internals.
use anyhow::{Result, ensure};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub struct Fixture {
    pub directory: tempfile::TempDir,
    pub manifest: Value,
    pub expected: Value,
    pub inputs: PathBuf,
}
impl Fixture {
    pub fn new() -> Result<Self> {
        let inputs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/contracts");
        let manifest: Value = serde_json::from_slice(&fs::read(inputs.join("scenario.json"))?)?;
        ensure!(
            manifest["fixture_version"] == 1,
            "unsupported fixture version"
        );
        let expected = serde_json::from_slice(&fs::read(
            inputs.join(manifest["expected"].as_str().unwrap()),
        )?)?;
        let fixture = Self {
            directory: tempfile::tempdir()?,
            manifest,
            expected,
            inputs,
        };
        for p in fixture.manifest["profiles"].as_array().unwrap() {
            let path = fixture.source(p["name"].as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap())?;
            fs::copy(fixture.inputs.join(p["input"].as_str().unwrap()), path)?;
        }
        Ok(fixture)
    }
    pub fn root(&self) -> &Path {
        self.directory.path()
    }
    pub fn source(&self, name: &str) -> PathBuf {
        let p = self.profile(name);
        self.root()
            .join(p["root"].as_str().unwrap())
            .join(p["file"].as_str().unwrap())
    }
    pub fn profile(&self, name: &str) -> &Value {
        self.manifest["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap()
    }
    pub fn append(&self, profile: &str, input: &str) -> Result<()> {
        fs::OpenOptions::new()
            .append(true)
            .open(self.source(profile))?
            .write_all(&fs::read(self.inputs.join(input))?)?;
        Ok(())
    }
}
