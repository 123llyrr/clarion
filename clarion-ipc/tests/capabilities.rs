use clarion_ipc::{Request, Response};

#[test]
fn optional_capabilities_use_a_stable_read_only_wire_format() {
    assert_eq!(
        serde_json::to_string(&Request::Capabilities).unwrap(),
        "\"Capabilities\""
    );
    let response = Response::Capabilities(vec!["desktop-menu-focus-restore".to_owned()]);
    assert_eq!(
        serde_json::to_string(&response).unwrap(),
        "{\"Capabilities\":[\"desktop-menu-focus-restore\"]}"
    );
    let Response::Capabilities(names) = serde_json::from_str::<Response>(
        "{\"Capabilities\":[\"future-feature\",\"desktop-menu-focus-restore\"]}",
    )
    .unwrap() else {
        panic!("expected capabilities")
    };
    assert_eq!(names.len(), 2);
}
