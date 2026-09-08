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
/// Costante di decadimento confidence (0.0.19, Walsh Pro2 Ch28):
/// `confidence = 1/(1+age/K)` con età in tick snapshot (4Hz).
/// Stesso K della threat map (`THREAT_DECAY_K`): le due non divergono mai
/// (test `confidence_matches_threat_decay` in `threat::tests`).
/// Uso diretto in 0.0.19+ (percezione pesata); oggi solo test: allow per
/// clippy `-D warnings`.
#[allow(dead_code)]
pub const CONFIDENCE_K: f32 = 60.0;

/// Confidence di un ricordo da età in tick snapshot: 1.0 a vista live,
/// decade verso 0 (0.5 a 15s, 0.33 a 30s, 0.2 a 60s/TTL). Pura.
/// Uso diretto in 0.0.19+; oggi solo test: allow per clippy `-D warnings`.
#[allow(dead_code)]
pub fn confidence(age_ticks: u64) -> f32 {
    1.0 / (1.0 + age_ticks as f32 / CONFIDENCE_K)
}

/// Uso diretto in 0.0.19+ (classifica/decadimento per età); oggi solo test:
/// allow per clippy `-D warnings`.
#[allow(dead_code)]
impl Contact {
    /// Età in tick snapshot (4Hz) rispetto a `tick` corrente. Pura.
    pub fn age(&self, tick: u64) -> u64 {
        tick.saturating_sub(self.tick)
    }

    /// Confidence 0..1 del contatto a `tick` corrente (Walsh perception:
    /// contatto → traccia → memoria con decadimento). Pura.
    pub fn confidence(&self, tick: u64) -> f32 {
        confidence(self.age(tick))
    }
}

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
/// 0.0.16 — API ufficiale (usata da `threat.rs` per pesatura per età e come
/// riferimento per `AiSnapshot::fresh_troop_memory`, che opera su `AiMemory`
/// invece che su `Contact`: le due restano allineate per costruzione).
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
/// 0.0.16 — API ufficiale: `AiSnapshot::remembered_centroid` ne replica la
/// matematica su `AiMemory` (lo snapshot espone ricordi grezzi con età, non
/// `Contact` con tick assoluto). Test incrociato in `strategy::tests`.
/// Uso diretto da `threat.rs` in 0.0.17 (ora solo test): allow per clippy `-D warnings`.
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
            Some(UnitKind::HeavyTank),
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

    #[test]
    fn confidence_decays_with_age() {
        // 0.0.19 — Walsh perception: 1/(1+age/60), stesso K della threat.
        assert!((confidence(0) - 1.0).abs() < 1e-6);
        assert!((confidence(60) - 0.5).abs() < 1e-6);
        assert!(confidence(120) < confidence(60));
        assert!(confidence(120) > 0.0);
        // Metodo su Contact: età da tick assoluto.
        let c = Contact {
            entity_bits: 7,
            pos: Vec3::ZERO,
            tick: 100,
            kind: Some(UnitKind::Scout),
            hp: 60.0,
            building: false,
        };
        assert!((c.confidence(100) - 1.0).abs() < 1e-6);
        assert!((c.confidence(160) - confidence(60)).abs() < 1e-6);
        assert_eq!(c.age(90), 0);
    }
}
