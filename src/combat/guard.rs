//! Combattimento: HP, armi, targeting guidato da ordini, proiettili, morti.
//!
//! Regole BAR-style: `AttackMove` si ferma e riprende, `Move` spara marciando,
//! `Hold`/`Idle` difendono in piedi. Targeting sempre gated su fog (`can_target`).
//! I marker `Chasing`/`HoldFire` sono locomozione, l'intento resta in `orders::UnitOrder`.

//! Guard: scorta ward amico, follow ravvicinato.

use super::acquire::AttackTarget;
use super::death::{Health, is_dead};
use crate::orders::{UnitOrder, UnitOrderQueue, complete_order};
use crate::units::{Team, Unit};
use bevy::prelude::*;

/// Follow distance: guards hold inside this radius of their ward instead
/// of stacking onto it, and resume following outside of it.
pub const GUARD_RADIUS: f32 = 6.0;

/// Guard ward validation, separate from enemy-lock validation: the ward
/// lives in the order (not the `AttackTarget` slot, which keeps enemy
/// locks), so this must visit every guard with or without a lock.
/// `validate_targets` only sees units carrying `AttackTarget` — a guard
/// holding near its ward has none, and ward death would go unnoticed.
/// A dead, despawned or hostile ward completes the order instead of
/// following a ghost. Health/team queries (not the spatial grid) are
/// authoritative, so completion never depends on index timing.
pub(crate) fn validate_guards(
    mut commands: Commands,
    health: Query<&Health>,
    teams: Query<&Team>,
    units: Query<(Entity, &UnitOrder), With<Unit>>,
    mut queues: Query<&mut UnitOrderQueue>,
) {
    for (entity, order) in &units {
        let UnitOrder::Guard { target: ward } = order else {
            continue;
        };
        let ward_valid = health
            .get(*ward)
            .ok()
            .filter(|health| !is_dead(health))
            .is_some()
            && teams
                .get(*ward)
                .ok()
                .zip(teams.get(entity).ok())
                .is_some_and(|(ward_team, own_team)| !own_team.is_enemy(*ward_team));
        if !ward_valid {
            commands.entity(entity).remove::<AttackTarget>();
            complete_order(&mut commands, entity, &mut queues);
        }
    }
}
