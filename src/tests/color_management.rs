use std::time::Duration;

use smithay::reexports::wayland_protocols::wp::color_management::v1::client::{
    wp_color_management_surface_v1::{self, WpColorManagementSurfaceV1},
    wp_color_manager_v1::{self, WpColorManagerV1},
    wp_image_description_creator_params_v1::{self, WpImageDescriptionCreatorParamsV1},
    wp_image_description_info_v1,
    wp_image_description_v1::{self, WpImageDescriptionV1},
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface as ServerSurface;
use smithay::reexports::wayland_server::Resource;
use wayland_backend::protocol::ProtocolError;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Proxy, WEnum};

use super::client::{Client, ClientId};
use super::Fixture;
use crate::color::{ColorDescription, Primaries, RenderingIntent, SurfaceColor, TransferFunction};
use crate::compositor::State;
use crate::protocols::color_management::{
    surface_color_description_for_surface, ColorManagementHandler, ColorManagementState,
};

fn color_fixture() -> (Fixture, ClientId) {
    let mut fixture = Fixture::new();
    // Headless production deliberately does not advertise an unmanaged HDR
    // renderer. These tests explicitly install the protocol independently of GPU.
    let manager = ColorManagementState::new::<State>(&fixture.clarion().display_handle, true);
    *fixture.clarion_state().color_management_state() = manager;
    fixture.add_output(1, (1920, 1080));
    let id = fixture.add_client();
    fixture.double_roundtrip(id);
    (fixture, id)
}

fn setup() -> (
    Fixture,
    ClientId,
    WlSurface,
    ServerSurface,
    WpColorManagerV1,
    WpColorManagementSurfaceV1,
) {
    let (mut fixture, id) = color_fixture();
    let client = fixture.client(id);
    let surface = client
        .state
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&client.qh, ());
    let manager = client.state.color_manager.as_ref().unwrap().clone();
    let color = manager.get_surface(&surface, &client.qh, ());
    surface.commit();
    fixture.double_roundtrip(id);
    let server_surface = fixture
        .clarion()
        .root_surface
        .keys()
        .find(|s| s.id().protocol_id() == surface.id().protocol_id())
        .unwrap()
        .clone();
    (fixture, id, surface, server_surface, manager, color)
}

fn creator(client: &Client, manager: &WpColorManagerV1) -> WpImageDescriptionCreatorParamsV1 {
    let creator = manager.create_parametric_creator(&client.qh, ());
    creator.set_primaries_named(wp_color_manager_v1::Primaries::Bt2020);
    creator.set_tf_named(wp_color_manager_v1::TransferFunction::St2084Pq);
    creator
}

fn pq(client: &Client, manager: &WpColorManagerV1) -> WpImageDescriptionV1 {
    let creator = creator(client, manager);
    creator.set_mastering_luminance(50, 1000);
    creator.set_max_cll(1000);
    creator.set_max_fall(400);
    creator.create(&client.qh, ())
}

fn has_ready(client: &Client, description: &WpImageDescriptionV1) -> bool {
    client
        .state
        .color_description_events
        .iter()
        .any(|(id, event)| {
            *id == description.id().protocol_id()
                && matches!(event, wp_image_description_v1::Event::Ready2 { .. })
        })
}

fn receive_protocol_error(fixture: &mut Fixture, id: ClientId) -> ProtocolError {
    fixture.client(id).connection.flush().unwrap();
    for _ in 0..10 {
        fixture.state.server.dispatch();
        let client = fixture.client(id);
        client
            .event_loop
            .dispatch(Duration::ZERO, &mut client.state)
            .unwrap();
        if let Some(error) = client.connection.protocol_error() {
            return error;
        }
    }
    panic!("server did not deliver the expected color-management protocol error");
}

#[test]
fn capabilities_advertise_only_implemented_transfer_functions_and_intents() {
    let (mut fixture, id) = color_fixture();
    let events = &fixture.client(id).state.color_manager_events;
    let transfers: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            wp_color_manager_v1::Event::SupportedTfNamed { tf } => Some(*tf),
            _ => None,
        })
        .collect();
    for supported in [
        wp_color_manager_v1::TransferFunction::CompoundPower24,
        wp_color_manager_v1::TransferFunction::Gamma22,
        wp_color_manager_v1::TransferFunction::ExtLinear,
        wp_color_manager_v1::TransferFunction::St2084Pq,
    ] {
        assert!(transfers.contains(&WEnum::Value(supported)));
    }
    assert!(!transfers.contains(&WEnum::Value(wp_color_manager_v1::TransferFunction::Hlg)));
    assert!(!transfers.contains(&WEnum::Value(wp_color_manager_v1::TransferFunction::Srgb)));
    assert!(!events.iter().any(|event| matches!(
        event,
        wp_color_manager_v1::Event::SupportedFeature {
            feature: WEnum::Value(
                wp_color_manager_v1::Feature::IccV2V4
                    | wp_color_manager_v1::Feature::SetPrimaries
                    | wp_color_manager_v1::Feature::SetTfPower
            )
        }
    )));
    assert!(matches!(
        events.last(),
        Some(wp_color_manager_v1::Event::Done)
    ));
}

#[test]
fn description_copy_survives_resource_destruction_and_only_applies_on_commit() {
    let (mut fixture, id, surface, server, manager, color) = setup();
    let description = pq(fixture.client(id), &manager);
    fixture.double_roundtrip(id);
    assert!(has_ready(fixture.client(id), &description));
    color.set_image_description(&description, wp_color_manager_v1::RenderIntent::Perceptual);
    description.destroy();
    fixture.double_roundtrip(id);
    assert_eq!(
        surface_color_description_for_surface(&server),
        SurfaceColor::default()
    );
    surface.commit();
    fixture.double_roundtrip(id);
    let current = surface_color_description_for_surface(&server);
    assert_eq!(current.description.transfer, TransferFunction::Pq);
    assert_eq!(current.description.primaries, Primaries::Bt2020);
    assert_eq!(current.description.reference_luminance, 203.);
    assert_eq!(current.description.max_luminance, 1000.);
    assert_eq!(current.description.max_fall, 400.);
    assert_eq!(current.intent, RenderingIntent::Perceptual);
    color.unset_image_description();
    fixture.double_roundtrip(id);
    assert_eq!(surface_color_description_for_surface(&server), current);
    surface.commit();
    fixture.double_roundtrip(id);
    assert_eq!(
        surface_color_description_for_surface(&server),
        SurfaceColor::default()
    );
}

#[test]
fn destroying_surface_color_object_unsets_on_commit_and_allows_recreation() {
    let (mut fixture, id, surface, server, manager, color) = setup();
    let description = manager.create_windows_scrgb(&fixture.client(id).qh, ());
    fixture.double_roundtrip(id);
    color.set_image_description(&description, wp_color_manager_v1::RenderIntent::Absolute);
    surface.commit();
    fixture.double_roundtrip(id);
    assert_eq!(
        surface_color_description_for_surface(&server).description,
        ColorDescription::windows_scrgb()
    );
    color.destroy();
    fixture.double_roundtrip(id);
    assert_eq!(
        surface_color_description_for_surface(&server).description,
        ColorDescription::windows_scrgb()
    );
    surface.commit();
    fixture.double_roundtrip(id);
    assert_eq!(
        surface_color_description_for_surface(&server),
        SurfaceColor::default()
    );
    let recreated = manager.get_surface(&surface, &fixture.client(id).qh, ());
    recreated.set_image_description(&description, wp_color_manager_v1::RenderIntent::Relative);
    surface.commit();
    fixture.double_roundtrip(id);
    assert_eq!(
        surface_color_description_for_surface(&server).intent,
        RenderingIntent::Relative
    );
}

#[test]
fn windows_bt2100_and_scrgb_are_real_hdr_descriptions() {
    let (mut fixture, id, surface, server, manager, color) = setup();
    let description = manager.create_windows_bt2100(&fixture.client(id).qh, ());
    fixture.double_roundtrip(id);
    assert!(has_ready(fixture.client(id), &description));
    color.set_image_description(&description, wp_color_manager_v1::RenderIntent::Absolute);
    surface.commit();
    fixture.double_roundtrip(id);
    assert_eq!(
        surface_color_description_for_surface(&server).description,
        ColorDescription::windows_bt2100()
    );
}

#[test]
fn default_pq_container_is_ready_without_mastering_metadata() {
    let (mut fixture, id) = color_fixture();
    let client = fixture.client(id);
    let manager = client.state.color_manager.as_ref().unwrap().clone();
    let description = creator(client, &manager).create(&client.qh, ());
    fixture.double_roundtrip(id);
    assert!(has_ready(fixture.client(id), &description));
}

#[test]
fn pq_max_luminance_argument_is_ignored_and_reference_is_independent() {
    let (mut fixture, id, surface, server, manager, color) = setup();
    let client = fixture.client(id);
    let creator = creator(client, &manager);
    creator.set_luminances(0, 0, 200);
    creator.set_mastering_luminance(0, 1500);
    let description = creator.create(&client.qh, ());
    fixture.double_roundtrip(id);
    assert!(has_ready(fixture.client(id), &description));
    color.set_image_description(&description, wp_color_manager_v1::RenderIntent::Relative);
    surface.commit();
    fixture.double_roundtrip(id);
    let current = surface_color_description_for_surface(&server).description;
    assert_eq!(current.reference_luminance, 200.);
    assert_eq!(current.max_luminance, 1500.);
    assert_eq!(current.linear_scale, 10000.);
}

#[test]
fn output_and_preferred_information_are_complete_and_immutable() {
    let (mut fixture, id, surface, _, manager, _) = setup();
    let client = fixture.client(id);
    let output = client.state.outputs.keys().next().unwrap().clone();
    let output = manager.get_output(&output, &client.qh, ());
    let description = output.get_image_description(&client.qh, ());
    let feedback = manager.get_surface_feedback(&surface, &client.qh, ());
    let preferred = feedback.get_preferred_parametric(&client.qh, ());
    fixture.double_roundtrip(id);
    assert!(has_ready(fixture.client(id), &description));
    assert!(has_ready(fixture.client(id), &preferred));
    let info = description.get_information(&fixture.client(id).qh, ());
    fixture.double_roundtrip(id);
    let events: Vec<_> = fixture
        .client(id)
        .state
        .color_information_events
        .iter()
        .filter(|(id, _)| *id == info.id().protocol_id())
        .map(|(_, event)| event)
        .collect();
    assert!(events
        .iter()
        .any(|event| matches!(event, wp_image_description_info_v1::Event::Primaries { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event, wp_image_description_info_v1::Event::TfNamed { .. })));
    assert!(events.iter().any(|event| matches!(
        event,
        wp_image_description_info_v1::Event::Luminances { .. }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        wp_image_description_info_v1::Event::TargetPrimaries { .. }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        wp_image_description_info_v1::Event::TargetLuminance { .. }
    )));
    assert!(matches!(
        events.last(),
        Some(wp_image_description_info_v1::Event::Done)
    ));
    let output_server = fixture.clarion_output(1);
    fixture
        .clarion_state()
        .color_management_state()
        .output_removed(&output_server);
    let failed = output.get_image_description(&fixture.client(id).qh, ());
    fixture.double_roundtrip(id);
    assert!(fixture
        .client(id)
        .state
        .color_description_events
        .iter()
        .any(|(id, event)| *id == failed.id().protocol_id()
            && matches!(
                event,
                wp_image_description_v1::Event::Failed {
                    cause: WEnum::Value(wp_image_description_v1::Cause::NoOutput),
                    ..
                }
            )));
    let client = fixture.client(id);
    let stale_wl_output = client.state.outputs.keys().next().unwrap();
    let new_object = manager.get_output(stale_wl_output, &client.qh, ());
    let failed_again = new_object.get_image_description(&client.qh, ());
    fixture.double_roundtrip(id);
    assert!(fixture
        .client(id)
        .state
        .color_description_events
        .iter()
        .any(|(id, event)| *id == failed_again.id().protocol_id()
            && matches!(
                event,
                wp_image_description_v1::Event::Failed {
                    cause: WEnum::Value(wp_image_description_v1::Cause::NoOutput),
                    ..
                }
            )));
    // The previous snapshot remains usable after output removal.
    let info2 = description.get_information(&fixture.client(id).qh, ());
    fixture.double_roundtrip(id);
    assert!(fixture
        .client(id)
        .state
        .color_information_events
        .iter()
        .any(|(id, event)| *id == info2.id().protocol_id()
            && matches!(event, wp_image_description_info_v1::Event::Done)));
}

#[test]
fn duplicate_surface_is_a_protocol_error() {
    let (mut fixture, id, surface, _, manager, _) = setup();
    manager.get_surface(&surface, &fixture.client(id).qh, ());
    let error = receive_protocol_error(&mut fixture, id);
    assert_eq!(error.code, wp_color_manager_v1::Error::SurfaceExists as u32);
}

#[test]
fn duplicate_transfer_property_is_a_protocol_error() {
    let (mut fixture, id) = color_fixture();
    let client = fixture.client(id);
    let manager = client.state.color_manager.as_ref().unwrap().clone();
    let creator = creator(client, &manager);
    creator.set_tf_named(wp_color_manager_v1::TransferFunction::Gamma22);
    let error = receive_protocol_error(&mut fixture, id);
    assert_eq!(
        error.code,
        wp_image_description_creator_params_v1::Error::AlreadySet as u32
    );
}

#[test]
fn max_fall_above_max_cll_is_a_protocol_error() {
    let (mut fixture, id) = color_fixture();
    let client = fixture.client(id);
    let manager = client.state.color_manager.as_ref().unwrap().clone();
    let creator = creator(client, &manager);
    creator.set_max_cll(400);
    creator.set_max_fall(1000);
    creator.create(&client.qh, ());
    let error = receive_protocol_error(&mut fixture, id);
    assert_eq!(
        error.code,
        wp_image_description_creator_params_v1::Error::InvalidLuminance as u32
    );
}

#[test]
fn unadvertised_hlg_and_rendering_intent_are_rejected() {
    let (mut fixture, id) = color_fixture();
    let client = fixture.client(id);
    let manager = client.state.color_manager.as_ref().unwrap().clone();
    let creator = manager.create_parametric_creator(&client.qh, ());
    creator.set_tf_named(wp_color_manager_v1::TransferFunction::Hlg);
    assert_eq!(
        receive_protocol_error(&mut fixture, id).code,
        wp_image_description_creator_params_v1::Error::InvalidTf as u32
    );
    let (mut fixture, id, _, _, manager, color) = setup();
    let description = pq(fixture.client(id), &manager);
    fixture.double_roundtrip(id);
    color.set_image_description(&description, wp_color_manager_v1::RenderIntent::Saturation);
    assert_eq!(
        receive_protocol_error(&mut fixture, id).code,
        wp_color_management_surface_v1::Error::RenderIntent as u32
    );
}

#[test]
fn incomplete_and_client_information_requests_are_rejected() {
    let (mut fixture, id) = color_fixture();
    let client = fixture.client(id);
    let manager = client.state.color_manager.as_ref().unwrap().clone();
    manager
        .create_parametric_creator(&client.qh, ())
        .create(&client.qh, ());
    assert_eq!(
        receive_protocol_error(&mut fixture, id).code,
        wp_image_description_creator_params_v1::Error::IncompleteSet as u32
    );
    let (mut fixture, id) = color_fixture();
    let client = fixture.client(id);
    let manager = client.state.color_manager.as_ref().unwrap().clone();
    let description = pq(client, &manager);
    fixture.double_roundtrip(id);
    description.get_information(&fixture.client(id).qh, ());
    assert_eq!(
        receive_protocol_error(&mut fixture, id).code,
        wp_image_description_v1::Error::NoInformation as u32
    );
}

#[test]
fn v1_clients_receive_legacy_ready_and_srgb_name() {
    let (mut fixture, id) = color_fixture();
    let client = fixture.client(id);
    let global = client
        .state
        .globals
        .iter()
        .find(|global| global.interface == WpColorManagerV1::interface().name)
        .unwrap();
    let registry = client.display.get_registry(&client.qh, ());
    let manager: WpColorManagerV1 = registry.bind(global.name, 1, &client.qh, ());
    let creator = manager.create_parametric_creator(&client.qh, ());
    creator.set_primaries_named(wp_color_manager_v1::Primaries::Srgb);
    creator.set_tf_named(wp_color_manager_v1::TransferFunction::Srgb);
    let description = creator.create(&client.qh, ());
    fixture.double_roundtrip(id);
    assert!(fixture.client(id).state.color_description_events.iter().any(|(id, event)| *id == description.id().protocol_id() && matches!(event, wp_image_description_v1::Event::Ready { identity } if *identity != 0)));
}

#[test]
fn synchronized_subsurface_color_waits_for_parent_commit() {
    let (mut fixture, id, parent, _, manager, _) = setup();
    let client = fixture.client(id);
    let child = client
        .state
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&client.qh, ());
    let _subsurface = client.state.subcompositor.as_ref().unwrap().get_subsurface(
        &child,
        &parent,
        &client.qh,
        (),
    );
    let color = manager.get_surface(&child, &client.qh, ());
    child.commit();
    parent.commit();
    fixture.double_roundtrip(id);
    let server_child = fixture
        .clarion()
        .root_surface
        .keys()
        .find(|s| s.id().protocol_id() == child.id().protocol_id())
        .unwrap()
        .clone();
    let description = pq(fixture.client(id), &manager);
    fixture.double_roundtrip(id);
    color.set_image_description(&description, wp_color_manager_v1::RenderIntent::Relative);
    child.commit();
    fixture.double_roundtrip(id);
    assert_eq!(
        surface_color_description_for_surface(&server_child),
        SurfaceColor::default()
    );
    parent.commit();
    fixture.double_roundtrip(id);
    assert_eq!(
        surface_color_description_for_surface(&server_child)
            .description
            .transfer,
        TransferFunction::Pq
    );
}

#[test]
fn surface_color_is_inert_after_wl_surface_destruction() {
    let (mut fixture, id, surface, _, _, color) = setup();
    surface.destroy();
    fixture.double_roundtrip(id);
    color.unset_image_description();
    assert_eq!(
        receive_protocol_error(&mut fixture, id).code,
        wp_color_management_surface_v1::Error::Inert as u32
    );
}

#[test]
fn image_record_identity_is_stable_for_repeated_preferred_requests() {
    let (mut fixture, id, surface, _, manager, _) = setup();
    let client = fixture.client(id);
    let feedback = manager.get_surface_feedback(&surface, &client.qh, ());
    let a = feedback.get_preferred(&client.qh, ());
    let b = feedback.get_preferred_parametric(&client.qh, ());
    fixture.double_roundtrip(id);
    let identity = |client: &Client, image: &WpImageDescriptionV1| {
        client
            .state
            .color_description_events
            .iter()
            .find_map(|(id, event)| match event {
                wp_image_description_v1::Event::Ready2 {
                    identity_hi,
                    identity_lo,
                } if *id == image.id().protocol_id() => {
                    Some(((*identity_hi as u64) << 32) | *identity_lo as u64)
                }
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(
        identity(fixture.client(id), &a),
        identity(fixture.client(id), &b)
    );
    assert_ne!(identity(fixture.client(id), &a), 0);
}

#[test]
fn metadata_only_commit_damages_the_existing_buffer_without_reattachment() {
    use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
    use smithay::wayland::compositor::with_states;
    let (mut fixture, id, surface, server, manager, color) = setup();
    let client = fixture.client(id);
    let buffer = client.state.spbm.as_ref().unwrap().create_u32_rgba_buffer(
        u32::MAX,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        &client.qh,
        (),
    );
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 1, 1);
    surface.commit();
    fixture.double_roundtrip(id);
    let (commit, buffer_id) = with_states(&server, |states| {
        let renderer = states
            .data_map
            .get::<RendererSurfaceStateUserData>()
            .unwrap()
            .lock()
            .unwrap();
        (renderer.current_commit(), renderer.buffer().unwrap().id())
    });
    let description = pq(fixture.client(id), &manager);
    fixture.double_roundtrip(id);
    color.set_image_description(&description, wp_color_manager_v1::RenderIntent::Perceptual);
    // No attach, damage, or frame request accompanies the metadata change.
    surface.commit();
    fixture.double_roundtrip(id);
    with_states(&server, |states| {
        let renderer = states
            .data_map
            .get::<RendererSurfaceStateUserData>()
            .unwrap()
            .lock()
            .unwrap();
        assert_eq!(renderer.buffer().unwrap().id(), buffer_id);
        assert_ne!(renderer.current_commit(), commit);
        assert!(!renderer.damage_since(Some(commit)).is_empty());
    });
}

#[test]
fn many_information_children_finish_after_parent_description_is_destroyed() {
    let (mut fixture, id, surface, _, manager, _) = setup();
    let client = fixture.client(id);
    let feedback = manager.get_surface_feedback(&surface, &client.qh, ());
    let description = feedback.get_preferred(&client.qh, ());
    fixture.double_roundtrip(id);
    let client = fixture.client(id);
    let information: Vec<_> = (0..64)
        .map(|_| description.get_information(&client.qh, ()))
        .collect();
    // Child snapshots retain the immutable record independently of their parent.
    description.destroy();
    fixture.double_roundtrip(id);
    for resource in information {
        assert_eq!(
            fixture
                .client(id)
                .state
                .color_information_events
                .iter()
                .filter(|(id, event)| *id == resource.id().protocol_id()
                    && matches!(event, wp_image_description_info_v1::Event::Done))
                .count(),
            1
        );
    }
}
