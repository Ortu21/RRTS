//! Memoria nemici: ultimi contatti visti per team (0.0.14 scouting).
//!
//! Lo snapshot onesto mostra solo il visibile *ora*; la memoria ricorda
//! *dove* si è visto il nemico e *quando*, così la strategia può marciare su
//! ultime posizioni note e gli scout hanno mete oltre "centro mappa".
//! Tutto puro e deterministico (ordinamenti stabili, tick monotoni).

use crate::units::UnitKind;
use bevy::prelude::*;
use std::collections::BTreeMap;

/// Tick snapshot (4Hz) dopo cui un contatto è dimenticato: 240 = 60s.
pub const MEMORY_TTL_TICKS: u64 = 240;
/// Sotto questa età (30s) un ricordo è "fresco" per decisioni e scouting.
pub const MEMORY_FRESH_TICKS: u64 = 120;

#[derive(Clone, Debug, PartialEq)]
pub struct Contact {
    pub entity_bits: u64,
    pub pos: Vec3,
    pub tick: u64,
    /// Some per unità, None per edifici (che non hanno `UnitKind`).
    pub kind: Option<UnitKind>,
    pub hp: f32,
    pub building: bool,
}

/// Ultimi contatti per team AI. Chiave = team id.
#[derive(Resource, Default, Clone, Debug)]
pub struct EnemyMemory(pub BTreeMap<u8, Vec<Contact>>);

/// Aggiorna la memoria del team con i contatti *visibili ora* (già filtrati
/// per fog dal chiamante, ordinati per `entity_bits` per determinismo).
/// Ritorna il numero di contatti ricordati dopo potatura.
pub fn update_memory(
    memory: &mut Vec<Contact>,
    tick: u64,
    observed: &[(u64, Vec3, Option<UnitKind>, f32, bool)],
) -> usize {
    for (bits, pos, kind, hp, building) in observed {
        match memory.iter_mut().find(|c| c.entity_bits == *bits) {
            Some(contact) => {
                contact.pos = *pos;
                contact.tick = tick;
                contact.kind = *kind;
                contact.hp = *hp;
                contact.building = *building;
            }
            None => memory.push(Contact {
                entity_bits: *bits,
                pos: *pos,
                tick,
                kind: *kind,
                hp: *hp,
                building: *building,
            }),
        }
    }
    memory.retain(|c| tick.saturating_sub(c.tick) <= MEMORY_TTL_TICKS);
    memory.sort_by_key(|c| c.entity_bits);
    memory.len()
}

/// Contatti freschi (età <= `max_age` tick), i più recenti prima.
/// Hook per future stime minaccia pesate per età (oggi coperto da test).
#[allow(dead_code)]
pub fn fresh_contacts(memory: &[Contact], tick: u64, max_age: u64) -> Vec<&Contact> {
    let mut fresh: Vec<&Contact> = memory
        .iter()
        .filter(|c| tick.saturating_sub(c.tick) <= max_age)
        .collect();
    fresh.sort_by(|a, b| {
        b.tick
            .cmp(&a.tick)
            .then_with(|| a.entity_bits.cmp(&b.entity_bits))
    });
    fresh
}

/// Baricentro dei contatti freschi (meta attacco / conferma scout).
/// Hook testato per future euristiche di caccia (oggi lo snapshot espone i
/// ricordi grezzi e la strategia li pesa da sé).
#[allow(dead_code)]
pub fn remembered_centroid(memory: &[Contact], tick: u64, max_age: u64) -> Option<Vec3> {
    let fresh = fresh_contacts(memory, tick, max_age);
    if fresh.is_empty() {
        return None;
    }
    let mut sum = Vec3::ZERO;
    for contact in &fresh {
        sum += contact.pos;
    }
    Some(sum / fresh.len() as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(bits: u64, x: f32, tick_hp: f32) -> (u64, Vec3, Option<UnitKind>, f32, bool) {
        (
            bits,
            Vec3::new(x, 0.0, 0.0),
            Some(UnitKind::Tank),
            tick_hp,
            false,
        )
    }

    #[test]
    fn insert_update_prune_are_deterministic() {
        let mut memory = Vec::new();
        // Inserimento fuori ordine: la memoria resta ordinata per bits.
        update_memory(&mut memory, 10, &[obs(9, 9.0, 100.0), obs(3, 3.0, 100.0)]);
        assert_eq!(
            vec![memory[0].entity_bits, memory[1].entity_bits],
            vec![3, 9]
        );
        // Re-osservato: posizione e tick aggiornati, non duplicato.
        update_memory(&mut memory, 20, &[obs(3, 30.0, 80.0)]);
        assert_eq!(memory.len(), 2);
        assert_eq!(memory[0].pos.x, 30.0);
        assert_eq!(memory[0].tick, 20);
        // TTL: a tick 260 il contatto tick-10 (età 250) scade,
        // quello tick-20 (età 240, al limite) resta.
        update_memory(&mut memory, 20 + MEMORY_TTL_TICKS, &[]);
        assert_eq!(memory.len(), 1);
        assert_eq!(memory[0].entity_bits, 3);
    }

    #[test]
    fn fresh_filter_and_centroid() {
        let mut memory = Vec::new();
        update_memory(&mut memory, 100, &[obs(1, 0.0, 100.0), obs(2, 10.0, 100.0)]);
        update_memory(&mut memory, 150, &[obs(2, 20.0, 100.0)]);
        let fresh = fresh_contacts(&memory, 160, 20);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].entity_bits, 2);
        let centroid = remembered_centroid(&memory, 160, 100).unwrap();
        assert!((centroid.x - 10.0).abs() < 0.001);
        assert!(remembered_centroid(&memory, 10_000, 10).is_none());
    }
}
