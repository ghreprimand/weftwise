//! Service-bound regression gates: D-Bus payload caps, exact PipeWire class
//! matching with degraded overflow, cascade removal, pod bounds, and privacy
//! evidence aging.

use weftwise::context::arbitration::Timestamp;
use weftwise::context::privacy::{
    PRIVACY_EVIDENCE_MAX_AGE_MILLIS, PrivacyDomain, PrivacyEvidence, PrivacyState, PrivacyUpdate,
};
use weftwise::services::audio::{
    AudioDirection, AudioNode, AudioNodeId, AudioState, AudioVolume, Generation,
    MAX_PROPS_POD_BYTES, build_props_pod, is_audio_endpoint_class, parse_props_pod,
};
use weftwise::services::capture::{CaptureGraph, DeviceApi, NodeRole, PortDirection};
use weftwise::services::logind::{LOGIND_INHIBITORS_MAX_BYTES, LOGIND_SIGNAL_MAX_BYTES};
use weftwise::services::mpris::{
    MPRIS_LIST_NAMES_MAX_BYTES, MPRIS_METADATA_MAX_BYTES, MPRIS_SIGNAL_MAX_BYTES,
    MPRIS_STRING_PROP_MAX_BYTES, dbus_body_within_cap,
};

#[test]
fn removing_a_device_cascades_to_its_nodes_ports_and_links() {
    let mut graph = CaptureGraph::new();
    assert!(graph.upsert_device(1, DeviceApi::Alsa));
    assert!(graph.upsert_node(2, NodeRole::AudioSource, Some(1)));
    assert!(graph.upsert_node(3, NodeRole::AudioInputStream, None));
    assert!(graph.upsert_port(4, 2, PortDirection::Output));
    assert!(graph.upsert_port(5, 3, PortDirection::Input));
    assert!(graph.upsert_link(6, 4, 5));

    assert!(graph.remove(1));
    assert!(
        graph.is_empty(),
        "dependent graph identities must be released"
    );
}

#[test]
fn cascade_outcome_exposes_released_proxy_identities() {
    let mut graph = CaptureGraph::new();
    graph.upsert_device(1, DeviceApi::Alsa);
    graph.upsert_node(2, NodeRole::AudioSource, Some(1));
    graph.upsert_node(3, NodeRole::AudioInputStream, None);
    graph.upsert_port(4, 2, PortDirection::Output);
    graph.upsert_port(5, 3, PortDirection::Input);
    graph.upsert_link(6, 4, 5);

    let outcome = graph.remove_cascade(1);
    assert!(outcome.changed);
    assert_eq!(outcome.nodes, vec![2, 3]);
    assert_eq!(outcome.links, vec![6]);
}

#[test]
fn props_pod_with_a_trailing_second_pod_is_rejected() {
    let mut bytes = build_props_pod(Some(AudioVolume::from_linear_millis(500)), None)
        .expect("synthetic Props pod");
    bytes.extend(build_props_pod(None, Some(true)).expect("synthetic trailing Props pod"));

    assert!(
        parse_props_pod(&bytes).is_none(),
        "a complete Props pod must consume the bounded input"
    );
}

#[test]
fn oversized_props_pod_is_rejected_before_deserialization() {
    assert!(parse_props_pod(&vec![0; MAX_PROPS_POD_BYTES + 1]).is_none());
}

#[test]
fn dbus_encoded_body_caps_prevent_the_decode_branch() {
    for cap in [
        MPRIS_SIGNAL_MAX_BYTES,
        MPRIS_METADATA_MAX_BYTES,
        MPRIS_LIST_NAMES_MAX_BYTES,
        MPRIS_STRING_PROP_MAX_BYTES,
        LOGIND_SIGNAL_MAX_BYTES,
        LOGIND_INHIBITORS_MAX_BYTES,
    ] {
        let mut decoded = false;
        if dbus_body_within_cap(cap, cap) {
            decoded = true;
        }
        assert!(decoded, "exact cap must remain decodable");

        decoded = false;
        if dbus_body_within_cap(cap + 1, cap) {
            decoded = true;
        }
        assert!(!decoded, "cap + 1 must not enter decode");
    }
}

#[test]
fn only_exact_trimmed_audio_endpoint_classes_are_admitted() {
    assert_eq!(
        is_audio_endpoint_class(" Audio/Sink "),
        Some(AudioDirection::Sink)
    );
    assert_eq!(
        is_audio_endpoint_class("Audio/Source"),
        Some(AudioDirection::Source)
    );
    for near_match in ["Audio/Sink/Virtual", "prefix Audio/Sink", "Audio/SourceX"] {
        assert_eq!(is_audio_endpoint_class(near_match), None);
    }
}

#[test]
fn degraded_audio_state_stays_stale_when_a_node_update_arrives() {
    let node = AudioNode::bounded(
        AudioNodeId::new(1),
        AudioDirection::Sink,
        "synthetic",
        "synthetic",
        AudioVolume::from_linear_millis(500),
        false,
        true,
    );
    let mut state = AudioState::default();
    state.apply_snapshot(
        vec![node.clone()],
        Some(AudioNodeId::new(1)),
        None,
        Generation::new(1),
    );
    state.mark_degraded();
    state.upsert_node(node);
    assert_eq!(
        state.availability,
        weftwise::state::AdapterAvailability::Stale
    );
}

#[test]
fn privacy_evidence_becomes_stale_at_the_exact_monotonic_boundary() {
    let mut privacy = PrivacyDomain::default();
    privacy.apply(
        PrivacyUpdate::Supported {
            evidence: PrivacyEvidence::Microphone,
            supported: true,
        },
        Timestamp::from_millis(0),
    );
    privacy.apply(
        PrivacyUpdate::Observed {
            evidence: PrivacyEvidence::Microphone,
            state: PrivacyState::Active,
        },
        Timestamp::from_millis(0),
    );
    assert!(!privacy.expire_stale(Timestamp::from_millis(PRIVACY_EVIDENCE_MAX_AGE_MILLIS - 1,)));
    assert_eq!(
        privacy.state(PrivacyEvidence::Microphone),
        PrivacyState::Active
    );
    assert!(privacy.expire_stale(Timestamp::from_millis(PRIVACY_EVIDENCE_MAX_AGE_MILLIS,)));
    assert_eq!(
        privacy.state(PrivacyEvidence::Microphone),
        PrivacyState::Stale
    );
    assert!(!privacy.expire_stale(Timestamp::from_millis(PRIVACY_EVIDENCE_MAX_AGE_MILLIS,)));
}
