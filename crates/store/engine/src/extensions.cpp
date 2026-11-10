#include "duckdb_static_extension.h"
#include "duckdb/main/extension/extension_loader.hpp"

extern "C" void icu_duckdb_cpp_init(duckdb::ExtensionLoader &);
extern "C" void fts_duckdb_cpp_init(duckdb::ExtensionLoader &);

static int32_t describe_icu(duckdb_extension_descriptor *d) {
    d->version = 1;
    d->name = "icu";
    d->extension_version = "v2.0.0";
    d->api_version = "v2.0.0";
    d->entry_cpp = reinterpret_cast<void (*)(void)>(icu_duckdb_cpp_init);
    return 0;
}
static int32_t describe_fts(duckdb_extension_descriptor *d) {
    d->version = 1;
    d->name = "fts";
    d->extension_version = "2266e67d01";
    d->api_version = "v2.0.0";
    d->entry_cpp = reinterpret_cast<void (*)(void)>(fts_duckdb_cpp_init);
    return 0;
}
extern "C" int32_t llm_history_register_duckdb_extensions() {
    return duckdb_register_static_extension(describe_icu) || duckdb_register_static_extension(describe_fts);
}
