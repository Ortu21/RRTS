//! Utility strategy + tactics (pure, testable).
//!
//! Decisioni data-driven: ogni opzione ha scorer 0..1, la personalità pesa
//! le soglie. Aggiungere unità/edifici futuri = nuove righe in tabella +
//! pesi in [`Personality`], mai `if kind == X` nel core.

use crate::{
    economy::balance::{BuildingKind, MAX_QUEUE},
    orders::UnitOrder,
    scenario::Scenario,
    structures::{factory_spawn_ok, placement_rule, snap_to_grid, valid_ground},
    units::{UnitKind, archetype},
};
use bevy::prelude::*;

use super::{memory::MEMORY_FRESH_TICKS, snapshot::AiSnapshot};

/// Personalità data-driven (oggi const, domani `.ron` senza toccare il core).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Personality {
    pub name: &'static str,
    /// Quante truppe prima di considerare l'attacco.
    pub army_threshold: usize,
    /// Moltiplicatore potenza richiesta vs nemico visibile.
    pub attack_power_mult: f32,
    /// Costruisci Engineer prima del secondo Tank (turtle eco).
    pub engineer_first: bool,
    /// Secondo Solar prima di attaccare.
    pub second_solar: bool,
    /// Unità da produrre di default.
    pub default_troop: UnitKind,
    /// 0.0.13 — ritirata sotto questa frazione HP (0 = mai).
    pub retreat_hp_frac: f32,
    /// 0.0.13 — focus fire sul nemico più debole quando dominante.
    pub focus_fire: bool,
}

impl Personality {
    pub const TURTLE: Self = Self {
        name: "turtle",
        army_threshold: 4,
        attack_power_mult: 1.5,
        engineer_first: true,
        second_solar: true,
        default_troop: UnitKind::Tank,
        retreat_hp_frac: 0.35,
        focus_fire: true,
    };
    pub const RUSHER: Self = Self {
        name: "rusher",
        army_threshold: 2,
        attack_power_mult: 1.0,
        engineer_first: false,
        second_solar: false,
        default_troop: UnitKind::Tank,
        retreat_hp_frac: 0.25,
        focus_fire: true,
    };

    pub fn from_name(name: &str) -> Self {
        match name {
            "rusher" => Self::RUSHER,
            _ => Self::TURTLE,
        }
    }
}

/// Potenza combattimento stile Lanchester: hp * dps. Engineer disarmato = 0.
pub fn combat_power(kind: UnitKind, health: f32) -> f32 {
    let stats = archetype(kind);
    if !stats.armed || health <= 0.0 {
        return 0.0;
    }
    let dps = stats.damage / stats.cooldown.max(0.05);
    let mut power = health * dps;
    if let Some(sec) = archetype::secondary_stats(kind) {
        power += health * (sec.damage / sec.cooldown.max(0.05)) * 0.7;
    }
    power
}

pub fn army_power(snapshot: &AiSnapshot) -> f32 {
    snapshot
        .my_units
        .iter()
        .map(|u| combat_power(u.kind, u.health))
        .sum()
}

pub fn visible_enemy_power(snapshot: &AiSnapshot) -> f32 {
    snapshot
        .visible_enemies
        .iter()
        .map(|e| combat_power(e.kind, e.health))
        .sum()
}

#[derive(Clone, Debug, PartialEq)]
pub enum AiIntent {
    Build(BuildingKind),
    Enqueue {
        factory: Entity,
        kind: UnitKind,
    },
    AttackMoveAll {
        destination: Vec3,
    },
    Scout {
        destination: Vec3,
    },
    /// 0.0.13 — queste unità (hp bassi) ripiegano alla base con Move
    /// (spara in marcia, mai chase suicida).
    Retreat {
        units: Vec<Entity>,
    },
    /// 0.0.13 — i più vicini convergono su questo nemico (solo dominante).
    FocusFire {
        target: Entity,
    },
}

/// Ricordi freschi con potenza stimata (scontata: posizioni non verificate).
pub fn remembered_enemy_power(snapshot: &AiSnapshot) -> f32 {
    snapshot
        .memory
        .iter()
        .filter(|m| m.age_ticks <= MEMORY_FRESH_TICKS && !m.building)
        .filter_map(|m| m.kind.map(|kind| combat_power(kind, m.hp) * 0.5))
        .sum()
}

/// Ha occhi freschi sul nemico (vista live o ricordi recenti)?
pub fn has_fresh_eyes(snapshot: &AiSnapshot) -> bool {
    !snapshot.visible_enemies.is_empty()
        || snapshot
            .memory
            .iter()
            .any(|m| !m.building && m.age_ticks <= MEMORY_FRESH_TICKS)
}

/// Meta scouting 0.0.14: ciclo deterministico su punti strategici (base
/// nemica, centro, fianchi). Il tick snapshot è a 4Hz: nuova meta ogni 2s.
pub fn scout_destination(snapshot: &AiSnapshot, scenario: Scenario) -> Vec3 {
    let foe_base = scenario.attack_target(snapshot.team as usize);
    // Ricordo fresco di truppe: lo scout va a confermare lì.
    let remembered: Vec<Vec3> = snapshot
        .memory
        .iter()
        .filter(|m| !m.building && m.age_ticks <= MEMORY_FRESH_TICKS)
        .map(|m| m.pos)
        .collect();
    if let Some(first) = remembered.first() {
        return *first;
    }
    let flank = Vec3::new(0.0, 0.0, 60.0);
    let points = [foe_base, Vec3::ZERO, foe_base + flank, foe_base - flank];
    points[(snapshot.tick as usize / 8) % points.len()]
}

/// Meta attacco: baricentro dei ricordi freschi se il nemico è sparito dalla
/// vista (inseguimento onesto), altrimenti base nemica.
pub fn attack_destination(snapshot: &AiSnapshot, scenario: Scenario) -> Vec3 {
    if snapshot.visible_enemies.is_empty() {
        let remembered: Vec<Vec3> = snapshot
            .memory
            .iter()
            .filter(|m| !m.building && m.age_ticks <= MEMORY_FRESH_TICKS)
            .map(|m| m.pos)
            .collect();
        if !remembered.is_empty() {
            let mut sum = Vec3::ZERO;
            for pos in &remembered {
                sum += *pos;
            }
            let centroid = sum / remembered.len() as f32;
            if centroid.is_finite() {
                return centroid;
            }
        }
    }
    scenario.attack_target(snapshot.team as usize)
}

/// Utility scoring: ritorna intenti ordinati per priorità. Puro e deterministico.
pub fn decide(
    snapshot: &AiSnapshot,
    personality: &Personality,
    scenario: Scenario,
    factories: &[(Entity, usize, bool)],
) -> Vec<AiIntent> {
    let mut intents = Vec::new();

    // 1. Macro: una costruzione alla volta (regola placement 1 sito/team).
    if snapshot.active_site.is_none() {
        let metal = snapshot.count_building(BuildingKind::Metal);
        let solar = snapshot.count_building(BuildingKind::Solar);
        let factory = snapshot.count_building(BuildingKind::Factory);
        if metal == 0 {
            intents.push(AiIntent::Build(BuildingKind::Metal));
        } else if solar == 0 {
            intents.push(AiIntent::Build(BuildingKind::Solar));
        } else if factory == 0 {
            intents.push(AiIntent::Build(BuildingKind::Factory));
        } else if personality.second_solar && solar < 2 {
            intents.push(AiIntent::Build(BuildingKind::Solar));
        }
    }

    // 2. Produzione: riempi code factory complete. Le bloccate (porte
    // ostruite, prodotto trattenuto) si saltano: accodare lì brucia solo eco.
    for (factory, queue_len, blocked) in factories {
        if *queue_len >= MAX_QUEUE || *blocked {
            continue;
        }
        // Turtle: prima un Engineer per doppio builder, poi truppe.
        let has_engineer = snapshot
            .my_units
            .iter()
            .any(|u| u.kind == UnitKind::Engineer);
        // 0.0.14 — occhi prima di muscoli: senza scout e senza ricordi
        // freschi, una factory produce uno Scout esploratore.
        let has_scout = snapshot.my_units.iter().any(|u| u.kind == UnitKind::Scout);
        let kind = if personality.engineer_first
            && !has_engineer
            && snapshot.complete_building(BuildingKind::Factory) > 0
        {
            UnitKind::Engineer
        } else if !has_scout
            && !has_fresh_eyes(snapshot)
            && snapshot.complete_building(BuildingKind::Factory) > 0
            && UnitKind::PRODUCIBLE.contains(&UnitKind::Scout)
        {
            UnitKind::Scout
        } else {
            personality.default_troop
        };
        // Accoda solo se producibile (mai Commander).
        if UnitKind::PRODUCIBLE.contains(&kind) {
            intents.push(AiIntent::Enqueue {
                factory: *factory,
                kind,
            });
            break; // una enqueue per tick di strategia (economia streaming)
        }
    }

    // 3. Tattica: attacco quando soglia truppe + potenza stimata.
    // La potenza nemica stimata unisce vista live e ricordi freschi scontati.
    let army_count = snapshot.army().len();
    let my_power = army_power(snapshot);
    let foe_power = visible_enemy_power(snapshot) + remembered_enemy_power(snapshot);
    let power_ok = if snapshot.visible_enemies.is_empty() && !has_fresh_eyes(snapshot) {
        // Nemico mai visto: serve massa critica per marciare alla cieca.
        army_count >= personality.army_threshold + 2
    } else {
        my_power > foe_power * personality.attack_power_mult.max(0.1)
            && army_count >= personality.army_threshold
    };
    if power_ok && army_count > 0 {
        intents.push(AiIntent::AttackMoveAll {
            destination: attack_destination(snapshot, scenario),
        });
    } else if !has_fresh_eyes(snapshot) && army_count == 0 {
        // Scout cieco: ciclo su punti strategici se hai uno scout.
        let has_scout = snapshot.my_units.iter().any(|u| u.kind == UnitKind::Scout);
        if has_scout {
            intents.push(AiIntent::Scout {
                destination: scout_destination(snapshot, scenario),
            });
        }
    }

    // 4. Micro 0.0.13 — ritirata: armati sotto soglia HP ripiegano alla base.
    // Mai i builder taskati sul sito (Build): mollare il cantiere peggiora.
    if personality.retreat_hp_frac > 0.0 {
        let mut low: Vec<Entity> = snapshot
            .my_units
            .iter()
            .filter(|u| {
                crate::units::archetype(u.kind).armed
                    && u.max_health > 0.0
                    && u.health / u.max_health < personality.retreat_hp_frac
                    && !matches!(u.order, UnitOrder::Build { .. })
            })
            .map(|u| u.entity)
            .collect();
        low.sort_by_key(|e| e.to_bits());
        if !low.is_empty() {
            intents.push(AiIntent::Retreat { units: low });
        }
    }

    // 5. Micro 0.0.13 — focus fire: il nemico visibile più debole (hp, poi
    // determinismo) solo quando dominante, mai inseguimenti suicidi.
    if personality.focus_fire && !snapshot.visible_enemies.is_empty() && my_power > foe_power * 2.0
    {
        let target = snapshot
            .visible_enemies
            .iter()
            .min_by(|a, b| {
                a.health
                    .total_cmp(&b.health)
                    .then_with(|| a.entity.to_bits().cmp(&b.entity.to_bits()))
            })
            .map(|e| e.entity);
        if let Some(target) = target {
            intents.push(AiIntent::FocusFire { target });
        }
    }

    intents
}

/// Ricerca deterministica dello spot edificabile: spirale dal centro base.
/// Ritorna il primo punto con `valid_ground` + `placement_rule` + path dal
/// builder — e per le Factory anche una porta d'uscita libera, così le truppe
/// in coda spawnano sempre (niente lab murati vivi).
#[allow(clippy::too_many_arguments)]
pub fn find_build_spot(
    grid: &crate::navigation::NavGrid,
    team: u8,
    kind: BuildingKind,
    scenario: Scenario,
    builder_pos: Vec3,
    buildings: &[(crate::units::Team, BuildingKind, Vec3, bool)],
    builders: &[(crate::units::Team, Vec3, f32)],
    units: &[(Vec3, f32)],
) -> Option<Vec3> {
    use std::f32::consts::PI;
    let base = scenario.center(team as usize);
    // Raggi crescenti deterministici dalla base: prima vicino (difendibile),
    // poi espansione. Angoli fissi 16 per anello = ordine stabile.
    for radius in [10.0, 14.0, 18.0, 24.0, 32.0, 42.0, 56.0] {
        for step in 0..16 {
            let angle = step as f32 / 16.0 * 2.0 * PI;
            let ideal = base + Vec3::new(angle.cos() * radius, 0.0, angle.sin() * radius);
            let point = snap_to_grid(ideal.with_y(0.0));
            if valid_ground(grid, kind, point, units).is_err() {
                continue;
            }
            if factory_spawn_ok(grid, kind, point).is_err() {
                continue; // porte murate: la spirale cerca un punto libero
            }
            if placement_rule(crate::units::Team(team), point, buildings, builders).is_err() {
                return None; // sito attivo o nessun builder: inutile cercare oltre
            }
            if grid.find_path(builder_pos, point).is_none() {
                continue;
            }
            return Some(point);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::UnitOrder;
    use std::collections::BTreeMap;

    fn empty_snapshot(team: u8) -> AiSnapshot {
        AiSnapshot {
            team,
            ..Default::default()
        }
    }

    #[test]
    fn power_weights_tank_over_scout_and_ignores_engineer() {
        let tank = combat_power(UnitKind::Tank, 120.0);
        let scout = combat_power(UnitKind::Scout, 60.0);
        assert!(tank > scout);
        assert_eq!(combat_power(UnitKind::Engineer, 70.0), 0.0);
        assert_eq!(combat_power(UnitKind::Tank, 0.0), 0.0);
    }

    #[test]
    fn build_order_metal_solar_factory_in_sequence() {
        let snap = empty_snapshot(1);
        let p = Personality::TURTLE;
        let intents = decide(&snap, &p, Scenario::Playground, &[]);
        assert_eq!(intents, vec![AiIntent::Build(BuildingKind::Metal)]);
    }

    #[test]
    fn rusher_attacks_earlier_than_turtle() {
        use crate::units::archetype;
        // 2 tank full vs 1 tank nemico visto.
        let mut snap = empty_snapshot(1);
        for i in 0..2 {
            snap.my_units.push(super::super::snapshot::AiUnit {
                entity: Entity::from_bits(100 + i),
                pos: Vec3::ZERO,
                kind: UnitKind::Tank,
                order: UnitOrder::Idle,
                health: archetype(UnitKind::Tank).max_health,
                max_health: archetype(UnitKind::Tank).max_health,
            });
        }
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::Tank,
            health: archetype(UnitKind::Tank).max_health,
        });
        // Completa eco per non generare Build che oscurano il test.
        for (i, k) in [
            BuildingKind::Metal,
            BuildingKind::Solar,
            BuildingKind::Factory,
        ]
        .into_iter()
        .enumerate()
        {
            snap.my_buildings.push(super::super::snapshot::AiBuilding {
                entity: Entity::from_bits(500 + i as u64),
                kind: k,
                pos: Vec3::ZERO,
                under_construction: false,
                health: 100.0,
            });
        }
        // Second solar per turtle ancora mancante: rimuovi l'intento build
        // aggiungendo il secondo solar così resta solo la decisione attacco.
        snap.my_buildings.push(super::super::snapshot::AiBuilding {
            entity: Entity::from_bits(600),
            kind: BuildingKind::Solar,
            pos: Vec3::ZERO,
            under_construction: false,
            health: 100.0,
        });
        let _ = BTreeMap::<u8, ()>::new();
        let rush = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[]);
        let turtle = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[]);
        assert!(
            rush.iter()
                .any(|i| matches!(i, AiIntent::AttackMoveAll { .. }))
        );
        // Turtle con 2 tank sotto soglia 4: non attacca.
        assert!(
            !turtle
                .iter()
                .any(|i| matches!(i, AiIntent::AttackMoveAll { .. }))
        );
    }

    #[test]
    fn never_enqueues_commander() {
        let p = Personality {
            default_troop: UnitKind::Commander,
            ..Personality::RUSHER
        };
        let snap = empty_snapshot(1);
        let intents = decide(
            &snap,
            &p,
            Scenario::Playground,
            &[(Entity::from_bits(1), 0, false)],
        );
        // Commander non è PRODUCIBLE: nessun Enqueue generato.
        assert!(!intents.iter().any(|i| matches!(
            i,
            AiIntent::Enqueue {
                kind: UnitKind::Commander,
                ..
            }
        )));
    }

    fn armed_snapshot(team: u8, kinds_hp: &[(UnitKind, f32, UnitOrder)]) -> AiSnapshot {
        use crate::units::archetype;
        let mut snap = empty_snapshot(team);
        for (i, (kind, hp, order)) in kinds_hp.iter().enumerate() {
            let max = archetype(*kind).max_health;
            snap.my_units.push(super::super::snapshot::AiUnit {
                entity: Entity::from_bits(100 + i as u64),
                pos: Vec3::ZERO,
                kind: *kind,
                order: order.clone(),
                health: *hp,
                max_health: max,
            });
        }
        // Eco completa: niente intenti Build a disturbare i test micro.
        for (i, k) in [
            BuildingKind::Metal,
            BuildingKind::Solar,
            BuildingKind::Factory,
            BuildingKind::Solar,
        ]
        .into_iter()
        .enumerate()
        {
            snap.my_buildings.push(super::super::snapshot::AiBuilding {
                entity: Entity::from_bits(500 + i as u64),
                kind: k,
                pos: Vec3::ZERO,
                under_construction: false,
                health: 100.0,
            });
        }
        snap
    }

    #[test]
    fn retreat_triggers_below_threshold_but_not_on_site() {
        use crate::units::archetype;
        let max = archetype(UnitKind::Tank).max_health;
        // Tank al 20% (< 0.25 rusher e < 0.35 turtle): ritirata per entrambi.
        let snap = armed_snapshot(1, &[(UnitKind::Tank, max * 0.2, UnitOrder::Idle)]);
        for p in [Personality::TURTLE, Personality::RUSHER] {
            let intents = decide(&snap, &p, Scenario::Playground, &[]);
            assert!(
                intents
                    .iter()
                    .any(|i| matches!(i, AiIntent::Retreat { .. })),
                "{p:?} dovrebbe ritirare il tank al 20%"
            );
        }
        // Tank sano: nessuna ritirata.
        let healthy = armed_snapshot(1, &[(UnitKind::Tank, max, UnitOrder::Idle)]);
        let intents = decide(&healthy, &Personality::TURTLE, Scenario::Playground, &[]);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Retreat { .. }))
        );
        // Builder ferito ma taskato sul sito: mai mollare il cantiere.
        let site = Entity::from_bits(777);
        let tasked = armed_snapshot(
            1,
            &[(
                UnitKind::Commander,
                archetype(UnitKind::Commander).max_health * 0.1,
                UnitOrder::Build { site },
            )],
        );
        let intents = decide(&tasked, &Personality::TURTLE, Scenario::Playground, &[]);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Retreat { .. }))
        );
    }

    #[test]
    fn focus_fire_picks_weakest_only_when_dominant() {
        use crate::units::archetype;
        let max = archetype(UnitKind::Tank).max_health;
        let mut snap = armed_snapshot(
            1,
            &[
                (UnitKind::Tank, max, UnitOrder::Idle),
                (UnitKind::Tank, max, UnitOrder::Idle),
                (UnitKind::Tank, max, UnitOrder::Idle),
                (UnitKind::Tank, max, UnitOrder::Idle),
            ],
        );
        // Due nemici: il più debole (bits alti) va designato.
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(901),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::Tank,
            health: max * 0.9,
        });
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(902),
            pos: Vec3::new(12.0, 0.0, 0.0),
            kind: UnitKind::Tank,
            health: max * 0.3,
        });
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[]);
        assert_eq!(
            intents.iter().find_map(|i| match i {
                AiIntent::FocusFire { target } => Some(*target),
                _ => None,
            }),
            Some(Entity::from_bits(902))
        );
        // Potenza pari (1 tank vs 1 tank): niente focus fire suicida.
        let mut even = armed_snapshot(1, &[(UnitKind::Tank, max, UnitOrder::Idle)]);
        even.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(903),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::Tank,
            health: max,
        });
        let intents = decide(&even, &Personality::RUSHER, Scenario::Playground, &[]);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::FocusFire { .. }))
        );
    }

    #[test]
    fn scouting_cycles_strategic_points_and_uses_memory() {
        use super::super::snapshot::AiMemory;
        // Scout senza occhi: mete diverse al passare dei tick.
        let mut snap = armed_snapshot(1, &[(UnitKind::Scout, 60.0, UnitOrder::Idle)]);
        snap.tick = 0;
        let early = scout_destination(&snap, Scenario::Playground);
        snap.tick = 8;
        let later = scout_destination(&snap, Scenario::Playground);
        assert_ne!(early, later);
        // Ricordo fresco: lo scout va a confermare lì.
        snap.memory.push(AiMemory {
            pos: Vec3::new(50.0, 0.0, -30.0),
            age_ticks: 10,
            kind: Some(UnitKind::Tank),
            hp: 100.0,
            building: false,
        });
        assert_eq!(
            scout_destination(&snap, Scenario::Playground),
            Vec3::new(50.0, 0.0, -30.0)
        );
        // Attacco su ricordi quando la vista è vuota.
        assert_eq!(
            attack_destination(&snap, Scenario::Playground),
            Vec3::new(50.0, 0.0, -30.0)
        );
    }

    #[test]
    fn factory_builds_scout_when_blind() {
        // Factory completa, niente scout, niente occhi: tocca allo Scout.
        let snap = armed_snapshot(1, &[]);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[(Entity::from_bits(1), 0, false)],
        );
        assert_eq!(
            intents.iter().find_map(|i| match i {
                AiIntent::Enqueue { kind, .. } => Some(*kind),
                _ => None,
            }),
            Some(UnitKind::Scout)
        );
    }

    #[test]
    fn skips_blocked_factories() {
        // Coda libera ma porte ostruite (prodotto trattenuto): niente enqueue,
        // la factory si sblocca da sola quando le porte si liberano.
        let snap = armed_snapshot(1, &[]);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[(Entity::from_bits(1), 0, true)],
        );
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Enqueue { .. }))
        );
    }
}
