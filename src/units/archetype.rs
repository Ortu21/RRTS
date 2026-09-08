//! Data-driven unit archetypes: every per-kind tunable lives in one table.
//!
//! Adding a unit is a data change, not a systems change: extend `UnitKind`,
//! append one `Archetype` row, and (optionally) pick a visual. Systems read
//! values through `archetype()` or from components composed at spawn, so
//! balance edits never touch scheduling, queries or behaviour code.
//! (Loading rows from RON/JSON files is a possible future step; the table
//! shape is already file-friendly.)

use bevy::prelude::*;

#[derive(
    Component, Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum UnitKind {
    Scout,
    HeavyTank,
    Artillery,
    Commander,
    Engineer,
    LightTank,
    HeavyTank2,
    Artillery2,
}

impl UnitKind {
    pub const ALL: [UnitKind; 8] = [
        Self::Scout,
        Self::HeavyTank,
        Self::Artillery,
        Self::Commander,
        Self::Engineer,
        Self::LightTank,
        Self::HeavyTank2,
        Self::Artillery2,
    ];
    /// Units buildable from factories. The Commander is unique: it spawns
    /// once per team as the initial builder/base and is never queued.
    /// The Engineer is the mobile builder, produced by the laboratory.
    /// Tier-2 units need a LabT2 (see `Archetype.tier` + `Factory.tier`).
    pub const PRODUCIBLE: [UnitKind; 7] = [
        Self::Scout,
        Self::HeavyTank,
        Self::Artillery,
        Self::Engineer,
        Self::LightTank,
        Self::HeavyTank2,
        Self::Artillery2,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn is_commander(self) -> bool {
        matches!(self, Self::Commander)
    }

    pub fn is_builder(self) -> bool {
        matches!(self, Self::Commander | Self::Engineer)
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
    /// Sight for fog of war: normally SMALLER than weapon range, so long
    /// guns need team spotters (vision is shared per team). Kept separate
    /// from acquisition, which stays long for coordinated engagements.
    pub sight: f32,
    pub body: Vec3,
    pub radius: f32,
    /// False for unarmed builders (Engineer): no Weapon components spawned,
    /// combat systems skip them via With<Weapon> filters.
    pub armed: bool,
    /// Construction throughput (work/s) contributed while alive. 0 for troops.
    pub build_power: f64,
    /// Radius enabling placement. 0 for troops.
    pub build_radius: f32,
    /// Production tier: 1 = Factory, 2 = LabT2 (see `Factory.tier`).
    pub tier: u8,
}

pub const ARCHETYPES: [Archetype; 8] = [
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
        sight: 11.0,
        body: Vec3::new(0.45, 0.6, 0.45),
        radius: 0.45,
        armed: true,
        build_power: 0.0,
        build_radius: 0.0,
        tier: 1,
    },
    Archetype {
        name: "heavy_tank",
        max_health: 170.0,
        speed: 5.2,
        hull_turn: 2.4,
        traverse: 2.6,
        aim_tolerance: 0.14,
        range: 19.0,
        cooldown: 1.3,
        damage: 15.0,
        projectile_speed: 30.0,
        acquisition: 30.0,
        sight: 15.0,
        body: Vec3::new(0.65, 0.9, 0.65),
        radius: 0.6,
        armed: true,
        build_power: 0.0,
        build_radius: 0.0,
        tier: 1,
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
        sight: 24.0,
        body: Vec3::new(0.7, 0.9, 0.9),
        radius: 0.65,
        armed: true,
        build_power: 0.0,
        build_radius: 0.0,
        tier: 1,
    },
    Archetype {
        name: "commander",
        max_health: 1200.0,
        speed: 5.0,
        hull_turn: 2.0,
        traverse: 2.5,
        aim_tolerance: 0.14,
        range: 20.0,
        cooldown: 0.35,
        damage: 7.0,
        projectile_speed: 36.0,
        acquisition: 32.0,
        sight: 17.0,
        // Very large hull: ~2.5x tank footprint, unmistakable on the field.
        body: Vec3::new(1.4, 1.6, 1.8),
        radius: 1.4,
        armed: true,
        build_power: crate::economy::balance::COMMANDER_BUILD_POWER,
        build_radius: crate::economy::balance::COMMAND_BUILD_RADIUS,
        tier: 1,
    },
    Archetype {
        name: "engineer",
        max_health: 70.0,
        speed: 8.5,
        hull_turn: 3.5,
        traverse: 0.0,
        aim_tolerance: 1.0,
        range: 0.0,
        cooldown: 1.0,
        damage: 0.0,
        projectile_speed: 1.0,
        acquisition: 0.0,
        sight: crate::economy::balance::ENGINEER_SIGHT,
        body: Vec3::new(0.5, 0.7, 0.55),
        radius: 0.5,
        armed: false,
        build_power: crate::economy::balance::ENGINEER_BUILD_POWER,
        build_radius: crate::economy::balance::ENGINEER_BUILD_RADIUS,
        tier: 1,
    },
    Archetype {
        name: "light_tank",
        max_health: 70.0,
        speed: 10.0,
        hull_turn: 4.0,
        traverse: 4.5,
        aim_tolerance: 0.17,
        range: 13.0,
        cooldown: 0.45,
        damage: 4.0,
        projectile_speed: 34.0,
        acquisition: 28.0,
        sight: 12.0,
        body: Vec3::new(0.45, 0.6, 0.5),
        radius: 0.45,
        armed: true,
        build_power: 0.0,
        build_radius: 0.0,
        tier: 1,
    },
    Archetype {
        name: "heavy_tank2",
        max_health: 272.0,
        speed: 5.2,
        hull_turn: 2.4,
        traverse: 2.6,
        aim_tolerance: 0.14,
        range: 21.9,
        cooldown: 1.3,
        damage: 24.0,
        projectile_speed: 30.0,
        acquisition: 32.0,
        sight: 16.0,
        body: Vec3::new(0.8, 1.05, 0.8),
        radius: 0.7,
        armed: true,
        build_power: 0.0,
        build_radius: 0.0,
        tier: 2,
    },
    Archetype {
        name: "artillery2",
        max_health: 128.0,
        speed: 4.5,
        hull_turn: 1.6,
        traverse: 1.8,
        aim_tolerance: 0.10,
        range: 34.5,
        cooldown: 3.0,
        damage: 40.0,
        projectile_speed: 26.0,
        acquisition: 40.0,
        sight: 26.0,
        body: Vec3::new(0.85, 1.05, 1.05),
        radius: 0.75,
        armed: true,
        build_power: 0.0,
        build_radius: 0.0,
        tier: 2,
    },
];

/// Secondary weapon (missiles) for the Commander. Prova: slow, long range,
/// high damage. Kept as data next to the archetype table so balance edits
/// never touch systems code. `None` for regular units.
#[derive(Debug, Clone, Copy)]
pub struct SecondaryStats {
    pub range: f32,
    pub cooldown: f32,
    pub damage: f32,
    pub projectile_speed: f32,
    pub acquisition: f32,
    pub traverse: f32,
    pub aim_tolerance: f32,
}

pub const COMMANDER_MISSILES: SecondaryStats = SecondaryStats {
    range: 34.0,
    cooldown: 2.5,
    damage: 40.0,
    projectile_speed: 22.0,
    acquisition: 36.0,
    traverse: 1.6,
    aim_tolerance: 0.12,
};

pub fn secondary_stats(kind: UnitKind) -> Option<&'static SecondaryStats> {
    match kind {
        UnitKind::Commander => Some(&COMMANDER_MISSILES),
        _ => None,
    }
}

/// Sight range for fog of war: per-kind spotting, normally shorter than
/// weapon range so long guns need team spotters. The Engineer keeps its own
/// const (unarmed forward builder). Single lookup so balance and visibility
/// never drift apart.
pub fn sight_range(kind: UnitKind) -> f32 {
    archetype(kind).sight
}

pub fn archetype(kind: UnitKind) -> &'static Archetype {
    &ARCHETYPES[kind.index()]
}

/// Deterministic force composition: heavy-front round robin shared by the
/// playground demo and the skirmish benchmark, so both fight the same war.
pub fn kind_for_index(index: usize) -> UnitKind {
    match index % 10 {
        0..=4 => UnitKind::HeavyTank,
        5..=7 => UnitKind::LightTank,
        8 => UnitKind::Scout,
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
            assert!(stats.radius > 0.0);
            if stats.armed {
                assert!(stats.hull_turn > 0.0 && stats.traverse > 0.0);
                assert!(stats.aim_tolerance > 0.0);
                assert!(stats.range > 0.0 && stats.acquisition >= stats.range);
                assert!(stats.cooldown > 0.0 && stats.damage > 0.0);
                assert!(stats.projectile_speed > 0.0);
                // Vision is normally shorter than weapon range: long guns
                // need team spotters (shared visibility).
                assert!(stats.sight > 0.0 && stats.sight < stats.range);
            } else {
                // Unarmed builders contribute construction instead of fire.
                assert!(stats.build_power > 0.0 && stats.build_radius > 0.0);
            }
            if kind.is_builder() {
                assert!(stats.build_power > 0.0 && stats.build_radius > 0.0);
            } else {
                assert_eq!(stats.build_power, 0.0);
            }
        }
        // Builders: commander heavy, engineer light and mobile.
        assert!(
            archetype(UnitKind::Commander).build_power > archetype(UnitKind::Engineer).build_power
        );
        assert!(archetype(UnitKind::Engineer).speed > archetype(UnitKind::Commander).speed);
    }

    #[test]
    fn composition_is_deterministic_and_heavy_front() {
        let kinds: Vec<_> = (0..100).map(kind_for_index).collect();
        assert_eq!(kinds, (0..100).map(kind_for_index).collect::<Vec<_>>());
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| **kind == UnitKind::HeavyTank)
                .count(),
            50
        );
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| **kind == UnitKind::LightTank)
                .count(),
            30
        );
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| **kind == UnitKind::Scout)
                .count(),
            10
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
        let light = archetype(UnitKind::LightTank);
        let heavy = archetype(UnitKind::HeavyTank);
        let artillery = archetype(UnitKind::Artillery);
        assert!(scout.speed > light.speed && light.speed > heavy.speed);
        assert!(heavy.speed > artillery.speed);
        assert!(artillery.range > heavy.range && heavy.range > light.range);
        assert!(artillery.damage > heavy.damage && heavy.damage > light.damage);
        assert!(light.traverse > artillery.traverse);
    }
}
