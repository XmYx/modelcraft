//! What a script may spend. A hostile or buggy script stops at the first limit it reaches.

use std::time::Duration;

/// CPU, operation and memory limits of one run.
///
/// - `max_operations` counts the interpreter's operations (every expression, call and loop turn),
///   so an endless loop stops quickly.
/// - `max_time` is wall-clock time, checked as the script runs and before every command, so a
///   script that spends its time in slow commands stops too. A single command already running
///   cannot be interrupted; commands bound their own work (see `docs/architecture.md`).
/// - Memory: strings, arrays and maps are capped (nested ones count together), every element a
///   script adds costs an operation, and the geometry a script builds or copies through its mesh
///   handles is capped by `max_native_items` over the whole run.
#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    pub max_operations: u64,
    pub max_time: Duration,
    /// Deepest nesting of script function calls.
    pub max_call_depth: usize,
    /// Deepest nesting of one expression, and of a function body.
    pub max_expr_depth: usize,
    /// Longest string, in bytes (all strings in an array or map count together).
    pub max_string_bytes: usize,
    pub max_array_len: usize,
    pub max_map_len: usize,
    /// Commands one run may execute.
    pub max_commands: u64,
    /// Vertices, faces and positions a run may build or copy through mesh handles.
    pub max_native_items: u64,
    /// Lines of output kept (print, debug, warnings).
    pub max_output_lines: usize,
    /// Bytes of one file a script may read.
    pub max_file_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_operations: 10_000_000,
            max_time: Duration::from_secs(10),
            max_call_depth: 24,
            max_expr_depth: 48,
            max_string_bytes: 256 * 1024,
            // Rhai re-measures a container on every change, so large arrays cost time; scripts
            // keep their bulk geometry in mesh handles instead.
            max_array_len: 20_000,
            max_map_len: 2_000,
            max_commands: 100_000,
            max_native_items: 4_000_000,
            max_output_lines: 5_000,
            max_file_bytes: 4 << 20,
        }
    }
}
