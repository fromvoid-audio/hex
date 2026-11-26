//! Negotiated audio output descriptors copied from the original stopped SDK instance.

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
use super::{Configuration, Reader};
#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
use crate::{Error, Result};
#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
use hex_plugin_state::buses::{BusAddress, BusDescriptor, BusDirection, BusKind, BusMedia};

const MAX_OUTPUTS: u8 = 16;
const MAX_NAME_UNITS: usize = 127;
pub(super) const BODY_LIMIT: usize = 51 + MAX_OUTPUTS as usize * (4 + 2 * MAX_NAME_UNITS);

/// Maximum retained descriptor and UTF-8 name capacities on the control lane.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) const MAX_RETAINED_BYTES: usize =
    MAX_OUTPUTS as usize * (size_of::<BusDescriptor>() + MAX_NAME_UNITS * 3);

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn retained_bytes(buses: &[BusDescriptor]) -> Option<usize> {
    buses.iter().try_fold(size_of_val(buses), |bytes, bus| {
        bytes.checked_add(bus.name.capacity())
    })
}

/// None means the original helper could not report a complete inventory.
/// This observation neither activates a bus nor assigns a destination.
#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
pub(crate) fn decode(body: &[u8], config: &Configuration) -> Result<Option<Box<[BusDescriptor]>>> {
    let mut input = Reader::new(body);
    if input.binding()? != config.binding || input.array::<16>()? != config.identity.class.bytes() {
        return Err(Error::Protocol("output bus inventory source mismatch"));
    }
    let count = input.byte()?;
    if count == 0 {
        input.end()?;
        return Ok(None);
    }
    if count > MAX_OUTPUTS || (config.input_channels != 0 && count != 1) {
        return Err(Error::Protocol("invalid negotiated audio output bus count"));
    }
    let mut buses = Vec::new();
    buses
        .try_reserve_exact(usize::from(count))
        .map_err(|_| Error::Protocol("output bus inventory allocation refused"))?;
    if buses.capacity() != usize::from(count) {
        return Err(Error::Protocol(
            "output bus inventory allocation bound differs",
        ));
    }
    for index in 0..count {
        let sdk_index = input.byte()?;
        let channel_count = input.byte()?;
        let flags = input.byte()?;
        let length = usize::from(input.byte()?);
        let active = flags & 1 != 0;
        let auxiliary = flags & 4 != 0;
        if sdk_index != index
            || !(1..=64).contains(&channel_count)
            || flags & !7 != 0
            || (active && channel_count > 2)
            || (index == 0 && (!active || auxiliary || channel_count != config.output_channels))
            || length > MAX_NAME_UNITS
        {
            return Err(Error::Protocol(
                "invalid negotiated audio output descriptor",
            ));
        }
        let raw = input.take(length * 2)?;
        let units = raw
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
        let name_bound = length * 3;
        let mut name = String::new();
        name.try_reserve_exact(name_bound)
            .map_err(|_| Error::Protocol("output bus name allocation refused"))?;
        if name.capacity() != name_bound {
            return Err(Error::Protocol("output bus name allocation bound differs"));
        }
        for unit in char::decode_utf16(units) {
            let ch = unit
                .ok()
                .filter(|ch| *ch != '\0')
                .ok_or(Error::Protocol("invalid audio output bus name"))?;
            name.push(ch);
        }
        buses.push(BusDescriptor {
            address: BusAddress {
                media: BusMedia::Audio,
                direction: BusDirection::Output,
                index: u16::from(index),
            },
            kind: if auxiliary {
                BusKind::Auxiliary
            } else {
                BusKind::Main
            },
            channel_count,
            default_active: flags & 2 != 0,
            active,
            name,
        });
    }
    input.end()?;
    Ok(Some(buses.into_boxed_slice()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{stamp, tests::config};

    #[test]
    fn inventory_refuses_wrong_source_slots_width_names_and_trailing_data() {
        let config = config();
        let mut header = Vec::new();
        stamp(&mut header, config.binding);
        header.extend(config.identity.class.bytes());
        let frame = |suffix: &[u8]| [header.as_slice(), suffix].concat();
        assert_eq!(decode(&frame(&[0]), &config).unwrap(), None);
        let valid = frame(&[1, 0, config.output_channels, 3, 1, 65, 0]);
        let parsed = decode(&valid, &config).unwrap().unwrap();
        assert_eq!((parsed[0].address.index, parsed[0].name.as_str()), (0, "A"));
        for suffix in [
            &[17][..],
            &[1, 1, 2, 3, 0],
            &[1, 0, 0, 3, 0],
            &[1, 0, 2, 7, 0],
            &[1, 0, 2, 2, 0],
            &[1, 0, 2, 11, 0],
            &[1, 0, 2, 3, 128],
            &[1, 0, 2, 3, 1, 0, 220],
            &[1, 0, 2, 3, 1, 0, 0],
            &[1, 0, 2, 3, 2, 65, 0],
            &[0, 0],
        ] {
            assert!(decode(&frame(suffix), &config).is_err(), "{suffix:?}");
        }
        for offset in [2, 34] {
            let mut stale = valid.clone();
            stale[offset] ^= 2;
            assert!(decode(&stale, &config).is_err());
        }
        let request = config.startup_with_output_buses().unwrap();
        let old = config.startup_request().unwrap();
        assert_eq!(&request[..8], b"HXVIST03");
        assert_eq!(&request[8..], &old[8..]);
    }
}
