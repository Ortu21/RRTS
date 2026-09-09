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
    // 0.0.19 — scout già taskati nel tick (un intento Scout = uno scout:
    // `decide()` emette mete distinte per max 2 scout, mai in pila).
    let mut tasked_scouts: Vec<Entity> = Vec::new();

    // Ordine di arbitraggio = ordine del vettore da decide()/decide_micro():
    // Build (una), Enqueue (solo conteggio qui), AttackMoveGroup/Scout,
    // Retreat, FocusFire, Screen, HoldAtMaxRange.
    // 0.0.17: Retreat prima di FocusFire — i feriti ripiegano invece di
    // convergere sul designato; il budget APM taglia dalla coda se pieno.
    // 0.0.20: micro a 4Hz con budget 2 (solo Retreat/Focus/Hold/Screen).
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
            AiIntent::AttackMoveGroup {
                units: group,
                destination,
            } => {
                // 0.0.20 — ondata: SOLO le unità elencate (la strategia ha già
                // escluso capitale/scout/feriti/builder). Validazione viva +
                // isteresi come prima (niente churn del planner budgetato).
                // Solo unità ancora vive: gli intenti nascono dallo snapshot
                // e ordinare un morto fa panic al flush.
                let mut attackers: Vec<(Entity, UnitOrder)> = units
                    .iter()
                    .filter(|(e, _, _, o)| {
                        group.contains(e) && !matches!(o, UnitOrder::Build { .. })
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
            AiIntent::HoldAtMaxRange {
                units: battery,
                position,
            } => {
                // 0.0.20 — batteria in posizione (Move allo stand-off,
                // acquisizione automatica all'arrivo). Riparazione body-aware
                // sullo scafo maggiore; fallback casa se irriparabile.
                // Isteresi: chi è già in Move lì resta.
                let radius = battery
                    .iter()
                    .filter_map(|e| {
                        units
                            .iter()
                            .find(|(ue, _, _, _)| ue == e)
                            .map(|(_, _, k, _)| crate::units::archetype(*k).radius)
                    })
                    .fold(0.5, f32::max);
                let mut placed: Vec<(Entity, Vec3, UnitOrder)> = units
                    .iter()
                    .filter(|(e, _, _, o)| {
                        battery.contains(e) && !matches!(o, UnitOrder::Build { .. })
                    })
                    .map(|(e, pos, _, o)| (*e, *pos, o.clone()))
                    .collect();
                placed.sort_by_key(|(e, _, _)| e.to_bits());
                let mut goal = grid.clear_point_for(*position, radius);
                if !(grid.is_walkable(goal) && grid.has_clearance_for(goal, radius)) {
                    goal = scenario.center(team as usize);
                }
                for (entity, pos, order) in placed {
                    if unit_orders >= max_unit_orders {
                        break;
                    }
                    // Isteresi sulla meta riparata (quella ordinata davvero):
                    // confrontare l'ideale darebbe churn a ogni tick.
                    if let UnitOrder::Move { destination: d } = &order
                        && (goal - *d).length_squared() < 100.0
                    {
                        continue;
                    }
                    // Raggiungibilità: mete walkable ma sigillate (tasche tra
                    // le rocce) manderebbero il planner in fail-loop a 4Hz
                    // (l'ordine fallito viene rimosso e riemesso ogni tick).
                    if grid.find_path_for(pos, goal, radius).is_none() {
                        continue;
                    }
                    queue_move(&mut commands.entity(entity), goal);
                    unit_orders += 1;
                }
            }
            AiIntent::Screen {
                units: line,
                position,
            } => {
                // 0.0.20 — schermo davanti a batterie/base (AttackMove in
                // posizione, ingaggia a contatto). Stessa riparazione e
                // isteresi della batteria (sullo stesso fronte).
                let radius = line
                    .iter()
                    .filter_map(|e| {
                        units
                            .iter()
                            .find(|(ue, _, _, _)| ue == e)
                            .map(|(_, _, k, _)| crate::units::archetype(*k).radius)
                    })
                    .fold(0.5, f32::max);
                let mut screen: Vec<(Entity, Vec3, UnitOrder)> = units
                    .iter()
                    .filter(|(e, _, _, o)| {
                        line.contains(e) && !matches!(o, UnitOrder::Build { .. })
                    })
                    .map(|(e, pos, _, o)| (*e, *pos, o.clone()))
                    .collect();
                screen.sort_by_key(|(e, _, _)| e.to_bits());
                let mut goal = grid.clear_point_for(*position, radius);
                if !(grid.is_walkable(goal) && grid.has_clearance_for(goal, radius)) {
                    goal = scenario.center(team as usize);
                }
                for (entity, pos, order) in screen {
                    if unit_orders >= max_unit_orders {
                        break;
                    }
                    // Isteresi sulla meta riparata (vedi Hold).
                    if let UnitOrder::AttackMove { destination: d } = &order
                        && (goal - *d).length_squared() < 100.0
                    {
                        continue;
                    }
                    // Raggiungibilità come Hold (niente fail-loop a 4Hz).
                    if grid.find_path_for(pos, goal, radius).is_none() {
                        continue;
                    }
                    queue_attack_move(&mut commands.entity(entity), goal);
                    unit_orders += 1;
                }
            }
            AiIntent::Scout { destination } => {
                // 0.0.19 — un intento = uno scout (mete distinte da
                // `frontier_targets`, mai in pila): primo libero non ancora
                // taskato nel tick, in ordine di bits (deterministico).
                // Liberi = Idle/Hold + Move verso meta calda (stesso predicato
                // di `decide()`: la coppia resta 1:1). Waypoint threat-aware +
                // riparazione walkable così la frontiera non genera mai
                // nav-failure. Chi è in rotta fredda la finisce (niente churn
                // da re-target a 1Hz), al prossimo Idle nuova frontiera.
                let threat = super::threat::build_threat(snapshot);
                let mut scouts: Vec<(Entity, Vec3, UnitOrder)> = units
                    .iter()
                    .filter(|(e, _, k, o)| {
                        *k == UnitKind::Scout
                            && !tasked_scouts.contains(e)
                            && (matches!(o, UnitOrder::Idle | UnitOrder::HoldPosition)
                                || matches!(o, UnitOrder::Move { destination }
                                if threat.query(*destination)
                                    > super::scout::SCOUT_THREAT_THRESHOLD))
                    })
                    .map(|(e, pos, _, o)| (*e, *pos, o.clone()))
                    .collect();
                scouts.sort_by_key(|(e, _, _)| e.to_bits());
                if let Some((entity, pos, _)) = scouts.into_iter().next()
                    && unit_orders < max_unit_orders
                {
                    let waypoint = super::scout::scout_waypoint(
                        pos,
                        *destination,
                        &threat,
                        super::scout::SCOUT_THREAT_THRESHOLD,
                    );
                    let repaired = grid.clear_point_for(waypoint, 0.5);
                    if grid.is_walkable(repaired) && grid.has_clearance(repaired) {
                        queue_move(&mut commands.entity(entity), repaired);
                        tasked_scouts.push(entity);
                        unit_orders += 1;
                    }
                }
            }
            AiIntent::Retreat { units: low } => {
                // 0.0.20 — ripiego in copertura torrette: slot in formazione
                // attorno alla torretta completa più vicina a casa (fallback
                // casa senza torrette). Isteresi a 30m dall'ancora (gli slot
                // stanno entro ~15m: niente churn, ma si segue l'ancora se la
                // torretta cade). Solo unità ancora vive: gli intenti nascono
                // dallo snapshot e ordinare un morto fa panic al flush.
                let home = scenario.center(team as usize);
                let turrets: Vec<Vec3> = buildings
                    .iter()
                    .filter(|(t, k, _, site)| {
                        t.0 == team
                            && matches!(*k, BuildingKind::Turret | BuildingKind::Lance)
                            && !site
                    })
                    .map(|(_, _, p, _)| *p)
                    .collect();
                let anchor = super::strategy::retreat_anchor(&turrets, home);
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
                    .formation_for(sorted.len(), anchor, 2.5, max_radius)
                    .unwrap_or_else(|| vec![anchor; sorted.len()]);
                for (entity, slot) in sorted.into_iter().zip(slots) {
                    if unit_orders >= max_unit_orders {
                        break;
                    }
                    let (pos, order) = units
                        .iter()
                        .find(|(e, _, _, _)| *e == entity)
                        .map(|(_, p, _, o)| (*p, o.clone()))
                        .unwrap_or((slot, UnitOrder::Idle));
                    // Isteresi sullo slot assegnato (come Hold/Screen): chi
                    // marcia già verso il suo slot non viene riordinato né
                    // rivalidato — senza, ogni ferito genera 4 repath/s per
                    // tutta la marcia (saturazione planner nei finali lunghi).
                    if let UnitOrder::Move { destination: d } = &order
                        && (slot - *d).length_squared() < 100.0
                    {
                        continue;
                    }
                    let already_home = matches!(order, UnitOrder::Move { destination } if destination.xz().distance(anchor.xz()) < 30.0);
                    if already_home {
                        continue;
                    }
                    // Raggiungibilità come Hold/Screen (niente fail-loop).
                    if grid.find_path_for(pos, slot, max_radius).is_none() {
                        continue;
                    }
                    queue_move(&mut commands.entity(entity), slot);
                    unit_orders += 1;
                }
            }
            AiIntent::FocusFire { target } => {
                // Fino a 3 attaccanti vicini sul bersaglio designato.
                // Isteresi: chi lo attacca già resta dov'è.
                // 0.0.19 — senza Scout: i veloci in prima linea si immolano e
                // costano gli occhi (come nell'ondata, vedi AttackMoveGroup).
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
                            && *k != UnitKind::Scout
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{orders::UnitOrder, units::UnitKind};

    fn open_grid() -> crate::navigation::NavGrid {
        crate::navigation::NavGrid::new(
            crate::navigation::HALF_SIZE,
            crate::navigation::CELL_SIZE,
            vec![],
        )
    }

    #[test]
    fn micro_respects_budget() {
        // 0.0.20 — micro a 4Hz con budget 2: 5 feriti, solo 2 ordini.
        use crate::units::archetype;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.finish();
        app.cleanup();
        let max = archetype(UnitKind::HeavyTank).max_health;
        let mut snap = AiSnapshot {
            team: 1,
            ..Default::default()
        };
        let mut units = Vec::new();
        for _ in 0..5 {
            let e = app.world_mut().spawn_empty().id();
            snap.my_units.push(crate::ai::snapshot::AiUnit {
                entity: e,
                pos: Vec3::ZERO,
                kind: UnitKind::HeavyTank,
                order: UnitOrder::Idle,
                health: max * 0.1,
                max_health: max,
            });
            units.push((e, Vec3::ZERO, UnitKind::HeavyTank, UnitOrder::Idle));
        }
        let low: Vec<Entity> = units.iter().map(|(e, _, _, _)| *e).collect();
        let intents = vec![AiIntent::Retreat { units: low }];
        let grid = open_grid();
        let mut commands = app.world_mut().commands();
        let (orders, builds, _) = execute_movement_and_build(
            &mut commands,
            &snap,
            &intents,
            1,
            crate::ai::MICRO_BUDGET,
            &grid,
            crate::scenario::Scenario::Playground,
            &units,
            &[],
            &[],
            &[],
        );
        assert_eq!(orders, 2, "budget micro = 2 ordini");
        assert_eq!(builds, 0);
        assert_eq!(crate::ai::MICRO_BUDGET, 2);
    }

    #[test]
    fn retreat_does_not_reorder_units_already_marching_to_slot() {
        // Guard anti-churn: chi marcia già verso il suo slot non viene
        // riordinato né rivalidato (prima: 4 repath/s per ferito per tutta
        // la marcia → saturazione planner nei finali lunghi).
        use crate::units::archetype;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.finish();
        app.cleanup();
        let radius = archetype(UnitKind::HeavyTank).radius;
        let grid = open_grid();
        let home = crate::scenario::Scenario::Playground.center(1);
        let mut entities = Vec::new();
        for _ in 0..2 {
            entities.push(app.world_mut().spawn_empty().id());
        }
        let mut sorted = entities.clone();
        sorted.sort_by_key(|e| e.to_bits());
        let slots = grid
            .formation_for(sorted.len(), home, 2.5, radius.max(0.5))
            .unwrap_or_else(|| vec![home; sorted.len()]);
        // Stesse unità, già in Move verso i loro slot: zero ordini.
        let units: Vec<(Entity, Vec3, UnitKind, UnitOrder)> = sorted
            .iter()
            .zip(slots.iter())
            .map(|(e, s)| {
                (
                    *e,
                    Vec3::ZERO,
                    UnitKind::HeavyTank,
                    UnitOrder::Move { destination: *s },
                )
            })
            .collect();
        let low: Vec<Entity> = sorted.clone();
        let snap = AiSnapshot {
            team: 1,
            ..Default::default()
        };
        let intents = vec![AiIntent::Retreat { units: low }];
        let mut commands = app.world_mut().commands();
        let (orders, _, _) = execute_movement_and_build(
            &mut commands,
            &snap,
            &intents,
            1,
            8,
            &grid,
            crate::scenario::Scenario::Playground,
            &units,
            &[],
            &[],
            &[],
        );
        assert_eq!(orders, 0, "niente riordini verso lo stesso slot");
    }
}
