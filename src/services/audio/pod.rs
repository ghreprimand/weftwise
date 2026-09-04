//! SPA `Props` pod codec for the PipeWire audio transport.
//!
//! The parsed model and its bounds are always compiled and unit tested; the
//! serialize/deserialize helpers require the `audio-transport` feature because
//! they depend on the optional `pipewire` crate. The bounds reject an empty,
//! oversized, or trailing-byte pod and an over-channel volume array before any
//! allocation, so an untrusted `Props` parameter cannot exhaust memory or be
//! silently narrowed. The parent module re-exports every public item here, so
//! importers and tests use the `services::audio` path unchanged.

use super::{AudioVolume, MAX_VOLUME_LINEAR_MILLIS};

/// Maximum accepted serialized `Props` pod size in bytes.
///
/// A `Props` parameter carrying volume and mute is a few dozen bytes; the cap
/// rejects an oversized or hostile pod before deserialization allocates.
pub const MAX_PROPS_POD_BYTES: usize = 64 * 1024;

/// Maximum accepted audio channel count in a parsed `Props` pod.
///
/// Real endpoints are mono, stereo, or a bounded surround layout; a pod naming
/// more channels than this is rejected rather than truncated so an untrusted
/// value can neither allocate without bound nor be silently narrowed.
pub const MAX_AUDIO_CHANNELS: usize = 64;

/// Parsed subset of a node `Props` parameter.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedProps {
    /// Per-channel linear volumes, if present.
    pub channel_volumes: Vec<AudioVolume>,
    /// Mute state, if present.
    pub muted: Option<bool>,
}

impl ParsedProps {
    /// Mean linear volume across channels, if any were reported.
    #[must_use]
    pub fn mean_volume(&self) -> Option<AudioVolume> {
        if self.channel_volumes.is_empty() {
            return None;
        }
        let total: u64 = self
            .channel_volumes
            .iter()
            .map(|volume| u64::from(volume.linear_millis()))
            .sum();
        let mean = total / self.channel_volumes.len() as u64;
        Some(AudioVolume::from_linear_millis(
            u32::try_from(mean).unwrap_or(MAX_VOLUME_LINEAR_MILLIS),
        ))
    }
}

/// Build a serialized SPA `Props` pod that sets channel volumes and/or mute.
#[cfg(feature = "audio-transport")]
#[must_use]
pub fn build_props_pod(volume: Option<AudioVolume>, muted: Option<bool>) -> Option<Vec<u8>> {
    use pipewire::spa::pod::serialize::PodSerializer;
    use pipewire::spa::pod::{Object, Property, PropertyFlags, Value, ValueArray};
    use std::io::Cursor;

    let mut properties = Vec::new();
    if let Some(volume) = volume {
        properties.push(Property {
            key: pipewire::spa::sys::SPA_PROP_channelVolumes,
            flags: PropertyFlags::empty(),
            value: Value::ValueArray(ValueArray::Float(vec![volume.linear(); 2])),
        });
    }
    if let Some(muted) = muted {
        properties.push(Property {
            key: pipewire::spa::sys::SPA_PROP_mute,
            flags: PropertyFlags::empty(),
            value: Value::Bool(muted),
        });
    }
    if properties.is_empty() {
        return None;
    }
    let object = Value::Object(Object {
        type_: pipewire::spa::sys::SPA_TYPE_OBJECT_Props,
        id: pipewire::spa::sys::SPA_PARAM_Props,
        properties,
    });
    let (cursor, _len) = PodSerializer::serialize(Cursor::new(Vec::new()), &object).ok()?;
    Some(cursor.into_inner())
}

/// Parse a serialized SPA `Props` pod into bounded channel volumes and mute.
#[cfg(feature = "audio-transport")]
#[must_use]
pub fn parse_props_pod(bytes: &[u8]) -> Option<ParsedProps> {
    use pipewire::spa::pod::deserialize::PodDeserializer;
    use pipewire::spa::pod::{Value, ValueArray};

    // Bound the input before deserialization allocates, and reject an empty pod.
    if bytes.is_empty() || bytes.len() > MAX_PROPS_POD_BYTES {
        return None;
    }
    let (rest, value) = PodDeserializer::deserialize_any_from(bytes).ok()?;
    // A single complete pod must consume the whole bounded input; trailing bytes
    // (a second appended pod or padding) make the frame ambiguous, so reject it.
    if !rest.is_empty() {
        return None;
    }
    let Value::Object(object) = value else {
        return None;
    };
    if object.type_ != pipewire::spa::sys::SPA_TYPE_OBJECT_Props {
        return None;
    }
    let mut parsed = ParsedProps::default();
    for property in object.properties {
        if property.key == pipewire::spa::sys::SPA_PROP_channelVolumes {
            if let Value::ValueArray(ValueArray::Float(values)) = property.value {
                // Reject rather than truncate an over-channel array so an
                // untrusted value can neither allocate without bound nor be
                // silently narrowed to a partial channel set.
                if values.len() > MAX_AUDIO_CHANNELS {
                    return None;
                }
                parsed.channel_volumes = values.into_iter().map(AudioVolume::from_linear).collect();
            }
        } else if property.key == pipewire::spa::sys::SPA_PROP_mute
            && let Value::Bool(muted) = property.value
        {
            parsed.muted = Some(muted);
        }
    }
    Some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "audio-transport")]
    #[test]
    fn props_pod_round_trips_volume_and_mute() {
        let volume = AudioVolume::from_linear_millis(250);
        let bytes = build_props_pod(Some(volume), Some(true)).expect("pod");
        let parsed = parse_props_pod(&bytes).expect("parsed");
        assert_eq!(parsed.muted, Some(true));
        let mean = parsed.mean_volume().expect("mean volume");
        assert_eq!(mean.linear_millis(), volume.linear_millis());
    }

    #[cfg(feature = "audio-transport")]
    #[test]
    fn props_pod_rejects_empty_oversized_and_trailing_input() {
        assert!(parse_props_pod(&[]).is_none());
        let oversized = vec![0_u8; MAX_PROPS_POD_BYTES + 1];
        assert!(parse_props_pod(&oversized).is_none());

        let mut trailing =
            build_props_pod(Some(AudioVolume::from_linear_millis(500)), None).expect("pod");
        trailing.extend(build_props_pod(None, Some(true)).expect("second pod"));
        assert!(parse_props_pod(&trailing).is_none());
    }

    #[cfg(feature = "audio-transport")]
    #[test]
    fn props_pod_rejects_over_channel_volume_arrays() {
        use pipewire::spa::pod::serialize::PodSerializer;
        use pipewire::spa::pod::{Object, Property, PropertyFlags, Value, ValueArray};
        use std::io::Cursor;

        let over = MAX_AUDIO_CHANNELS + 1;
        let object = Value::Object(Object {
            type_: pipewire::spa::sys::SPA_TYPE_OBJECT_Props,
            id: pipewire::spa::sys::SPA_PARAM_Props,
            properties: vec![Property {
                key: pipewire::spa::sys::SPA_PROP_channelVolumes,
                flags: PropertyFlags::empty(),
                value: Value::ValueArray(ValueArray::Float(vec![0.5; over])),
            }],
        });
        let (cursor, _len) =
            PodSerializer::serialize(Cursor::new(Vec::new()), &object).expect("serialize");
        assert!(parse_props_pod(&cursor.into_inner()).is_none());

        // Exactly the maximum channel count is accepted.
        let object = Value::Object(Object {
            type_: pipewire::spa::sys::SPA_TYPE_OBJECT_Props,
            id: pipewire::spa::sys::SPA_PARAM_Props,
            properties: vec![Property {
                key: pipewire::spa::sys::SPA_PROP_channelVolumes,
                flags: PropertyFlags::empty(),
                value: Value::ValueArray(ValueArray::Float(vec![0.5; MAX_AUDIO_CHANNELS])),
            }],
        });
        let (cursor, _len) =
            PodSerializer::serialize(Cursor::new(Vec::new()), &object).expect("serialize");
        let parsed = parse_props_pod(&cursor.into_inner()).expect("max channels parse");
        assert_eq!(parsed.channel_volumes.len(), MAX_AUDIO_CHANNELS);
    }
}
