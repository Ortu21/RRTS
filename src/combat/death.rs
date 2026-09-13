//! Combattimento: HP, armi, targeting guidato da ordini, proiettili, morti.
//!
//! Regole BAR-style: `AttackMove` si ferma e riprende, `Move` spara marciando,
//! `Hold`/`Idle` difendono in piedi. Targeting sempre gated su fog (`can_target`).
//! I marker `Chasing`/`HoldFire` sono locomozione, l'intento resta in `orders::UnitOrder`.

//! Morte: HP, danno, cleanup.

use bevy::prelude::*;

#[derive(Component, Debug, Clone, Copy)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

pub fn apply_damage_to(current: f32, damage: f32) -> f32 {
    (current - damage).max(0.0)
}

pub fn is_dead(health: &Health) -> bool {
    health.current <= 0.0
}

pub(crate) fn process_deaths(mut commands: Commands, units: Query<(Entity, &Health)>) {
    for (entity, health) in &units {
        if is_dead(health) {
            // Descendants (selection rings) are despawned automatically.
            commands.entity(entity).despawn();
        }
    }
}
