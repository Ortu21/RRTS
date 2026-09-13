//! Utility strategy + tactics (pure, testable).
//!
//! Decisioni data-driven: ogni opzione ha scorer 0..1, la personalità pesa
//! le soglie. Aggiungere unità/edifici futuri = nuove righe in tabella +
//! pesi in [`Personality`], mai `if kind == X` nel core.

//! Micro 4Hz: retreat/focus/hold/screen + posizioni tattiche.

use super::decide::{centroid_of, estimate_forces, my_primary_kind};
use super::*;
use crate::ai::{memory::MEMORY_FRESH_TICKS, snapshot::AiSnapshot};
use crate::{orders::UnitOrder, scenario::Scenario, units::UnitKind};
use bevy::prelude::*;

/// Fase A step 2 — distanza d'ingaggio (mio baricentro armato → baricentro
/// minaccia: visibile, altrimenti ricordi freschi). Sconosciuta = 0.0
/// (mischia immediata = vecchia matematica esatta). Pura.
pub fn engagement_range(snapshot: &AiSnapshot) -> f32 {
    let mut sum = Vec3::ZERO;
    let mut n = 0u32;
    for u in snapshot.army() {
        sum += u.pos;
        n += 1;
    }
    if n == 0 {
        return 0.0;
    }
    let mine = sum / n as f32;
    if let Some(vis) = visible_centroid(snapshot) {
        return mine.distance(vis);
    }
    if let Some(rem) = snapshot.remembered_centroid(MEMORY_FRESH_TICKS) {
        return mine.distance(rem);
    }
    0.0
}

/// 0.0.20 — baricentro nemici visibili (punto minaccia per screen/hold).
/// Ordine snapshot (bits) = somma deterministica. `None` senza contatti live
/// (niente posizionamento sui fantasmi). Puro.
pub fn visible_centroid(snapshot: &AiSnapshot) -> Option<Vec3> {
    if snapshot.visible_enemies.is_empty() {
        return None;
    }
    let mut sum = Vec3::ZERO;
    for e in &snapshot.visible_enemies {
        sum += e.pos;
    }
    Some(sum / snapshot.visible_enemies.len() as f32)
}

/// 0.0.20 — ancora di ritirata: torretta completa propria più vicina a casa
/// (copertura cannoni), fallback casa senza torrette. `turrets` = posizioni
/// torrette complete del team ( executor: da edifici vivi). Puro.
pub fn retreat_anchor(turrets: &[Vec3], home: Vec3) -> Vec3 {
    turrets
        .iter()
        .min_by(|a, b| {
            a.distance_squared(home)
                .total_cmp(&b.distance_squared(home))
                .then_with(|| a.x.to_bits().cmp(&b.x.to_bits()))
                .then_with(|| a.z.to_bits().cmp(&b.z.to_bits()))
        })
        .copied()
        .unwrap_or(home)
}

/// 0.0.20 — posizione schermo: 30% da guardia a minaccia (linea davanti alle
/// batterie / alla base). Pura.
pub fn screen_position(guard: Vec3, threat: Vec3) -> Vec3 {
    guard.lerp(threat, 0.3).with_y(0.0)
}

/// Kite: punto di arretramento per unità pressata — proiezione a `range − 4`
/// dalla minaccia lungo la direttrice, SEMPRE (a differenza di `hold_position`,
/// che tiene il terreno se già in gittata: lì va bene, qui serve creare
/// distanza). Degeneri (sopra il nemico): passo +X deterministico. Pura.
pub fn kite_position(unit: Vec3, threat: Vec3, range: f32) -> Vec3 {
    let mut dir = unit - threat;
    dir.y = 0.0;
    let dir = if dir.length_squared() < 1e-6 {
        Vec3::X
    } else {
        dir.normalize()
    };
    (threat + dir * (range - 4.0)).with_y(0.0)
}

/// 0.0.20 — posizione batteria: sul lato armata della minaccia, a
/// `range − 4` (dentro gittata con margine). Già in gittata (distanza
/// ancora-minaccia <= `range − 4`): tiene il terreno (niente avanzate
/// inutili dentro la gittata). Pura.
pub fn hold_position(anchor: Vec3, threat: Vec3, range: f32) -> Vec3 {
    let mut dir = anchor - threat;
    dir.y = 0.0;
    if dir.length_squared() < 1.0 {
        return anchor.with_y(0.0);
    }
    let dist = dir.length();
    if dist <= range - 4.0 {
        return anchor.with_y(0.0);
    }
    (threat + dir / dist * (range - 4.0)).with_y(0.0)
}

/// 0.0.20 — micro a 4Hz: SOLO Retreat/FocusFire/HoldAtMaxRange/Screen, in
/// quest'ordine di priorità (il budget APM taglia dalla coda). Puro e
/// deterministico. Screen/Hold solo per unità ferme (Idle/Hold): chi marcia
/// in un'ondata non viene deviato (niente churn marcia↔schermo a 4Hz); chi
/// arriva a destinazione torna Idle e si riposiziona. Retreat/Focus valgono
/// sempre (urgenza e dominanza scavalcano la marcia).
pub fn decide_micro(
    snapshot: &AiSnapshot,
    personality: &Personality,
    scenario: Scenario,
) -> Vec<AiIntent> {
    let _ = scenario;
    let mut intents = Vec::new();

    // Ritirata: armati sotto soglia HP + arty ferite (<50%: inefficaci, si
    // preservano). Mai i builder taskati sul sito. Il Commander ferito
    // ripiega come gli altri (capitale!). L'executor porta in copertura
    // torrette (fallback base).
    if personality.retreat_hp_frac > 0.0 {
        let mut low: Vec<Entity> = snapshot
            .my_units
            .iter()
            .filter(|u| {
                crate::units::archetype(u.kind).armed
                    && u.max_health > 0.0
                    && (u.health / u.max_health < personality.retreat_hp_frac
                        || (is_arty_role(u.kind) && u.health / u.max_health < WOUNDED_ARTY_FRAC))
                    && !matches!(u.order, UnitOrder::Build { .. })
            })
            .map(|u| u.entity)
            .collect();
        low.sort_by_key(|e| e.to_bits());
        if !low.is_empty() {
            intents.push(AiIntent::Retreat { units: low });
        }
    }

    // Focus fire a priorità minaccia (dps × counter contro il kind primario
    // proprio): solo quando dominante (win_prob oltre soglia), mai
    // inseguimenti suicidi. Fase A step 2: stima range-aware sul contatto
    // live (a contatto ≈ mischia, ma con artiglierie la gittata conta).
    // Snipe: Commander nemico in vista = designato a soglia ridotta (vale il
    // rischio); precede il focus normale.
    let (my_list, foe_list) = estimate_forces(snapshot);
    let win_prob = crate::ai::combat::predict_outcome_at_range(
        &my_list,
        &foe_list,
        engagement_range(snapshot),
    );
    if personality.focus_fire && !snapshot.visible_enemies.is_empty() {
        if win_prob > SNIPE_MIN_WIN_PROB
            && let Some(snipe) = snipe_target(snapshot)
        {
            intents.push(AiIntent::FocusFire { target: snipe });
        } else if win_prob > FOCUS_MIN_WIN_PROB {
            let primary = my_primary_kind(snapshot);
            let target = snapshot
                .visible_enemies
                .iter()
                .max_by(|a, b| {
                    crate::ai::combat::target_priority(a.kind, primary)
                        .total_cmp(&crate::ai::combat::target_priority(b.kind, primary))
                        .then_with(|| b.health.total_cmp(&a.health))
                        .then_with(|| a.entity.to_bits().cmp(&b.entity.to_bits()))
                })
                .map(|e| e.entity);
            if let Some(target) = target {
                intents.push(AiIntent::FocusFire { target });
            }
        }
    }

    // Schermo + batteria: solo a contatto live e solo per unità ferme
    // (niente deviazioni dell'ondata in marcia), TRANNE il kite: le batterie
    // pressate arretrano anche in marcia (mai gli Attack del focus: la
    // pressione del focus resta). La batteria tiene la
    // gittata sul lato armata; lo schermo sta davanti alla batteria
    // (senza batteria: picchetto avanzato al 30% da guardia a minaccia).
    if let Some(threat) = visible_centroid(snapshot) {
        // Kite: per ogni batteria (ferma o in marcia, mai in focus/build),
        // se il nemico più vicino è dentro il 75% della gittata, meta di
        // arretramento sparando lungo la direttrice unità→nemico. Direzione
        // per-unità (non centroide): tiene anche contro fiancheggiatori.
        // Chi kita esce dalla lista Hold (niente doppio ordine).
        let mut kited: Vec<Entity> = Vec::new();
        let mut kites: Vec<(Entity, Vec3)> = Vec::new();
        for u in snapshot.my_units.iter().filter(|u| {
            crate::units::archetype(u.kind).armed
                && is_arty_role(u.kind)
                && u.kind != UnitKind::Scout
                && u.kind != UnitKind::Commander
                && u.max_health > 0.0
                && u.health / u.max_health >= WOUNDED_ARTY_FRAC
                && matches!(
                    u.order,
                    UnitOrder::Idle | UnitOrder::HoldPosition | UnitOrder::Move { .. }
                )
                && !matches!(u.order, UnitOrder::Build { .. })
        }) {
            let range = crate::units::archetype(u.kind).range;
            let mut near: Option<(f32, u64, Vec3)> = None;
            for e in &snapshot.visible_enemies {
                let d = u.pos.xz().distance_squared(e.pos.xz());
                let better = match near {
                    None => true,
                    Some((bd, bb, _)) => {
                        use std::cmp::Ordering as O;
                        match d.total_cmp(&bd) {
                            O::Less => true,
                            O::Greater => false,
                            O::Equal => e.entity.to_bits() < bb,
                        }
                    }
                };
                if better {
                    near = Some((d, e.entity.to_bits(), e.pos));
                }
            }
            if let Some((d_sq, _, npos)) = near
                && d_sq < (range * KITE_LINE_FRAC) * (range * KITE_LINE_FRAC)
            {
                kited.push(u.entity);
                kites.push((u.entity, kite_position(u.pos, npos, range)));
            }
        }
        kited.sort_by_key(|e| e.to_bits());
        kites.sort_by_key(|(e, _)| e.to_bits());
        if !kites.is_empty() {
            intents.push(AiIntent::Kite { moves: kites });
        }
        let mut battery: Vec<(Entity, f32)> = snapshot
            .my_units
            .iter()
            .filter(|u| {
                crate::units::archetype(u.kind).armed
                    && is_arty_role(u.kind)
                    && u.kind != UnitKind::Scout
                    && u.kind != UnitKind::Commander
                    && u.max_health > 0.0
                    && u.health / u.max_health >= WOUNDED_ARTY_FRAC
                    && matches!(u.order, UnitOrder::Idle | UnitOrder::HoldPosition)
                    && !matches!(u.order, UnitOrder::Build { .. })
                    && !kited.contains(&u.entity)
            })
            .map(|u| (u.entity, crate::units::archetype(u.kind).range))
            .collect();
        battery.sort_by_key(|(e, _)| e.to_bits());
        let hold_spot = if battery.is_empty() {
            None
        } else {
            let range = battery.iter().map(|(_, r)| *r).fold(0.0f32, f32::max);
            let units: Vec<Entity> = battery.iter().map(|(e, _)| *e).collect();
            let anchor = centroid_of(snapshot, &units).unwrap_or(threat);
            Some(hold_position(anchor, threat, range))
        };
        let mut melee: Vec<Entity> = snapshot
            .my_units
            .iter()
            .filter(|u| {
                crate::units::archetype(u.kind).armed
                    && !is_arty_role(u.kind)
                    && u.kind != UnitKind::Scout
                    && u.kind != UnitKind::Commander
                    && u.health > 0.0
                    && matches!(u.order, UnitOrder::Idle | UnitOrder::HoldPosition)
                    && !matches!(u.order, UnitOrder::Build { .. })
            })
            .map(|u| u.entity)
            .collect();
        melee.sort_by_key(|e| e.to_bits());
        if !melee.is_empty() {
            let guard = centroid_of(snapshot, &melee).unwrap_or(threat);
            let position = match hold_spot {
                Some(hold) => hold.lerp(threat, 0.35).with_y(0.0),
                None => screen_position(guard, threat),
            };
            intents.push(AiIntent::Screen {
                units: melee,
                position,
            });
        }
        if let Some(position) = hold_spot {
            let units: Vec<Entity> = battery.into_iter().map(|(e, _)| e).collect();
            intents.push(AiIntent::HoldAtMaxRange { units, position });
        }
    }

    intents
}
