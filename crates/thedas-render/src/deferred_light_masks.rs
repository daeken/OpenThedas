//! Transposed room/domain membership, intersected with tile masks before light iteration.

use super::{GpuLight, MAX_LIGHTS, MAX_ROOMS};
use bytemuck::Zeroable;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Masks {
    /// Room zero is also the unassigned/overflow/no-rooms-view fallback.
    rooms: [[u32; 8]; MAX_ROOMS as usize + 1],
    /// Non-character and character light domains, respectively.
    domains: [[u32; 8]; 2],
}

pub(super) fn build(lights: &[GpuLight]) -> Masks {
    let mut out = Masks::zeroed();
    for (i, light) in lights.iter().take(MAX_LIGHTS).enumerate() {
        let (word, bit) = (i / 32, 1 << (i % 32));
        for (room, mask) in out.rooms.iter_mut().enumerate() {
            if room == 0 || light.rooms[room / 32] & (1 << (room % 32)) != 0 {
                mask[word] |= bit;
            }
        }
        if !(light.shadow[1] > 1.5 && light.shadow[1] < 2.5) {
            out.domains[0][word] |= bit;
        }
        if light.shadow[1] >= 1.5 {
            out.domains[1][word] |= bit;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transposed_masks_match_room_and_domain_rules_at_every_light_index() {
        assert_eq!(std::mem::size_of::<Masks>(), 8256);
        assert!(std::mem::size_of::<Masks>() <= 16 * 1024);
        for count in [0, 1, 31, 32, 33, 98, 191, 192] {
            let lights: Vec<_> = (0..count)
                .map(|i| {
                    let mut l = GpuLight::zeroed();
                    l.shadow[1] = [1.0, 2.0, 3.0][i % 3];
                    for room in 1..=255 {
                        if (room + i) % 7 == 0 || i % 11 == 0 {
                            l.rooms[room / 32] |= 1 << (room % 32);
                        }
                    }
                    l
                })
                .collect();
            let masks = build(&lights);
            for room in 0..=255 {
                for character in [false, true] {
                    let mut indices = Vec::new();
                    for word in 0..6 {
                        let mut bits =
                            masks.rooms[room][word] & masks.domains[usize::from(character)][word];
                        while bits != 0 {
                            let index = word * 32 + bits.trailing_zeros() as usize;
                            indices.push(index);
                            bits &= bits - 1;
                        }
                    }
                    let expected: Vec<_> = lights
                        .iter()
                        .enumerate()
                        .filter(|(_, l)| {
                            (room == 0 || l.rooms[room / 32] & (1 << (room % 32)) != 0)
                                && !((character && l.shadow[1] < 1.5)
                                    || (!character && l.shadow[1] > 1.5 && l.shadow[1] < 2.5))
                        })
                        .map(|(i, _)| i)
                        .collect();
                    assert_eq!(
                        indices, expected,
                        "{count} lights, room {room}, character {character}"
                    );
                }
            }
        }
    }
}
