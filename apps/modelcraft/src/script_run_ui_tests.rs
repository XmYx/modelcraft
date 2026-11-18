//! `script.run` over the window's control socket (egui_kittest).

use std::io::{BufRead, BufReader, Write};

use egui_kittest::Harness;

use crate::app::ModelApp;
use crate::args::Cli;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

fn ask(h: &mut Harness<'static, ModelApp>, port: u16, request: &str) -> serde_json::Value {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    writeln!(s, "{request}").unwrap();
    let mut reader = BufReader::new(s.try_clone().unwrap());
    let waiter = std::thread::spawn(move || {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        line
    });
    for _ in 0..300 {
        h.run();
        if waiter.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    serde_json::from_str(&waiter.join().unwrap()).unwrap()
}

#[test]
fn an_agent_runs_a_script_in_the_window_with_the_users_selection() {
    let mut h = harness();
    h.run();
    let token = "feedfacefeedfacefeedfacefeedfacefeedface";
    h.state_mut().agents.token = Some(token.into());
    h.state_mut().agents.port = Some(0);
    crate::agents::start(h.state_mut(), &egui::Context::default());
    assert!(h.state().agents.is_on(), "{}", h.state().status);
    let port = h.state().agents.listening.as_ref().unwrap().port;
    let a = h.state_mut().doc.execute(modelcraft_engine::Command::AddGroup { name: Some("picked".into()) }).unwrap().created[0];
    let b = h.state_mut().doc.execute(modelcraft_engine::Command::AddGroup { name: Some("other".into()) }).unwrap().created[0];
    h.state_mut().selection = vec![a];
    h.run();
    let req = |source: &str| serde_json::json!({"id": 1, "method": "script.run", "params": {"source": source}, "token": token}).to_string();
    let r = ask(&mut h, port, &req("print(name_of(selection()[0])); add_primitive(\"cube\", \"Agent cube\"); set_selection([2]);"));
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["result"]["ok"], true, "{r}");
    assert_eq!(r["result"]["output"][0]["text"], "picked", "the script saw the user's selection");
    assert!(h.state().doc.scene.objects().iter().any(|o| o.name == "Agent cube"));
    assert_eq!(h.state().selection, vec![b], "the selection the script set is the window's");
    assert!(h.state().status.contains("An agent changed the scene"), "{}", h.state().status);
    // A failing script is a reply with the error, and the window is still fine.
    let r = ask(&mut h, port, &req("name_of(999);"));
    assert_eq!(r["result"]["ok"], false, "{r}");
    assert!(r["result"]["error"]["message"].as_str().is_some_and(|m| !m.is_empty()));
    crate::agents::stop(h.state_mut());
}
