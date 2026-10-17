use clarion_config::Config;
use smithay::reexports::wayland_protocols::wp::content_type::v1::client::wp_content_type_v1::{
    Type, WpContentTypeV1,
};
use wayland_client::protocol::wl_surface::WlSurface;

use super::client::ClientId;
use super::Fixture;
use crate::layout::LayoutElement as _;
use crate::window::ResolvedWindowRules;

fn setup(config: &str) -> (Fixture, ClientId, WlSurface, WpContentTypeV1) {
    let config = Config::parse_mem(config).unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let client = f.client(id);
    let surface = client.create_window().surface.clone();
    let hint = client
        .state
        .content_type_manager
        .as_ref()
        .unwrap()
        .get_surface_content_type(&surface, &client.qh, ());
    (f, id, surface, hint)
}

fn map(f: &mut Fixture, id: ClientId, surface: &WlSurface) {
    f.client(id).window(surface).commit();
    f.double_roundtrip(id);
    let window = f.client(id).window(surface);
    window.attach_new_buffer();
    window.ack_last_and_commit();
    f.double_roundtrip(id);
}

fn rules(f: &mut Fixture) -> ResolvedWindowRules {
    f.clarion()
        .layout
        .windows()
        .next()
        .unwrap()
        .1
        .rules()
        .clone()
}

#[test]
fn content_hint_changes_rules_only_on_commit() {
    let (mut f, id, surface, hint) = setup(
        r#"
        window-rule {
            match content-type="game"
            variable-refresh-rate true
            opacity 1.0
        }
        window-rule {
            match content-type="video"
            dim-inactive false
        }
    "#,
    );
    map(&mut f, id, &surface);
    assert_eq!(rules(&mut f).variable_refresh_rate, None);

    hint.set_content_type(Type::Game);
    f.double_roundtrip(id);
    assert_eq!(rules(&mut f).variable_refresh_rate, None);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(rules(&mut f).variable_refresh_rate, Some(true));
    assert_eq!(rules(&mut f).opacity, Some(1.0));

    hint.set_content_type(Type::Video);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(rules(&mut f).variable_refresh_rate, None);
    assert_eq!(rules(&mut f).dim_inactive, Some(false));

    hint.destroy();
    f.double_roundtrip(id);
    assert_eq!(rules(&mut f).dim_inactive, Some(false));
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(rules(&mut f).dim_inactive, None);
}

#[test]
fn content_hint_applies_to_initial_configure_and_exclusions() {
    let (mut f, id, surface, hint) = setup(
        r#"
        window-rule {
            match content-type="photo"
            open-floating true
            default-window-height { fixed 240; }
        }
        window-rule {
            exclude content-type="photo"
            opacity 0.5
        }
    "#,
    );
    hint.set_content_type(Type::Photo);
    map(&mut f, id, &surface);
    let clarion = f.clarion();
    let mapped = clarion.layout.windows().next().unwrap().1;
    assert!(mapped.is_floating());
    assert_eq!(mapped.rules().opacity, None);
}

#[test]
fn hint_between_configure_and_mapping_updates_dynamic_rules() {
    let (mut f, id, surface, hint) = setup(
        r#"
        window-rule {
            match content-type="game"
            variable-refresh-rate true
        }
    "#,
    );
    surface.commit();
    f.double_roundtrip(id);
    hint.set_content_type(Type::Game);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    assert_eq!(rules(&mut f).variable_refresh_rate, Some(true));

    f.client(id).window(&surface).attach_null();
    surface.commit();
    f.double_roundtrip(id);
    hint.set_content_type(Type::None);
    map(&mut f, id, &surface);
    assert_eq!(rules(&mut f).variable_refresh_rate, None);
}

#[test]
fn content_matcher_validates_all_values() {
    for value in ["none", "photo", "video", "game"] {
        let config = format!("window-rule {{ match content-type=\"{value}\"; }}");
        assert!(Config::parse_mem(&config).is_ok());
    }
    assert!(Config::parse_mem(r#"window-rule { match content-type="invalid"; }"#).is_err());
}
