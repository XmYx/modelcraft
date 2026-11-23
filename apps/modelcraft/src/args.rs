//! Command-line arguments.

use std::path::PathBuf;

pub const USAGE: &str = "Usage: modelcraft [OPTIONS] [FILE]

FILE                       a .modelcraft project, or a .glb/.gltf/.obj/.stl/.ply file to import

Options:
  --demo model|sculpt        open a built-in demo scene
  --workspace model|sculpt|paint
                             start in this workspace
  --quad                     start with four views
  --uv-editor none|angle|area|stretch
                             open the UV Editor with this heat map
  --screenshot <PNG>         render the window once to PNG and quit
  --screenshot-state <NAME>  open one window first: timeline, curve-editor, render-view,
                             scripts, plugins, preferences, shortcuts, palette or layers
  --bench-viewport <FRAMES>  orbit around a 1M-polygon sculpt for FRAMES frames, print frame
                             times and quit
  --bench-polys <N>          polygon count for --bench-viewport (default 1000000)
  --bench-skin <FRAMES>      play a skinned character (100,000 vertices, 60 joints) for FRAMES
                             frames and print frame, pose, evaluation and upload times as JSON
  --bench-skin-dq            with --bench-skin: dual-quaternion skinning
  --control                  let agents control this window over the loopback channel
  --control-port <PORT>      the same, on exactly this port
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
    /// Open this window on the first frame (for `--screenshot`).
    pub screenshot_state: Option<crate::shot_state::ShotState>,
    /// Open the UV Editor at start, with this heat map (`none`, `angle`, `area`, `stretch`).
    pub uv_editor: Option<crate::uv_ui::Heat>,
    pub bench_frames: Option<u32>,
    pub bench_polys: Option<u32>,
    /// Play the skinned benchmark character for this many frames and print timings.
    pub bench_skin: Option<u32>,
    /// Skin the benchmark character with dual quaternions.
    pub bench_skin_dq: bool,
    pub file: Option<PathBuf>,
    /// Open the agent channel (`Some(None)`: the default port).
    pub control: Option<Option<u16>>,
}

#[derive(Debug, PartialEq)]
pub enum Launch {
    Run(Cli),
    Print(String),
}

/// `ModelCraft <version>`, plus the commit and date `packaging/common.sh` exports for release builds.
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
            "--uv-editor" => {
                cli.uv_editor = Some(match value("--uv-editor")?.as_str() {
                    "none" => crate::uv_ui::Heat::Off,
                    "angle" => crate::uv_ui::Heat::Angle,
                    "area" => crate::uv_ui::Heat::Area,
                    "stretch" => crate::uv_ui::Heat::Stretch,
                    other => return Err(format!("unknown heat map {other:?}")),
                })
            }
            "--quad" => cli.quad = true,
            "--control" => cli.control = Some(cli.control.flatten()),
            "--control-port" => cli.control = Some(Some(value("--control-port")?.parse().map_err(|_| "--control-port takes a port number")?)),
            "--screenshot" => cli.screenshot = Some(PathBuf::from(value("--screenshot")?)),
            "--screenshot-state" => cli.screenshot_state = Some(crate::shot_state::ShotState::parse(&value("--screenshot-state")?)?),
            "--bench-viewport" => cli.bench_frames = Some(value("--bench-viewport")?.parse().map_err(|_| "--bench-viewport takes a frame count")?),
            "--bench-skin" => cli.bench_skin = Some(value("--bench-skin")?.parse().map_err(|_| "--bench-skin takes a frame count")?),
            "--bench-skin-dq" => cli.bench_skin_dq = true,
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
        let Ok(Launch::Run(c)) = parse(["--control-port", "29000"].map(String::from)) else { panic!() };
        assert_eq!(c.control, Some(Some(29000)));
        let Ok(Launch::Run(c)) = parse(["--control".to_string()]) else { panic!() };
        assert_eq!(c.control, Some(None));
        assert!(parse(["--control-port", "x"].map(String::from)).is_err());
        let Ok(Launch::Run(c)) = parse(["--screenshot-state", "curve-editor"].map(String::from)) else { panic!() };
        assert_eq!(c.screenshot_state, Some(crate::shot_state::ShotState::CurveEditor));
        assert!(parse(["--screenshot-state", "nope"].map(String::from)).is_err());
        assert!(parse(["--screenshot-state".to_string()]).is_err());
    }
}
