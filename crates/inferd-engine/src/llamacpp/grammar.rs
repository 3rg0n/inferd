//! JSON Schema to GBNF grammar compilation.
//!
//! Wraps the grammar shim (cpp/grammar_shim.cpp) which calls
//! `json_schema_to_grammar()` from llama.cpp's common library.

#![allow(unsafe_code)] // FFI surface; module-scoped.

use std::ffi::{CStr, CString};

/// Error type for grammar compilation failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrammarError {
    /// Invalid JSON Schema (parse or conversion failed).
    InvalidSchema,
    /// Failed to convert schema string to CString (contained null bytes).
    SchemaEncode,
    /// Failed to convert GBNF result from C string (invalid UTF-8).
    GbnfDecode,
}

/// Compile a JSON Schema to a GBNF grammar string.
///
/// Args:
///   schema: a serde_json Value representing a JSON Schema
///
/// Returns:
///   A String containing GBNF grammar, or a GrammarError if the
///   schema is invalid or compilation fails.
pub fn json_schema_to_gbnf(schema: &serde_json::Value) -> Result<String, GrammarError> {
    // Serialize the schema to a JSON string.
    let schema_str = serde_json::to_string(schema).map_err(|_| GrammarError::InvalidSchema)?;

    // Convert to C string.
    let schema_cstr = CString::new(schema_str).map_err(|_| GrammarError::SchemaEncode)?;

    // Call the C shim.
    let gbnf_cptr = unsafe { inferd_json_schema_to_grammar(schema_cstr.as_ptr()) };

    if gbnf_cptr.is_null() {
        return Err(GrammarError::InvalidSchema);
    }

    // Convert the C string to Rust String.
    let gbnf_str = unsafe {
        CStr::from_ptr(gbnf_cptr)
            .to_str()
            .map_err(|_| GrammarError::GbnfDecode)?
            .to_owned()
    };

    // Free the C string.
    unsafe {
        inferd_grammar_free(gbnf_cptr);
    }

    Ok(gbnf_str)
}

unsafe extern "C" {
    /// Convert a JSON Schema (as a C string containing JSON) to GBNF grammar.
    ///
    /// Returns a malloc'd C string, or NULL on error.
    /// The caller MUST free with inferd_grammar_free().
    fn inferd_json_schema_to_grammar(schema_json: *const std::ffi::c_char)
    -> *mut std::ffi::c_char;

    /// Free a string allocated by inferd_json_schema_to_grammar.
    fn inferd_grammar_free(s: *mut std::ffi::c_char);
}
