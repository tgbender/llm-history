use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn sources(root: &Path, extension: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(sources(&path, extension));
        } else if path.extension().is_some_and(|e| e == extension) {
            found.push(path);
        }
    }
    found.sort();
    found
}

fn sql_assets(fts: &Path, out: &Path) -> PathBuf {
    let config = fs::read_to_string(fts.join("CMakeLists.txt")).unwrap();
    let mut header = String::from(
        "#pragma once\n#include \"fts_sql_template.hpp\"\nnamespace duckdb { namespace fts_sql {\n",
    );
    let mut source =
        String::from("#include \"fts_sql_assets.hpp\"\nnamespace duckdb { namespace fts_sql {\n");
    for (index, (name, path)) in config
        .lines()
        .filter_map(|l| l.split('"').nth(1)?.split_once('|'))
        .enumerate()
    {
        header.push_str(&format!("extern const SQLTemplateAsset {name};\n"));
        let sql = fs::read_to_string(fts.join(path))
            .unwrap()
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let delimiter = format!("fts{index}");
        assert!(!sql.contains(&format!("){delimiter}\"")));
        source.push_str(&format!("const SQLTemplateAsset {name} = {{\"{path}\",\n"));
        // Keep literals below MSVC's size limit, as in upstream's CMake generator.
        let mut chunk = String::new();
        for line in sql.split_inclusive('\n') {
            assert!(line.len() <= 16000);
            if chunk.len() + line.len() > 16000 {
                source.push_str(&format!("R\"{delimiter}({chunk}){delimiter}\"\n"));
                chunk.clear();
            }
            chunk.push_str(line);
        }
        source.push_str(&format!("R\"{delimiter}({chunk}){delimiter}\"}};\n"));
    }
    header.push_str("}}\n");
    source.push_str("}}\n");
    fs::write(out.join("fts_sql_assets.hpp"), header).unwrap();
    let path = out.join("fts_sql_assets.cpp");
    fs::write(&path, source).unwrap();
    path
}

fn main() {
    let root = Path::new("../../../vendor/duckdb-extensions");
    for path in [
        "icu",
        "fts/src",
        "fts/third_party/snowball",
        "fts/CMakeLists.txt",
    ] {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    println!("cargo:rerun-if-changed=src/extensions.cpp");
    let include = PathBuf::from(env::var_os("DEP_DUCKDB_INCLUDE").expect("DuckDB headers"));
    let duckdb = include.parent().unwrap().parent().unwrap();
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let fts = root.join("fts");
    let icu = root.join("icu");
    let mut cpp = cc::Build::new();
    cpp.cpp(true)
        .std("c++17")
        .debug(false)
        .opt_level(2)
        .warnings(false)
        .define("NDEBUG", None)
        .define("DUCKDB_STATIC_BUILD", None)
        .define("EXT_VERSION_ICU", Some("\"v2.0.0\""))
        .define("EXT_VERSION_FTS", Some("\"2266e67d01\""))
        .include(&include)
        .include(&out)
        .include(duckdb.join("third_party/utf8proc/include"))
        .include(duckdb.join("third_party/fmt/include"))
        .include(duckdb.join("third_party/zstd/include"))
        .include(fts.join("src/include"))
        .include(fts.join("third_party/snowball/include"))
        .include(icu.join("include"))
        .include(icu.join("datetime/include"))
        .include(icu.join("collation/include"))
        .files(sources(&icu, "cpp"))
        .files(sources(&fts.join("src"), "cpp"))
        .file(sql_assets(&fts, &out))
        .file("src/extensions.cpp");
    if env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|e| e == "msvc") {
        cpp.flag("/EHsc").flag("/bigobj").flag("/utf-8");
    }
    cpp.compile("llm_history_duckdb_extensions");
    let mut c = cc::Build::new();
    c.debug(false)
        .opt_level(2)
        .warnings(false)
        .include(fts.join("third_party/snowball/include"))
        .include(fts.join("third_party/snowball/libstemmer"))
        .include(fts.join("third_party/snowball/runtime"))
        .files(sources(&fts.join("third_party/snowball"), "c"));
    c.compile("llm_history_duckdb_stemmers");
}
