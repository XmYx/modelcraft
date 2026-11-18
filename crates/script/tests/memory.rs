//! How much memory a hostile script can make the process use under the default limits.
//!
//! There is no allocator cap (see `docs/scripting.md`, "Memory"), so this is a measurement with a
//! regression guard, not a guarantee: the process's peak resident size (`VmHWM`, Linux) before
//! and after each hostile script. This file holds one test on purpose: the peak is per process, so
//! other tests running beside it would add to it. `cargo test -p modelcraft-script --test memory
//! -- --nocapture` prints the numbers recorded in `docs/performance.md`. The peak never goes down,
//! so a script that needs less than an earlier one in the list shows +0.

use modelcraft_engine::Document;
use modelcraft_script::{ErrorKind, Limits, Permissions, RunContext, Session};

/// The growth of the peak resident set a run may cause, in MiB. Measured values are a third of this.
const BUDGET_MIB: u64 = 512;

fn peak_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|l| l.strip_prefix("VmHWM:")).and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
}

const SCRIPTS: &[(&str, &str)] = &[
    ("map keys with integer values", "let m = #{}; let i = 0; loop { m[\"key\" + i] = i; i += 1; }"),
    ("map keys with an array each", "let m = #{}; let i = 0; loop { m[\"key\" + i] = [1, 2, 3, 4, 5, 6, 7, 8]; i += 1; }"),
    ("map keys with a 1 KiB string each", "let s = \"x\"; for i in 0..10 { s += s; } let m = #{}; let i = 0; loop { m[\"key\" + i] = s; i += 1; }"),
    ("array push of arrays", "let row = [1, 2, 3, 4, 5, 6, 7, 8]; let a = []; loop { a.push(row); }"),
    ("string doubling", "let s = \"x\"; loop { s += s; }"),
    ("nested containers", "let a = []; for i in 0..1000 { let b = []; for j in 0..1000 { b.push(j); } a.push(b); }"),
    ("mesh built from a huge range", "let b = mesh_builder(); loop { b.vertex(1.0, 2.0, 3.0); }"),
];

#[test]
fn hostile_scripts_stay_inside_a_measured_memory_bound() {
    let Some(start) = peak_kib() else {
        eprintln!("memory test skipped: no /proc/self/status on this platform");
        return;
    };
    let mut worst = 0u64;
    let mut before = start;
    for (what, src) in SCRIPTS {
        let mut s = Session::new(Limits::default(), Permissions::none());
        let mut d = Document::new();
        let r = s.run(&mut d, &mut RunContext::default(), "hostile.rhai", src);
        drop(s);
        let after = peak_kib().unwrap_or(before);
        let grew = after.saturating_sub(before) / 1024;
        worst = worst.max(grew);
        eprintln!("{what}: peak +{grew} MiB in {:.2} s, ended with {:?}", r.elapsed.as_secs_f64(), r.error.as_ref().map(|e| e.kind));
        assert!(r.error.as_ref().is_some_and(|e| e.kind == ErrorKind::Limit) || r.ok(), "{what}: {:?}", r.error);
        before = after;
    }
    let total = peak_kib().unwrap_or(start).saturating_sub(start) / 1024;
    eprintln!("worst single script: +{worst} MiB, all together: +{total} MiB (budget {BUDGET_MIB} MiB each)");
    assert!(worst < BUDGET_MIB, "a script grew the peak by {worst} MiB");
    assert!(total < BUDGET_MIB * 2, "all scripts together grew the peak by {total} MiB");
}
