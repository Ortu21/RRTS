//! Data-driven unit archetypes: every per-kind tunable lives in one table.
//!
//! Adding a unit is a data change, not a systems change: extend `UnitKind`,
//! append one `Archetype` row, and (optionally) pick a visual. Systems read
//! values through `archetype()` or from components composed at spawn, so
//! balance edits never touch scheduling, queries or behaviour code.
//! (Loading rows from RON/JSON files is a possible future step; the table
//! shape is already file-friendly.)

use bevy::prelude::*;

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnitKind {
    Scout,
    Tank,
    Artillery,
}

impl UnitKind {
    pub const ALL: [UnitKind; 3] = [Self::Scout, Self::Tank, Self::Artillery];

    pub fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Archetype {
    pub name: &'static str,
    pub max_health: f32,
    pub speed: f32,
    pub hull_turn: f32,
    pub traverse: f32,
    pub aim_tolerance: f32,
    pub range: f32,
    pub cooldown: f32,
    pub damage: f32,
    pub projectile_speed: f32,
    pub acquisition: f32,
    pub body: Vec3,
    pub radius: f32,
}

pub const ARCHETYPES: [Archetype; 3] = [
    Archetype {
        name: "scout",
        max_health: 60.0,
        speed: 11.0,
        hull_turn: 4.5,
        traverse: 5.0,
        aim_tolerance: 0.17,
        range: 14.0,
        cooldown: 0.7,
        damage: 6.0,
        projectile_speed: 34.0,
        acquisition: 34.0,
        body: Vec3::new(0.45, 0.6, 0.45),
        radius: 0.45,
    },
    Archetype {
        name: "tank",
        max_health: 120.0,
        speed: 7.0,
        hull_turn: 2.8,
        traverse: 3.0,
        aim_tolerance: 0.14,
        range: 18.0,
        cooldown: 1.0,
        damage: 10.0,
        projectile_speed: 30.0,
        acquisition: 30.0,
        body: Vec3::new(0.55, 0.8, 0.55),
        radius: 0.55,
    },
    Archetype {
        name: "artillery",
        max_health: 80.0,
        speed: 4.5,
        hull_turn: 1.6,
        traverse: 1.8,
        aim_tolerance: 0.10,
        range: 30.0,
        cooldown: 3.0,
        damage: 25.0,
        projectile_speed: 26.0,
        acquisition: 36.0,
        body: Vec3::new(0.7, 0.9, 0.9),
        radius: 0.65,
    },
];

pub fn archetype(kind: UnitKind) -> &'static Archetype {
    &ARCHETYPES[kind.index()]
}

/// Deterministic force composition: tank-heavy round robin shared by the
/// playground demo and the skirmish benchmark, so both fight the same war.
pub fn kind_for_index(index: usize) -> UnitKind {
    match index % 10 {
        0..=5 => UnitKind::Tank,
        6..=8 => UnitKind::Scout,
        _ => UnitKind::Artillery,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_covers_every_kind_with_sane_values() {
        assert_eq!(ARCHETYPES.len(), UnitKind::ALL.len());
        for (index, kind) in UnitKind::ALL.into_iter().enumerate() {
            assert_eq!(kind.index(), index);
            let stats = archetype(kind);
            assert!(stats.max_health > 0.0);
            assert!(stats.speed > 0.0);
            assert!(stats.hull_turn > 0.0 && stats.traverse > 0.0);
            assert!(stats.aim_tolerance > 0.0);
            assert!(stats.range > 0.0 && stats.acquisition >= stats.range);
            assert!(stats.cooldown > 0.0 && stats.damage > 0.0);
            assert!(stats.projectile_speed > 0.0);
            assert!(stats.radius > 0.0);
        }
    }

    #[test]
    fn composition_is_deterministic_and_tank_heavy() {
        let kinds: Vec<_> = (0..100).map(kind_for_index).collect();
        assert_eq!(kinds, (0..100).map(kind_for_index).collect::<Vec<_>>());
        assert_eq!(
            kinds.iter().filter(|kind| **kind == UnitKind::Tank).count(),
            60
        );
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| **kind == UnitKind::Scout)
                .count(),
            30
        );
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| **kind == UnitKind::Artillery)
                .count(),
            10
        );
    }

    #[test]
    fn kinds_handle_differently_by_design() {
        let scout = archetype(UnitKind::Scout);
        let tank = archetype(UnitKind::Tank);
        let artillery = archetype(UnitKind::Artillery);
        assert!(scout.speed > tank.speed && tank.speed > artillery.speed);
        assert!(artillery.range > tank.range && tank.range > scout.range);
        assert!(artillery.damage > tank.damage && tank.damage > scout.damage);
        assert!(scout.traverse > artillery.traverse);
    }
}
