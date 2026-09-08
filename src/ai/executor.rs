//! Executor: unico punto che scrive ordini. Rispetta budget APM e non
//! interrompe mai builder già taskati su un sito attivo.

use crate::{
    economy::balance::BuildingKind,
    movement::queue_move,
    orders::{UnitOrder, queue_attack, queue_attack_move, queue_build},
    structures::spawn_building,
    units::{Team, UnitKind},
};
use bevy::prelude::*;

use super::{
    snapshot::AiSnapshot,
    strategy::{AiIntent, find_build_spot},
};

/// Applica intenti con budget: max N ordini unità + 1 build.
/// Le Enqueue factory sono applicate dal chiamante via query mutabile;
/// qui contiamo solo quante ne ha richieste la strategia.
/// Ritorna (ordini_unità, builds, enqueues_richieste, enqueue_targets).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn execute_movement_and_build(
    commands: &mut Commands,
    snapshot: &AiSnapshot,
    intents: &[AiIntent],
    team: u8,
    max_unit_orders: usize,
    grid: &crate::navigation::NavGrid,
    scenario: crate::scenario::Scenario,
    units: &[(Entity, Vec3, UnitKind, UnitOrder)],
    buildings: &[(Team, BuildingKind, Vec3, bool)],
    builders_live: &[(Team, Vec3, f32)],
    unit_footprints: &[(Vec3, f32)],
) -> (usize, usize, Vec<(Entity, UnitKind)>) {
    let mut unit_orders = 0;
    let mut builds = 0;
    let mut enqueues: Vec<(Entity, UnitKind)> = Vec::new();

    // Ordine di arbitraggio = ordine del vettore da decide(): Build (una),
    // Enqueue (solo conteggio qui), AttackMoveAll/Scout, Retreat, FocusFire.
    // 0.0.17: Retreat prima di FocusFire — i feriti ripiegano invece di
    // convergere sul designato; il budget APM taglia dalla coda se pieno.
    for intent in intents {
        match intent {
            AiIntent::Build(kind) => {
                if builds >= 1 || snapshot.active_site.is_some() {
                    continue;
                }
                let mut candidates: Vec<(Entity, Vec3)> = units
                    .iter()
                    .filter(|(_, _, k, o)| {
                        k.is_builder() && matches!(o, UnitOrder::Idle | UnitOrder::HoldPosition)
                    })
                    .map(|(e, pos, _, _)| (*e, *pos))
                    .collect();
                if candidates.is_empty() {
                    continue;
                }
                candidates.sort_by_key(|(e, _)| e.to_bits());
                let (builder_entity, builder_pos) = candidates[0];
                // G1 — Metal solo su spot (regola hard, come il ghost player);
                // altri edifici sulla spirale classica. Lo spot eredita il
                // mult del deposito (centro ×2).
                // 0.0.18 — torrette: spirale sull'hotspot minaccia (fallback
                // base); muri: slot davanti alla prima torretta che non murano
                // le factory. Threat dalla snapshot onesta (mai query dirette).
                let Some(spot) = (if *kind == BuildingKind::Metal {
                    let metals: Vec<Vec3> = buildings
                        .iter()
                        .filter(|(_, k, _, _)| *k == BuildingKind::Metal)
                        .map(|(_, _, p, _)| *p)
                        .collect();
                    super::strategy::find_metal_spot(
                        grid,
                        &snapshot.deposits,
                        &metals,
                        builder_pos,
                        unit_footprints,
                    )
                } else if *kind == BuildingKind::Wall {
                    let hotspot = super::threat::build_threat(snapshot)
                        .hotspot()
                        .unwrap_or_else(|| scenario.attack_target(team as usize));
                    super::strategy::find_wall_spot(
                        grid,
                        team,
                        buildings,
                        builder_pos,
                        unit_footprints,
                        hotspot,
                        scenario.center(team as usize),
                    )
                } else {
                    let anchor = (*kind == BuildingKind::Turret)
                        .then(|| super::threat::build_threat(snapshot).hotspot())
                        .flatten();
                    find_build_spot(
                        grid,
                        team,
                        *kind,
                        scenario,
                        builder_pos,
                        buildings,
                        builders_live,
                        unit_footprints,
                        anchor,
                    )
                }) else {
                    continue;
                };
                let site = spawn_building(commands, Team(team), *kind, spot, false);
                crate::structures::apply_deposit_yield(
                    commands,
                    site,
                    *kind,
                    &snapshot.deposits,
                    spot,
                );
                queue_build(&mut commands.entity(builder_entity), site);
                builds += 1;
            }
            AiIntent::Enqueue { factory, kind } => {
                enqueues.push((*factory, *kind));
            }
            AiIntent::AttackMoveAll { destination } => {
                let mut attackers: Vec<(Entity, UnitOrder)> = units
                    .iter()
                    .filter(|(_, _, k, o)| {
                        crate::units::archetype(*k).armed && !matches!(o, UnitOrder::Build { .. })
                    })
                    .map(|(e, _, _, o)| (*e, o.clone()))
                    .collect();
                attackers.sort_by_key(|(e, _)| e.to_bits());
                for (entity, order) in attackers {
                    if unit_orders >= max_unit_orders {
                        break;
                    }
                    // Isteresi: chi è già in AttackMove verso lo stesso fronte
                    // non viene riordinato (evita churn del planner budgetato).
                    if let UnitOrder::AttackMove { destination: d } = &order
                        && (*destination - *d).length_squared() < 100.0
                    {
                        continue;
                    }
                    queue_attack_move(&mut commands.entity(entity), *destination);
                    unit_orders += 1;
                }
            }
            AiIntent::Scout { destination } => {
                let mut scouts: Vec<Entity> = units
                    .iter()
                    .filter(|(_, _, k, o)| {
                        *k == UnitKind::Scout
                            && matches!(o, UnitOrder::Idle | UnitOrder::HoldPosition)
                    })
                    .map(|(e, _, _, _)| *e)
                    .collect();
                scouts.sort_by_key(|e| e.to_bits());
                if let Some(entity) = scouts.first()
                    && unit_orders < max_unit_orders
                {
                    queue_move(&mut commands.entity(*entity), *destination);
                    unit_orders += 1;
                }
            }
            AiIntent::Retreat { units: low } => {
                // Ripiego ordinato: slot in formazione attorno alla base.
                // Isteresi: chi marcia già verso casa non viene riordinato.
                // Solo unità ancora vive: gli intenti nascono dallo snapshot
                // (fino a 1s fa) e ordinare un morto fa panic al flush.
                let home = scenario.center(team as usize);
                let mut sorted: Vec<Entity> = low
                    .iter()
                    .filter(|e| units.iter().any(|(ue, _, _, _)| ue == *e))
                    .copied()
                    .collect();
                sorted.sort_by_key(|e| e.to_bits());
                let max_radius = sorted
                    .iter()
                    .filter_map(|e| {
                        units
                            .iter()
                            .find(|(ue, _, _, _)| ue == e)
                            .map(|(_, _, k, _)| crate::units::archetype(*k).radius)
                    })
                    .fold(0.5, f32::max);
                let slots = grid
                    .formation_for(sorted.len(), home, 2.5, max_radius)
                    .unwrap_or_else(|| vec![home; sorted.len()]);
                for (entity, slot) in sorted.into_iter().zip(slots) {
                    if unit_orders >= max_unit_orders {
                        break;
                    }
                    let already_home = units.iter().find(|(e, _, _, _)| *e == entity).is_some_and(
                        |(_, _, _, order)| match order {
                            UnitOrder::Move { destination } => {
                                destination.xz().distance(home.xz()) < 20.0
                            }
                            _ => false,
                        },
                    );
                    if already_home {
                        continue;
                    }
                    queue_move(&mut commands.entity(entity), slot);
                    unit_orders += 1;
                }
            }
            AiIntent::FocusFire { target } => {
                // Fino a 3 attaccanti vicini sul bersaglio designato.
                // Isteresi: chi lo attacca già resta dov'è.
                let target_pos = snapshot
                    .visible_enemies
                    .iter()
                    .find(|e| e.entity == *target)
                    .map(|e| e.pos)
                    .unwrap_or_else(|| scenario.center(team as usize));
                let mut attackers: Vec<(Entity, f32)> = units
                    .iter()
                    .filter(|(_, _, k, o)| {
                        crate::units::archetype(*k).armed
                            && !matches!(o, UnitOrder::Build { .. })
                            && !matches!(o, UnitOrder::Attack { target: t } if *t == *target)
                    })
                    .map(|(e, pos, _, _)| (*e, pos.distance_squared(target_pos)))
                    .collect();
                attackers.sort_by(|a, b| {
                    a.1.total_cmp(&b.1)
                        .then_with(|| a.0.to_bits().cmp(&b.0.to_bits()))
                });
                for (entity, _) in attackers.into_iter().take(3) {
                    if unit_orders >= max_unit_orders {
                        break;
                    }
                    queue_attack(&mut commands.entity(entity), *target);
                    unit_orders += 1;
                }
            }
        }
    }
    (unit_orders, builds, enqueues)
}
