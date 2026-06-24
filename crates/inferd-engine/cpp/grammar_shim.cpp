// inferd grammar shim: wraps json_schema_to_grammar from llama.cpp
// to provide a C-friendly interface for calling from Rust.

#include <cstring>
#include <new>
#include <string>

// Include nlohmann::ordered_json for parsing.
#include <nlohmann/json.hpp>

// The include path for json-schema-to-grammar.h is set by the CMakeLists.txt
// target_include_directories to point to the llama.cpp common directory.
#include "json-schema-to-grammar.h"

using json = nlohmann::ordered_json;

extern "C" {
    /// Convert a JSON Schema to a GBNF grammar string.
    ///
    /// Args:
    ///   schema_json: a C string containing valid JSON representing a JSON Schema
    ///
    /// Returns:
    ///   a malloc'd C string containing GBNF, or NULL if parsing/conversion fails.
    ///   The caller MUST free the result with inferd_grammar_free().
    char* inferd_json_schema_to_grammar(const char* schema_json) {
        if (!schema_json) {
            return nullptr;
        }

        try {
            // Parse the JSON string using nlohmann::ordered_json.
            auto schema = json::parse(schema_json);

            // Convert to GBNF grammar.
            auto gbnf = json_schema_to_grammar(schema, /*force_gbnf=*/true);

            // Allocate a C string and copy the result.
            size_t len = gbnf.length() + 1;
            char* result = static_cast<char*>(std::malloc(len));
            if (!result) {
                return nullptr;
            }
            std::memcpy(result, gbnf.c_str(), len);
            return result;
        } catch (...) {
            // Parse error or conversion error — return null.
            return nullptr;
        }
    }

    /// Free a string allocated by inferd_json_schema_to_grammar.
    void inferd_grammar_free(char* s) {
        std::free(s);
    }
}
