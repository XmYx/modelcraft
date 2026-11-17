//! Command-line arguments.

use std::path::PathBuf;

pub const USAGE: &str = "Usage: modelcraft [OPTIONS] [FILE]

FILE                       a .modelcraft project, or a .glb/.gltf/.obj/.stl/.ply file to import

Options:
  --demo model|sculpt        open a built-in demo scene
  --workspace model|sculpt|paint
                             start in this workspace
  --quad                     start with four views
  --screenshot <PNG>         render the window once to PNG and quit
  --bench-viewport <FRAMES>  orbit around a 1M-polygon sculpt for FRAMES frames, print frame
                             times and quit
  --bench-polys <N>          polygon count for --bench-viewport (default 1000000)
  -h, --help                 print this help
  -V, --version              print the version
";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Demo {
    #[default]
    None,
    Model,
    Sculpt,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StartWorkspace {
    #[default]
    Model,
    Sculpt,
    Paint,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cli {
    pub demo: Demo,
    pub workspace: Option<StartWorkspace>,
    pub quad: bool,
    pub screenshot: Option<PathBuf>,
    pub bench_frames: Option<u32>,
    pub bench_polys: Option<u32>,
    pub file: Option<PathBuf>,
}

#[derive(Debug, PartialEq)]
pub enum Launch {
    Run(Cli),
    Print(String),
}

/// `ModelCraft <version>`, plus the commit and date `packaging/env.sh` exports for release builds.
pub fn version() -> String {
    let mut v = modelcraft_engine::formats::GENERATOR.to_string();
    if let Some(sha) = option_env!("MODELCRAFT_BUILD_SHA").filter(|s| !s.is_empty()) {
        v.push_str(&format!(" ({})", sha.get(..12).unwrap_or(sha)));
    }
    if let Some(date) = option_env!("MODELCRAFT_BUILD_DATE").filter(|s| !s.is_empty()) {
        v.push_str(&format!(" {date}"));
    }
    v.push('\n');
    v
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Launch, String> {
    let mut cli = Cli::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "-h" | "--help" => return Ok(Launch::Print(USAGE.to_string())),
            "-V" | "--version" => return Ok(Launch::Print(version())),
            "--demo" => {
                cli.demo = match value("--demo")?.as_str() {
                    "model" => Demo::Model,
                    "sculpt" => Demo::Sculpt,
                    other => return Err(format!("unknown demo {other:?}")),
                }
            }
            "--workspace" => {
                cli.workspace = Some(match value("--workspace")?.as_str() {
                    "model" => StartWorkspace::Model,
                    "sculpt" => StartWorkspace::Sculpt,
                    "paint" => StartWorkspace::Paint,
                    other => return Err(format!("unknown workspace {other:?}")),
                })
            }
            "--quad" => cli.quad = true,
            "--screenshot" => cli.screenshot = Some(PathBuf::from(value("--screenshot")?)),
            "--bench-viewport" => cli.bench_frames = Some(value("--bench-viewport")?.parse().map_err(|_| "--bench-viewport takes a frame count")?),
            "--bench-polys" => cli.bench_polys = Some(value("--bench-polys")?.parse().map_err(|_| "--bench-polys takes a number")?),
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            _ => {
                if cli.file.is_some() {
                    return Err("only one file can be opened".into());
                }
                cli.file = Some(PathBuf::from(a));
            }
        }
    }
    Ok(Launch::Run(cli))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_options() {
        let Ok(Launch::Run(c)) = parse(["--demo", "sculpt", "--quad", "x.modelcraft"].map(String::from)) else { panic!() };
        assert_eq!(c.demo, Demo::Sculpt);
        assert!(c.quad);
        assert_eq!(c.file, Some(PathBuf::from("x.modelcraft")));
        assert!(parse(["--nope".to_string()]).is_err());
        assert!(parse(["--demo".to_string()]).is_err());
        assert!(matches!(parse(["--help".to_string()]), Ok(Launch::Print(_))));
    }
}
