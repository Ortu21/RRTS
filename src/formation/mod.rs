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
}
