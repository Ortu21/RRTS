//! Formazioni: slot deterministici attorno a un centro.
//!
//! Funzioni pure (`formation_slots`, `skirmish_slots`): niente query Bevy.
//! Usate da `navigation` (planning) e `orders` (assegnazione slot unici).

use bevy::prelude::*;

pub fn formation_slots(count: usize, center: Vec3, spacing: f32) -> Vec<Vec3> {
    if count == 0 {
        return Vec::new();
    }
    let columns = (count as f32).sqrt().ceil() as usize;
    let mut slots: Vec<Vec3> = (0..count)
        .map(|index| {
            Vec3::new(
                (index % columns) as f32 * spacing,
                0.0,
                (index / columns) as f32 * spacing,
            )
        })
        .collect();
    // Center the actual occupied slots, including an incomplete final row.
    let centroid = slots.iter().copied().sum::<Vec3>() / count as f32;
    for slot in &mut slots {
        *slot += center - centroid;
    }
    slots
}

/// Deterministic grid fill staging one army at its front edge for skirmish
/// stress tests. Unlike `formation_slots`, the layout scales to any count:
/// spacing shrinks once the block outgrows the map half. Team 0 masses west
/// of the center gap, team 1 mirrors east, so front lines always start
/// inside acquisition range of each other.
pub fn skirmish_slots(count: usize, team: usize, half_size: f32) -> Vec<Vec3> {
    if count == 0 {
        return Vec::new();
    }
    let depth_limit = half_size - 5.0 - 8.0;
    let width_limit = (half_size - 5.0) * 2.0;
    let cols = (count as f32).sqrt().ceil() as usize;
    let rows = count.div_ceil(cols);
    let spacing = 2.2_f32
        .min(depth_limit / cols as f32)
        .min(width_limit / rows as f32);
    let height = rows as f32 * spacing;
    (0..count)
        .map(|index| {
            let column = index % cols;
            let row = index / cols;
            let west_x = -8.0 - (column as f32 + 0.5) * spacing;
            Vec3::new(
                if team == 0 { west_x } else { -west_x },
                0.0,
                -height * 0.5 + (row as f32 + 0.5) * spacing,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_single_unit() {
        let center = Vec3::new(12.0, 0.0, -9.0);
        assert!(formation_slots(0, center, 2.5).is_empty());
        assert_eq!(formation_slots(1, center, 2.5), vec![center]);
    }

    #[test]
    fn slots_are_counted_centered_separated_and_deterministic() {
        let center = Vec3::new(-13.0, 0.0, 21.0);
        for count in [2, 3, 7, 10, 16, 99, 100] {
            let slots = formation_slots(count, center, 2.5);
            assert_eq!(slots.len(), count);
            assert_eq!(slots, formation_slots(count, center, 2.5));
            let centroid = slots.iter().copied().sum::<Vec3>() / count as f32;
            assert!(centroid.distance(center) < 0.0001);
            for (index, slot) in slots.iter().enumerate() {
                assert_eq!(slot.y, center.y);
                for other in &slots[index + 1..] {
                    assert!(slot.distance(*other) >= 2.5 - 0.0001);
                }
            }
        }
    }

    #[test]
    fn skirmish_halves_scale_and_face_each_other() {
        use crate::navigation::HALF_SIZE;
        for count in [1, 100, 1000, 10_000] {
            let west = skirmish_slots(count, 0, HALF_SIZE);
            let east = skirmish_slots(count, 1, HALF_SIZE);
            assert_eq!(west.len(), count);
            assert_eq!(east.len(), count);
            assert_eq!(west, skirmish_slots(count, 0, HALF_SIZE));
            for slot in west.iter().chain(&east) {
                assert!(slot.x.abs() <= HALF_SIZE - 5.0 && slot.z.abs() <= HALF_SIZE - 5.0);
            }
            let west_front = west.iter().map(|slot| slot.x).fold(f32::MIN, f32::max);
            let east_front = east.iter().map(|slot| slot.x).fold(f32::MAX, f32::min);
            assert!(west_front <= -8.0 + 0.0001 && east_front >= 8.0 - 0.0001);
            // Front-packed blocks always open inside acquisition range.
            assert!(east_front - west_front <= 30.0);
        }
    }
}
