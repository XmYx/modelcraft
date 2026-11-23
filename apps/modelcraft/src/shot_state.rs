//! `--screenshot-state <name>`: open one window or panel on the first frame, so a
//! `--screenshot` can picture a state the other start options cannot reach (the Animation panel,
//! the Curve Editor, the Render View, the Scripts and Plugins windows, Preferences, Keyboard
//! Shortcuts, the command palette and the Layers window). Each state runs the same action a menu
//! item or shortcut runs; nothing here changes the document. Docked panels are left out on purpose:
//! showing one changes the workspace layout, which is saved in the preferences when the app quits.

use crate::actions::Action;
use crate::anim_ui::AnimView;
use crate::app::ModelApp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShotState {
    Timeline,
    CurveEditor,
    RenderView,
    Scripts,
    Plugins,
    Preferences,
    Shortcuts,
    Palette,
    Layers,
    Statistics,
    IconPalettes,
}

pub const ALL: [ShotState; 11] = [
    ShotState::Timeline,
    ShotState::CurveEditor,
    ShotState::RenderView,
    ShotState::Scripts,
    ShotState::Plugins,
    ShotState::Preferences,
    ShotState::Shortcuts,
    ShotState::Palette,
    ShotState::Layers,
    ShotState::Statistics,
    ShotState::IconPalettes,
];

impl ShotState {
    pub fn name(self) -> &'static str {
        match self {
            ShotState::Timeline => "timeline",
            ShotState::CurveEditor => "curve-editor",
            ShotState::RenderView => "render-view",
            ShotState::Scripts => "scripts",
            ShotState::Plugins => "plugins",
            ShotState::Preferences => "preferences",
            ShotState::Shortcuts => "shortcuts",
            ShotState::Palette => "palette",
            ShotState::Layers => "layers",
            ShotState::Statistics => "statistics",
            ShotState::IconPalettes => "icon-palettes",
        }
    }

    pub fn parse(s: &str) -> Result<ShotState, String> {
        ALL.into_iter().find(|v| v.name() == s).ok_or_else(|| {
            let names: Vec<&str> = ALL.iter().map(|v| v.name()).collect();
            format!("unknown screenshot state {s:?} (one of: {})", names.join(", "))
        })
    }
}

/// Open the state's window or panel, as its menu item would.
pub fn apply(app: &mut ModelApp, ctx: &egui::Context, s: ShotState) {
    match s {
        ShotState::Timeline => {
            app.anim.open = true;
            app.anim.view = AnimView::Sheet;
        }
        ShotState::CurveEditor => {
            app.anim.open = true;
            app.anim.view = AnimView::Curves;
        }
        ShotState::RenderView => {
            if !app.render_view.open {
                app.act(ctx, Action::RenderView);
            }
        }
        ShotState::Scripts => {
            if !app.script.open {
                app.act(ctx, Action::ScriptWindow);
            }
        }
        ShotState::Plugins => {
            if !app.script.plugins.open {
                app.act(ctx, Action::PluginsWindow);
            }
        }
        ShotState::Preferences => app.act(ctx, Action::Preferences),
        ShotState::Shortcuts => app.act(ctx, Action::KeyboardShortcuts),
        ShotState::Palette => app.act(ctx, Action::Palette),
        ShotState::Layers => app.act(ctx, Action::Organize(crate::organize_ui::Op::Layers)),
        // The overlay is a per-view option, not part of the saved layout.
        ShotState::IconPalettes => app.icon_palettes = true,
        ShotState::Statistics => app.views.iter_mut().for_each(|v| v.options.stats = true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Cli;
    use egui_kittest::Harness;

    fn harness(cli: Cli) -> Harness<'static, ModelApp> {
        let ctx = egui::Context::default();
        let mut app = ModelApp::with_context(&ctx, cli);
        // A small render on the CPU, so the Render View state is quick and the same everywhere.
        app.render_view.width = 32;
        app.render_view.height = 24;
        app.render_view.samples = 1;
        app.render_view.device = crate::render_view::Device::Cpu;
        Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
    }

    #[test]
    fn every_name_parses_back_and_unknown_names_are_refused() {
        for s in ALL {
            assert_eq!(ShotState::parse(s.name()), Ok(s));
        }
        let Err(err) = ShotState::parse("node-editor") else { panic!("an unknown state must be refused") };
        assert!(err.contains("timeline") && err.contains("layers"), "{err}");
        assert!(ShotState::parse("simulation").is_err(), "panels are not states: they would change the saved layout");
        assert!(ShotState::parse("").is_err());
    }

    #[test]
    fn each_state_opens_its_window_on_the_first_frame_and_only_once() {
        for s in ALL {
            let mut h = harness(Cli { screenshot_state: Some(s), ..Cli::default() });
            // `step`, not `run`: a running render asks for repaints, which `run` would wait out.
            h.step();
            h.step();
            let app = h.state();
            assert!(app.start_state.is_none(), "{s:?} is applied once");
            let open = match s {
                ShotState::Timeline => app.anim.open && app.anim.view == AnimView::Sheet,
                ShotState::CurveEditor => app.anim.open && app.anim.view == AnimView::Curves,
                ShotState::RenderView => app.render_view.open,
                ShotState::Scripts => app.script.open,
                ShotState::Plugins => app.script.plugins.open,
                ShotState::Preferences => app.prefs_window.open,
                ShotState::Shortcuts => app.shortcuts_state.is_some(),
                ShotState::Palette => app.palette.is_some(),
                ShotState::Layers => app.organize.layers_open,
                ShotState::IconPalettes => app.icon_palettes,
                ShotState::Statistics => app.views.iter().all(|v| v.options.stats),
            };
            assert!(open, "{s:?} opens its window");
            assert!(!app.doc.dirty, "{s:?} leaves the document unchanged");
            let default = crate::dock::Layouts::default();
            assert_eq!(app.layouts.to_saved(), default.to_saved(), "{s:?} leaves the saved layout alone");
            // Let a started render end before the harness goes away.
            for _ in 0..1500 {
                if !h.state().render_view.rendering() {
                    break;
                }
                h.step();
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            h.state_mut().render_view.stop();
        }
    }

    #[test]
    fn a_picture_run_keeps_settings_in_memory_and_has_no_recovery_folder() {
        use craft_settings::paths::Location;
        let picture = crate::app::PictureRun(true);
        assert_eq!(picture.settings_location(), Location::Memory, "a picture must not read or write the user's settings");
        assert_eq!(picture.recovery_dir(), None, "a picture must not journal into or clean up the user's recovery folder");
        assert_eq!(crate::app::PictureRun(false).settings_location(), Location::Auto);
        let ctx = egui::Context::default();
        let with = ModelApp::with_context(&ctx, Cli { screenshot: Some("x.png".into()), ..Cli::default() });
        assert!(crate::app::PictureRun::of(&with).0);
        assert!(!crate::app::PictureRun::of(&ModelApp::with_context(&ctx, Cli::default())).0);
    }

    #[test]
    fn without_a_state_nothing_extra_opens() {
        let mut h = harness(Cli::default());
        h.run();
        let app = h.state();
        assert!(!app.anim.open && !app.render_view.open && !app.script.open && !app.prefs_window.open && app.palette.is_none());
    }
}
