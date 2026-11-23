//! `cargo xtask parity`: scores `docs/target-app-parity.md`.
//!
//! The map is a list of areas (`## ` headings), each with a `Weight: <n>` line and one or more
//! tables of `| Feature | Refs | Status | Size | Notes |` rows.
//!
//! - Status: `D` done, `X` exceeds (both count 1), `P` partial (½), `M` missing (0), `O` out of
//!   scope (left out of every denominator; the note must give the reason).
//! - Size is the effort to build the feature: `S` = 1, `M` = 2, `L` = 4, `XL` = 8.
//!
//! Three numbers come out:
//! - **row parity**: (D + X + ½·P) / in-scope rows, every row counting the same;
//! - **area effort parity**: the same sum with each row weighted by its size, inside one area;
//! - **weighted parity**: the area effort parities averaged with the area weights.

use std::fmt::Write as _;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Area {
    pub name: String,
    pub weight: f64,
    /// Rows by status: done, exceeds, partial, missing, out of scope.
    pub d: u32,
    pub x: u32,
    pub p: u32,
    pub m: u32,
    pub o: u32,
    /// Size-weighted credit and in-scope size total.
    pub credit: f64,
    pub size: f64,
}

impl Area {
    pub fn rows(&self) -> u32 {
        self.d + self.x + self.p + self.m
    }
    pub fn row_score(&self) -> f64 {
        (f64::from(self.d + self.x) + 0.5 * f64::from(self.p)) / f64::from(self.rows().max(1)) * 100.0
    }
    pub fn effort_score(&self) -> f64 {
        if self.size > 0.0 { self.credit / self.size * 100.0 } else { 0.0 }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub areas: Vec<Area>,
}

impl Report {
    fn total(&self) -> Area {
        let mut t = Area::default();
        for a in &self.areas {
            t.d += a.d;
            t.x += a.x;
            t.p += a.p;
            t.m += a.m;
            t.o += a.o;
            t.credit += a.credit;
            t.size += a.size;
        }
        t
    }

    /// Area effort parities averaged with the area weights.
    pub fn weighted(&self) -> f64 {
        let w: f64 = self.areas.iter().map(|a| a.weight).sum();
        if w <= 0.0 {
            return 0.0;
        }
        self.areas.iter().map(|a| a.weight * a.effort_score()).sum::<f64>() / w
    }

    #[cfg(test)]
    pub fn row(&self) -> f64 {
        self.total().row_score()
    }

    /// The area that measures UI parity with the reference layout, when the map has one (its
    /// heading starts with "UI parity").
    pub fn ui(&self) -> Option<&Area> {
        self.areas.iter().find(|a| a.name.starts_with("UI parity"))
    }

    /// The Markdown block written between the `parity:` markers.
    pub fn block(&self) -> String {
        let t = self.total();
        let wsum: f64 = self.areas.iter().map(|a| a.weight).sum();
        let mut s = String::from("| | Value | Kind |\n|---|---|---|\n");
        let _ = writeln!(s, "| In-scope feature rows | {} | counted |", t.rows());
        let _ = writeln!(s, "| Done / exceeds / partial / missing | {} / {} / {} / {} | counted |", t.d, t.x, t.p, t.m);
        let _ = writeln!(s, "| Out of scope (not in any denominator) | {} | counted |", t.o);
        let _ = writeln!(s, "| **Row parity**, (D + X + ½·P) / rows | **{:.1} %** | counted |", t.row_score());
        let _ = writeln!(s, "| **Weighted parity**, area weights × size-weighted rows | **{:.1} %** | counted |", self.weighted());
        let _ = writeln!(s, "| Size-weighted parity over all rows, no area weights | {:.1} % | counted |", t.effort_score());
        if let Some(ui) = self.ui() {
            let _ = writeln!(s, "| **UI parity**, rows of the reference-layout area | **{:.1} %** | counted |", ui.row_score());
        }
        s.push('\n');
        s.push_str("| Area | Weight | Rows | D | X | P | M | Out | Row parity | Effort parity |\n");
        s.push_str("|---|---|---|---|---|---|---|---|---|---|\n");
        for a in &self.areas {
            let share = if wsum > 0.0 { a.weight / wsum * 100.0 } else { 0.0 };
            let _ = writeln!(
                s,
                "| {} | {} ({:.0} %) | {} | {} | {} | {} | {} | {} | {:.0} % | {:.0} % |",
                a.name,
                a.weight,
                share,
                a.rows(),
                a.d,
                a.x,
                a.p,
                a.m,
                a.o,
                a.row_score(),
                a.effort_score()
            );
        }
        s
    }
}

fn size_of(s: &str) -> Option<f64> {
    match s {
        "S" => Some(1.0),
        "M" => Some(2.0),
        "L" => Some(4.0),
        "XL" => Some(8.0),
        _ => None,
    }
}

/// Parses the map. Errors name the line, so a malformed row never scores silently.
pub fn score(text: &str) -> Result<Report, String> {
    let mut report = Report::default();
    let mut current: Option<Area> = None;
    let mut in_block = false;
    for (n, line) in text.lines().enumerate() {
        let n = n + 1;
        // The generated block is output, not input.
        if line.starts_with("<!-- parity:start") {
            in_block = true;
            continue;
        }
        if line.starts_with("<!-- parity:end") {
            in_block = false;
            continue;
        }
        if in_block {
            continue;
        }
        if let Some(h) = line.strip_prefix("## ") {
            if let Some(a) = current.take() {
                finish(&mut report, a)?;
            }
            current = Some(Area { name: h.trim().to_string(), ..Area::default() });
            continue;
        }
        if let Some(w) = line.strip_prefix("Weight:") {
            let w = w.trim().trim_matches('*').trim();
            let w: f64 = w.parse().map_err(|_| format!("line {n}: weight {w:?} is not a number"))?;
            if !(w > 0.0 && w <= 100.0) {
                return Err(format!("line {n}: weight {w} must be in (0, 100]"));
            }
            match current.as_mut() {
                Some(a) => a.weight = w,
                None => return Err(format!("line {n}: weight outside an area")),
            }
            continue;
        }
        if !line.starts_with('|') {
            continue;
        }
        let cols: Vec<&str> = line.split('|').map(str::trim).collect();
        if cols.len() < 7 {
            continue;
        }
        let credit = match cols[3] {
            "D" | "X" => 1.0,
            "P" => 0.5,
            "M" => 0.0,
            "O" => -1.0,
            _ => continue,
        };
        let Some(a) = current.as_mut() else { return Err(format!("line {n}: feature row outside an area")) };
        let size = size_of(cols[4]).ok_or_else(|| format!("line {n}: size {:?} must be S, M, L or XL", cols[4]))?;
        match cols[3] {
            "D" => a.d += 1,
            "X" => a.x += 1,
            "P" => a.p += 1,
            "M" => a.m += 1,
            _ => {
                if cols[5].is_empty() {
                    return Err(format!("line {n}: an out-of-scope row needs its reason in the notes"));
                }
                a.o += 1;
                continue;
            }
        }
        a.credit += credit * size;
        a.size += size;
    }
    if let Some(a) = current.take() {
        finish(&mut report, a)?;
    }
    if report.areas.is_empty() {
        return Err("no feature rows found".into());
    }
    Ok(report)
}

fn finish(report: &mut Report, a: Area) -> Result<(), String> {
    if a.rows() + a.o == 0 {
        return Ok(());
    }
    if a.weight <= 0.0 {
        return Err(format!("area {:?} has rows but no `Weight:` line", a.name));
    }
    report.areas.push(a);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::score;

    const MAP: &str = "# Map\n\n<!-- parity:start -->\n| A | 1 | D | S | x |\n<!-- parity:end -->\n\n## Intro\n\nText.\n\n## One\n\nWeight: 3\n\n| Feature | Refs | Status | Size | Notes |\n|---|---|---|---|---|\n| a | C | D | S | |\n| b | C | P | L | |\n| c | C | M | M | |\n| d | C | O | XL | vendor cloud |\n\n## Two\n\nWeight: **1**\n\n| Feature | Refs | Status | Size | Notes |\n|---|---|---|---|---|\n| e | M | X | M | |\n";

    #[test]
    fn scores_rows_sizes_and_weights() {
        let r = score(MAP).unwrap();
        assert_eq!(r.areas.len(), 2, "areas without rows are skipped");
        let one = &r.areas[0];
        assert_eq!((one.d, one.p, one.m, one.o), (1, 1, 1, 1));
        assert!((one.row_score() - 50.0).abs() < 1e-9);
        // (1·1 + 0.5·4 + 0·2) / 7
        assert!((one.effort_score() - 300.0 / 7.0).abs() < 1e-9);
        assert!((r.areas[1].effort_score() - 100.0).abs() < 1e-9);
        let w = (3.0 * 300.0 / 7.0 + 100.0) / 4.0;
        assert!((r.weighted() - w).abs() < 1e-9);
        // Four in-scope rows: D, P, M, X.
        assert!((r.row() - 62.5).abs() < 1e-9);
    }

    #[test]
    fn rejects_malformed_rows() {
        assert!(score("## A\n\n| a | C | D | S | |\n").is_err(), "no weight");
        assert!(score("## A\nWeight: 1\n| a | C | D | huge | |\n").is_err(), "bad size");
        assert!(score("## A\nWeight: 1\n| a | C | O | S | |\n").is_err(), "out of scope without a reason");
        assert!(score("## A\nWeight: x\n").is_err(), "bad weight");
        assert!(score("| a | C | D | S | |\n").is_err(), "row outside an area");
        assert!(score("nothing").is_err());
    }

    #[test]
    fn ui_parity_is_the_row_score_of_the_ui_area() {
        assert!(score(MAP).unwrap().ui().is_none());
        assert!(!score(MAP).unwrap().block().contains("UI parity"), "no UI line without the area");
        let map = format!(
            "{MAP}\n## UI parity with the reference layout\n\nWeight: 2\n\n| Feature | Refs | Status | Size | Notes |\n|---|---|---|---|---|\n| f | C | D | S | |\n| g | C | P | M | |\n| h | C | M | L | |\n| i | C | O | S | vendor service |\n"
        );
        let r = score(&map).unwrap();
        let ui = r.ui().unwrap();
        assert_eq!(ui.rows(), 3, "the out-of-scope row is left out");
        // (1 + 0.5 + 0) / 3
        assert!((ui.row_score() - 50.0).abs() < 1e-9);
        assert!(r.block().contains("| **UI parity**, rows of the reference-layout area | **50.0 %** | counted |"), "{}", r.block());
    }

    #[test]
    fn the_map_in_the_repository_has_a_ui_area() {
        let r = score(include_str!("../../docs/target-app-parity.md")).unwrap();
        let ui = r.ui().unwrap();
        assert!(ui.rows() >= 46, "{} UI rows", ui.rows());
        assert!(r.block().contains("**UI parity**"));
    }

    #[test]
    fn block_is_stable() {
        let r = score(MAP).unwrap();
        let b = r.block();
        assert!(b.contains("| One | 3 (75 %) | 3 | 1 | 0 | 1 | 1 | 1 | 50 % | 43 % |"), "{b}");
        assert_eq!(b, score(MAP).unwrap().block());
    }
}
