//! Utility strategy + tactics (pure, testable).
//!
//! Decisioni data-driven: ogni opzione ha scorer 0..1, la personalità pesa
//! le soglie. Aggiungere unità/edifici futuri = nuove righe in tabella +
//! pesi in [`Personality`], mai `if kind == X` nel core.

//! Split per dominio (re-export invariato):
//! - `personality` — data-driven, pesi, `.ron`.
//! - `decide` — scorer macro, courage, counter-comp, ondate.
//! - `waves` — id ondata, gruppo, minaccia base.
//! - `micro` — decide_micro + posizioni tattiche.
//! - `spots` — wall/metal/build/rally.
//!
//! Vedi `ARCHITECTURE.md`.

pub mod decide;
pub mod micro;
pub mod personality;
pub mod spots;
pub mod waves;

pub use decide::{
    AiIntent, FOCUS_MIN_WIN_PROB, FactoryView, LABT2_MIN_ENERGY_INCOME, LABT2_MIN_METAL_INCOME,
    LABT2_MIN_TICK, T2_MIX, TECH_EDGE_MIN, army_power, attack_destination, combat_power, decide,
    decide_with_threat, has_fresh_eyes, remembered_enemy_power, scout_destination, snipe_target,
    tech_pick, visible_enemy_power, win_prob_of,
};
pub use micro::{
    decide_micro, engagement_range, hold_position, kite_position, retreat_anchor, screen_position,
    visible_centroid,
};
pub use personality::{Personality, PersonalityDef};
pub use spots::{default_rally, find_build_spot, find_metal_spot, find_wall_spot, wall_slots};
pub use waves::{
    BASE_THREAT_RADIUS, BATTERY_MIN_RANGE, KITE_LINE_FRAC, SNIPE_MIN_WIN_PROB, WAVE_PERIOD_TICKS,
    WOUNDED_ARTY_FRAC, base_under_threat, base_under_threat_with_map, is_arty_role, is_wave_tick,
    wave_group, wave_id,
};

#[cfg(test)]
mod tests {
    use super::decide::{estimate_forces, foe_mix};
    use super::*;
    use crate::orders::UnitOrder;
    use crate::{
        ai::{memory::MEMORY_FRESH_TICKS, snapshot::AiSnapshot},
        economy::balance::BuildingKind,
        scenario::Scenario,
        units::UnitKind,
    };
    use bevy::prelude::*;
    use std::collections::BTreeMap;

    fn home_deposit() -> crate::structures::MetalDeposit {
        // G1: mondo con un deposito libero (i test che vogliono lo spot
        // occupato lo coprono con edifici nemici/propri sopra).
        crate::structures::MetalDeposit {
            pos: Vec3::new(30.0, 0.0, 0.0),
            mult: 1.0,
        }
    }

    fn empty_snapshot(team: u8) -> AiSnapshot {
        AiSnapshot {
            team,
            deposits: vec![home_deposit()],
            ..Default::default()
        }
    }

    #[test]
    fn power_weights_tank_over_scout_and_ignores_engineer() {
        let tank = combat_power(UnitKind::HeavyTank, 120.0);
        let scout = combat_power(UnitKind::Scout, 60.0);
        assert!(tank > scout);
        assert_eq!(combat_power(UnitKind::Engineer, 70.0), 0.0);
        assert_eq!(combat_power(UnitKind::HeavyTank, 0.0), 0.0);
    }

    #[test]
    fn default_rally_never_points_into_rock() {
        use crate::navigation::{CELL_SIZE, HALF_SIZE, NavGrid, Obstacle};
        // Muro largo 48m tra factory e fronte: il rally grezzo ci finirebbe dentro.
        let grid = NavGrid::new(
            HALF_SIZE,
            CELL_SIZE,
            vec![Obstacle {
                center: Vec2::new(0.0, 0.0),
                half_size: Vec2::new(24.0, 4.0),
            }],
        );
        let from = Vec3::new(-40.0, 0.0, 0.0);
        let target = Vec3::new(100.0, 0.0, 0.0);
        let rally = default_rally(&grid, from, target).expect("rally riparato");
        assert!(grid.is_walkable(rally) && grid.has_clearance_for(rally, 0.5));
        // Punta ancora verso il fronte (non dietro).
        assert!(rally.x > from.x);
        // Senza direzione (factory sul target): niente rally, fallback apron.
        assert_eq!(default_rally(&grid, target, target), None);
        // Campo aperto: il default passa liscio a ~18m.
        let open = NavGrid::new(HALF_SIZE, CELL_SIZE, vec![]);
        let rally = default_rally(&open, from, target).unwrap();
        assert!((rally.x - (from.x + 18.0)).abs() < 3.0);
    }

    #[test]
    fn build_order_metal_solar_factory_in_sequence() {
        let mut snap = empty_snapshot(1);
        idle_commander(&mut snap);
        // Commander + Engineer liberi: bootstrap parallelo, stessa specie.
        idle_engineer(&mut snap);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert_eq!(
            intents,
            vec![
                AiIntent::Build(BuildingKind::Metal),
                AiIntent::Build(BuildingKind::Metal)
            ]
        );
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
                kind: UnitKind::HeavyTank,
                order: UnitOrder::Idle,
                health: archetype(UnitKind::HeavyTank).max_health,
                max_health: archetype(UnitKind::HeavyTank).max_health,
            });
        }
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: archetype(UnitKind::HeavyTank).max_health,
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
        let rush = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        let turtle = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            rush.iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. }))
        );
        // Turtle con 2 tank sotto soglia 4: non attacca.
        assert!(
            !turtle
                .iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. }))
        );
    }

    #[test]
    fn never_enqueues_commander() {
        let p = Personality {
            mix: [
                (UnitKind::Commander, 1),
                (UnitKind::HeavyTank, 1),
                (UnitKind::LightTank, 1),
            ],
            ..Personality::RUSHER
        };
        let snap = empty_snapshot(1);
        let intents = decide(
            &snap,
            &p,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        // Commander non è PRODUCIBLE: nessun Enqueue generato.
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Enqueue { .. }))
        );
    }

    fn fac(
        bits: u64,
        queue_len: usize,
        blocked: bool,
        tier: u8,
        queued: &[UnitKind],
    ) -> FactoryView {
        FactoryView {
            entity: Entity::from_bits(bits),
            queue_len,
            blocked,
            tier,
            queued: queued.to_vec(),
        }
    }

    fn enqueue_kind(intents: &[AiIntent]) -> Option<UnitKind> {
        intents.iter().find_map(|i| match i {
            AiIntent::Enqueue { kind, .. } => Some(*kind),
            _ => None,
        })
    }

    #[test]
    fn mix_chases_target_shares() {
        // 0.0.19 — due Scout vivi (cap max, niente ramo scout): a conteggi
        // vuoti vince il peso maggiore, poi il deficit guida le scelte.
        let scouts = &[
            (UnitKind::Scout, 60.0, UnitOrder::Idle),
            (UnitKind::Scout, 60.0, UnitOrder::Idle),
        ];
        let snap = armed_snapshot(1, scouts);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::HeavyTank));
        // 6 heavy saturano il 60%: tocca al LightTank (0/3).
        let mut heavies: Vec<(UnitKind, f32, UnitOrder)> =
            scouts.iter().map(|(k, h, o)| (*k, *h, o.clone())).collect();
        let max = crate::units::archetype(UnitKind::HeavyTank).max_health;
        for _ in 0..6 {
            heavies.push((UnitKind::HeavyTank, max, UnitOrder::Idle));
        }
        let snap = armed_snapshot(1, &heavies);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::LightTank));
    }

    #[test]
    fn tech_pick_reacts_to_comp_not_noise() {
        use super::super::opponent::OpponentKind;
        let max_l = crate::units::archetype(UnitKind::LightTank).max_health;
        let max_h = crate::units::archetype(UnitKind::HeavyTank).max_health;
        let lights: Vec<(UnitKind, f32)> = vec![(UnitKind::LightTank, max_l); 3];
        let heavies: Vec<(UnitKind, f32)> = vec![(UnitKind::HeavyTank, max_h); 2];
        // Sciame Light → MgTank (1.15); corazze → MortarTank (1.1).
        assert_eq!(
            tech_pick(
                &Personality::TURTLE.tech_mix,
                &lights,
                OpponentKind::Unknown,
                0,
                2,
                1.0
            ),
            Some(UnitKind::MgTank)
        );
        assert_eq!(
            tech_pick(
                &Personality::TURTLE.tech_mix,
                &heavies,
                OpponentKind::Unknown,
                0,
                2,
                1.0
            ),
            Some(UnitKind::MortarTank)
        );
        // Ignoto / cap pieno / pesi 0 → None = mix normale.
        assert_eq!(
            tech_pick(
                &Personality::TURTLE.tech_mix,
                &[],
                OpponentKind::Unknown,
                0,
                2,
                1.0
            ),
            None
        );
        assert_eq!(
            tech_pick(
                &Personality::TURTLE.tech_mix,
                &lights,
                OpponentKind::Unknown,
                2,
                2,
                1.0
            ),
            None
        );
        assert_eq!(
            tech_pick(
                &Personality::ECO_ONLY.tech_mix,
                &lights,
                OpponentKind::Unknown,
                0,
                0,
                1.0
            ),
            None
        );
    }

    #[test]
    fn turtle_builds_mg_vs_light_swarm() {
        use crate::units::archetype;
        // 2 engineer (cap) + 2 scout (cap, niente ramo occhi) + 3 light visti
        // + factory T1 libera → tech reattivo MgTank, non Heavy.
        let max_e = archetype(UnitKind::Engineer).max_health;
        let max_s = archetype(UnitKind::Scout).max_health;
        let max_l = archetype(UnitKind::LightTank).max_health;
        let kinds = &[
            (UnitKind::Engineer, max_e, UnitOrder::Idle),
            (UnitKind::Engineer, max_e, UnitOrder::Idle),
            (UnitKind::Scout, max_s, UnitOrder::Idle),
            (UnitKind::Scout, max_s, UnitOrder::Idle),
        ];
        let mut snap = armed_snapshot(1, kinds);
        for i in 0..3 {
            snap.visible_enemies.push(super::super::snapshot::AiEnemy {
                entity: Entity::from_bits(900 + i),
                pos: Vec3::new(10.0, 0.0, 0.0),
                kind: UnitKind::LightTank,
                health: max_l,
            });
        }
        let intents = decide(
            &snap,
            &Personality::TURTLE,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::MgTank));
        // Stesso ma 2 Mg vivi (cap tech): torna il mix normale (Heavy).
        let mut capped_kinds: Vec<(UnitKind, f32, UnitOrder)> =
            kinds.iter().map(|(k, h, o)| (*k, *h, o.clone())).collect();
        let max_mg = archetype(UnitKind::MgTank).max_health;
        capped_kinds.push((UnitKind::MgTank, max_mg, UnitOrder::Idle));
        capped_kinds.push((UnitKind::MgTank, max_mg, UnitOrder::Idle));
        let mut capped = armed_snapshot(1, &capped_kinds);
        for i in 0..3 {
            capped
                .visible_enemies
                .push(super::super::snapshot::AiEnemy {
                    entity: Entity::from_bits(910 + i),
                    pos: Vec3::new(10.0, 0.0, 0.0),
                    kind: UnitKind::LightTank,
                    health: max_l,
                });
        }
        let intents = decide(
            &capped,
            &Personality::TURTLE,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::HeavyTank));
    }

    #[test]
    fn blind_single_scout_builds_second_up_to_cap() {
        // 0.0.19 — max 2 scout (Welsh): con uno Scout vivo ma ancora alla
        // cieca, la factory accoda il secondo; con due, passa ai muscoli.
        let one = &[(UnitKind::Scout, 60.0, UnitOrder::Idle)];
        let snap = armed_snapshot(1, one);
        assert!(!has_fresh_eyes(&snap));
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::Scout));
        let two = &[
            (UnitKind::Scout, 60.0, UnitOrder::Idle),
            (UnitKind::Scout, 60.0, UnitOrder::Idle),
        ];
        let snap = armed_snapshot(1, two);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::HeavyTank));
    }

    #[test]
    fn t2_units_only_from_labt2() {
        use crate::units::archetype;
        let mut snap = armed_snapshot(1, &[(UnitKind::Scout, 60.0, UnitOrder::Idle)]);
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: archetype(UnitKind::HeavyTank).max_health,
        });
        // Factory T1: solo mix T1, mai T2 anche con occhi.
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        let kind = enqueue_kind(&intents).unwrap();
        assert_eq!(archetype(kind).tier, 1);
        // LabT2: mix pesante T2, prima scelta HeavyTank2.
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(2, 0, false, 2, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::HeavyTank2));
    }

    #[test]
    fn labt2_needs_time_and_factory() {
        let mut snap = armed_snapshot(1, &[]);
        idle_commander(&mut snap);
        // 0.0.18 — il LabT2 vuole eco vera (2 Metal + 2 Solar di income),
        // non solo il tick: la soglia temporale da sola non basta più.
        snap.income = [10.0, 24.0];
        snap.tick = 0;
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::LabT2)))
        );
        snap.tick = LABT2_MIN_TICK;
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::LabT2)));
    }

    #[test]
    fn turret_defense_is_turtle_only_and_capped() {
        let snap = armed_snapshot(1, &[]);
        let turtle = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(turtle.contains(&AiIntent::Build(BuildingKind::Turret)));
        let rush = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            !rush
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Turret)))
        );
        // Cap raggiunto (2 torrette): basta richiederne.
        let mut capped = armed_snapshot(1, &[]);
        for i in 0..2 {
            capped
                .my_buildings
                .push(super::super::snapshot::AiBuilding {
                    entity: Entity::from_bits(700 + i),
                    kind: BuildingKind::Turret,
                    pos: Vec3::ZERO,
                    under_construction: false,
                    health: 100.0,
                });
        }
        let intents = decide(&capped, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Turret)))
        );
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
        let max = archetype(UnitKind::HeavyTank).max_health;
        // Tank al 20% (< 0.25 rusher e < 0.35 turtle): ritirata per entrambi.
        let snap = armed_snapshot(1, &[(UnitKind::HeavyTank, max * 0.2, UnitOrder::Idle)]);
        for p in [Personality::TURTLE, Personality::RUSHER] {
            let intents = decide_micro(&snap, &p, Scenario::Playground);
            assert!(
                intents
                    .iter()
                    .any(|i| matches!(i, AiIntent::Retreat { .. })),
                "{p:?} dovrebbe ritirare il tank al 20%"
            );
        }
        // Tank sano: nessuna ritirata.
        let healthy = armed_snapshot(1, &[(UnitKind::HeavyTank, max, UnitOrder::Idle)]);
        let intents = decide_micro(&healthy, &Personality::TURTLE, Scenario::Playground);
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
        let intents = decide_micro(&tasked, &Personality::TURTLE, Scenario::Playground);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Retreat { .. }))
        );
    }

    #[test]
    fn focus_fire_picks_weakest_only_when_dominant() {
        use crate::units::archetype;
        let max = archetype(UnitKind::HeavyTank).max_health;
        let mut snap = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
            ],
        );
        // Due nemici: il più debole (bits alti) va designato.
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(901),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max * 0.9,
        });
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(902),
            pos: Vec3::new(12.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max * 0.3,
        });
        let intents = decide_micro(&snap, &Personality::RUSHER, Scenario::Playground);
        assert_eq!(
            intents.iter().find_map(|i| match i {
                AiIntent::FocusFire { target } => Some(*target),
                _ => None,
            }),
            Some(Entity::from_bits(902))
        );
        // Potenza pari (1 tank vs 1 tank): niente focus fire suicida.
        let mut even = armed_snapshot(1, &[(UnitKind::HeavyTank, max, UnitOrder::Idle)]);
        even.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(903),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max,
        });
        let intents = decide_micro(&even, &Personality::RUSHER, Scenario::Playground);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::FocusFire { .. }))
        );
    }

    #[test]
    fn courage_blocks_suicide_but_allows_dominance() {
        use crate::units::archetype;
        let max = archetype(UnitKind::HeavyTank).max_health;
        let enemy = |bits: u64| super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(bits),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max,
        };
        // 1 vs 4 a occhi aperti: win_prob ~0, niente ondata suicida.
        let mut weak = armed_snapshot(1, &[(UnitKind::HeavyTank, max, UnitOrder::Idle)]);
        for i in 0..4 {
            weak.visible_enemies.push(enemy(900 + i));
        }
        // Eco completa ma niente torrette: turtle emetterebbe Build(Turret),
        // il rusher no — l'assert resta solo sull'attacco.
        let intents = decide(&weak, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. }))
        );
        // 4 vs 1: win_prob ~1, l'ondata parte.
        let mut strong = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
            ],
        );
        strong.visible_enemies.push(enemy(910));
        let intents = decide(&strong, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            intents
                .iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. }))
        );
    }

    #[test]
    fn counter_weights_shift_production_to_lights_vs_artillery() {
        use crate::units::archetype;
        // 6H/3L/1A vivi: a pesi puri è triplo pareggio (1.0) e vincerebbe il
        // primo (Heavy); col counter Light-vs-Arty 1.25 tocca ai Light.
        let max_h = archetype(UnitKind::HeavyTank).max_health;
        let max_l = archetype(UnitKind::LightTank).max_health;
        let max_a = archetype(UnitKind::Artillery).max_health;
        let mut kinds: Vec<(UnitKind, f32, UnitOrder)> = Vec::new();
        for _ in 0..6 {
            kinds.push((UnitKind::HeavyTank, max_h, UnitOrder::Idle));
        }
        for _ in 0..3 {
            kinds.push((UnitKind::LightTank, max_l, UnitOrder::Idle));
        }
        kinds.push((UnitKind::Artillery, max_a, UnitOrder::Idle));
        let mut snap = armed_snapshot(1, &kinds);
        for i in 0..3 {
            snap.visible_enemies.push(super::super::snapshot::AiEnemy {
                entity: Entity::from_bits(920 + i),
                pos: Vec3::new(10.0 + i as f32, 0.0, 0.0),
                kind: UnitKind::Artillery,
                health: max_a,
            });
        }
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::LightTank));
    }

    #[test]
    fn focus_fire_prefers_guns_over_screen() {
        use crate::units::archetype;
        // Dominanza + screen di Light davanti all'Arty a pieni hp: la vecchia
        // logica (hp minori) designerebbe il Light, la priorità minaccia
        // (dps × counter vs Heavy primario) designa l'Arty.
        let max = archetype(UnitKind::HeavyTank).max_health;
        let mut snap = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
            ],
        );
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(901),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::LightTank,
            health: archetype(UnitKind::LightTank).max_health,
        });
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(902),
            pos: Vec3::new(12.0, 0.0, 0.0),
            kind: UnitKind::Artillery,
            health: archetype(UnitKind::Artillery).max_health,
        });
        let intents = decide_micro(&snap, &Personality::RUSHER, Scenario::Playground);
        assert_eq!(
            intents.iter().find_map(|i| match i {
                AiIntent::FocusFire { target } => Some(*target),
                _ => None,
            }),
            Some(Entity::from_bits(902))
        );
    }

    #[test]
    fn remembered_power_matches_threat_cell_on_fresh_contact() {
        use super::super::snapshot::AiMemory;
        // Stessa formula nei due moduli (potenza × decay × sconto): un singolo
        // ricordo fresco età 0 deve coincidere col max della threat map.
        let mut snap = empty_snapshot(1);
        snap.memory.push(AiMemory {
            entity_bits: None,
            pos: Vec3::new(50.0, 0.0, -30.0),
            age_ticks: 0,
            kind: Some(UnitKind::HeavyTank),
            hp: 100.0,
            building: false,
        });
        let power = remembered_enemy_power(&snap);
        let map = super::super::threat::build_threat(&snap);
        assert!(power > 0.0);
        assert!((power - map.max()).abs() < 0.001);
    }

    #[test]
    fn force_and_mix_deduplicate_live_memory_by_entity() {
        use super::super::snapshot::{AiEnemy, AiMemory};
        let live = Entity::from_bits(501);
        let remembered = Entity::from_bits(502);
        let mut snap = empty_snapshot(1);
        snap.visible_enemies.push(AiEnemy {
            entity: live,
            pos: Vec3::ZERO,
            kind: UnitKind::HeavyTank,
            health: 170.0,
        });
        snap.memory.extend([
            AiMemory {
                entity_bits: Some(live.to_bits()),
                pos: Vec3::ZERO,
                age_ticks: 0,
                kind: Some(UnitKind::HeavyTank),
                hp: 170.0,
                building: false,
            },
            AiMemory {
                entity_bits: Some(remembered.to_bits()),
                pos: Vec3::X * 20.0,
                age_ticks: 10,
                kind: Some(UnitKind::LightTank),
                hp: 100.0,
                building: false,
            },
        ]);

        let mix = foe_mix(&snap);
        assert_eq!(
            mix.iter().find(|(kind, _)| *kind == UnitKind::HeavyTank),
            Some(&(UnitKind::HeavyTank, 170.0))
        );
        assert_eq!(estimate_forces(&snap).1.len(), 2);
    }

    #[test]
    fn metal_without_free_spot_falls_through_to_solar() {
        // G1: unico deposito occupato dal nemico visibile → niente Metal,
        // la macro passa al Solare invece di intasarsi.
        let mut snap = empty_snapshot(1);
        idle_commander(&mut snap);
        snap.visible_enemy_buildings
            .push(super::super::snapshot::AiBuilding {
                entity: Entity::from_bits(800),
                kind: BuildingKind::Metal,
                pos: home_deposit().pos,
                under_construction: false,
                health: 100.0,
            });
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert_eq!(intents, vec![AiIntent::Build(BuildingKind::Solar)]);
    }

    #[test]
    fn metal_spot_picks_nearest_free_and_skips_occupied() {
        use crate::navigation::{CELL_SIZE, HALF_SIZE, NavGrid};
        use crate::structures::MetalDeposit;
        let grid = NavGrid::new(HALF_SIZE, CELL_SIZE, vec![]);
        let deps = vec![
            MetalDeposit {
                pos: Vec3::new(0.0, 0.0, 0.0),
                mult: 1.0,
            },
            MetalDeposit {
                pos: Vec3::new(100.0, 0.0, 0.0),
                mult: 2.0,
            },
        ];
        let builder = Vec3::new(10.0, 0.0, 0.0);
        // Libero più vicino.
        assert_eq!(
            find_metal_spot(&grid, &deps, &[], builder, &[]),
            Some(Vec3::new(0.0, 0.0, 0.0))
        );
        // Occupato → il lontano.
        assert_eq!(
            find_metal_spot(&grid, &deps, &[Vec3::new(1.0, 0.0, 0.0)], builder, &[]),
            Some(Vec3::new(100.0, 0.0, 0.0))
        );
        // Tutto occupato → niente.
        assert_eq!(
            find_metal_spot(
                &grid,
                &deps,
                &[Vec3::new(0.0, 0.0, 0.0), Vec3::new(100.0, 0.0, 0.0)],
                builder,
                &[]
            ),
            None
        );
    }

    #[test]
    fn labt2_without_income_stays_locked() {
        // 0.0.18 — tick ok ma eco a zero: niente LabT2 (l'eco T1 prima).
        let mut snap = armed_snapshot(1, &[]);
        snap.tick = LABT2_MIN_TICK;
        snap.income = [0.0, 0.0];
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::LabT2)))
        );
    }

    #[test]
    fn planner_picks_bigger_relative_deficit() {
        // Entrambi affamati: metallo 1.2×, energia 2.0× → vince il Solare
        // (la vecchia catena avrebbe preso il Metal per primo).
        let snap = eco_snapshot(1, 2, 1, 1, [10.0, 20.0], [12.0, 40.0]);
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert_eq!(intents, vec![AiIntent::Build(BuildingKind::Solar)]);
    }

    #[test]
    fn turret_spiral_uses_hotspot_anchor() {
        use crate::navigation::{CELL_SIZE, HALF_SIZE, NavGrid};
        // Campo aperto: primo anello a ovest dell'anchor (dentro la mappa),
        // non della base.
        let grid = NavGrid::new(HALF_SIZE, CELL_SIZE, vec![]);
        let base = Scenario::Playground.center(1);
        let anchor = base + Vec3::new(-100.0, 0.0, 0.0);
        let builders = [(crate::units::Team(1), base, 1.0)];
        let spot = find_build_spot(
            &grid,
            1,
            BuildingKind::Turret,
            Scenario::Playground,
            base,
            &[],
            &builders,
            &[],
            Some(anchor),
        );
        assert_eq!(spot, Some(anchor + Vec3::new(10.0, 0.0, 0.0)));
        // Senza anchor: stesso primo anello ma dalla base.
        let spot = find_build_spot(
            &grid,
            1,
            BuildingKind::Turret,
            Scenario::Playground,
            base,
            &[],
            &builders,
            &[],
            None,
        );
        assert_eq!(spot, Some(base + Vec3::new(10.0, 0.0, 0.0)));
    }

    #[test]
    fn wall_slots_form_ahead_screen() {
        // Torretta in (0,0), minaccia da +x: 3 slot a x=10, simmetrici in z.
        let slots = wall_slots(Vec3::ZERO, Vec3::X);
        assert_eq!(slots.len(), 3);
        for s in &slots {
            assert!((s.x - 10.0).abs() < 0.001, "{s:?}");
        }
        let mut zs: Vec<f32> = slots.iter().map(|s| s.z).collect();
        zs.sort_by(f32::total_cmp);
        assert_eq!(zs, vec![-2.0, 0.0, 2.0]);
        // Direzione nulla: fallback +x, mai NaN.
        let slots = wall_slots(Vec3::ZERO, Vec3::ZERO);
        assert!(slots.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn walls_need_a_turret_and_respect_cap() {
        use super::super::snapshot::AiBuilding;
        // Turtle + torretta completa: chiede il muro.
        let mut snap = armed_snapshot(1, &[]);
        snap.my_buildings.push(AiBuilding {
            entity: Entity::from_bits(700),
            kind: BuildingKind::Turret,
            pos: Vec3::ZERO,
            under_construction: false,
            health: 100.0,
        });
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::Wall)));
        // Al cap (3 muri): basta.
        for i in 0..3 {
            snap.my_buildings.push(AiBuilding {
                entity: Entity::from_bits(710 + i),
                kind: BuildingKind::Wall,
                pos: Vec3::new(20.0 + i as f32 * 4.0, 0.0, 0.0),
                under_construction: false,
                health: 100.0,
            });
        }
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Wall)))
        );
        // Rusher non mura mai.
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Wall)))
        );
    }

    #[test]
    fn wall_spot_needs_turret_leaves_factory_doors_open() {
        use crate::navigation::{CELL_SIZE, HALF_SIZE, NavGrid};
        use crate::units::Team;
        let grid = NavGrid::new(HALF_SIZE, CELL_SIZE, vec![]);
        let turret = Vec3::new(0.0, 0.0, 0.0);
        let mine = Team(1);
        let complete = false; // false = completa (no Construction)
        let buildings = vec![(mine, BuildingKind::Turret, turret, complete)];
        // Campo aperto: primo slot valido davanti (verso +x).
        let spot = find_wall_spot(
            &grid,
            1,
            &buildings,
            Vec3::new(0.0, 0.0, -10.0),
            &[],
            Vec3::new(100.0, 0.0, 0.0),
            Vec3::new(-260.0, 0.0, -260.0),
        );
        assert_eq!(spot, Some(Vec3::new(10.0, 0.0, -2.0)));
        // Senza torrette: niente muri mirati.
        assert_eq!(
            find_wall_spot(
                &grid,
                1,
                &[],
                Vec3::new(0.0, 0.0, -10.0),
                &[],
                Vec3::new(100.0, 0.0, 0.0),
                Vec3::new(-260.0, 0.0, -260.0),
            ),
            None
        );
        // Slot occupati da unità: niente (valid_ground li rifiuta tutti).
        let bodies = vec![
            (Vec3::new(10.0, 0.0, -2.0), 0.5),
            (Vec3::new(10.0, 0.0, 0.0), 0.5),
            (Vec3::new(10.0, 0.0, 2.0), 0.5),
        ];
        assert_eq!(
            find_wall_spot(
                &grid,
                1,
                &buildings,
                Vec3::new(0.0, 0.0, -10.0),
                &bodies,
                Vec3::new(100.0, 0.0, 0.0),
                Vec3::new(-260.0, 0.0, -260.0),
            ),
            None
        );
    }

    #[test]
    fn scouting_uses_frontier_and_memory() {
        use super::super::snapshot::AiMemory;
        // 0.0.19 — frontiera deterministica: a parità di esplorato la meta non
        // balla più col tick (il ciclo su 4 punti fissi mandava in roccia).
        let mut snap = armed_snapshot(1, &[(UnitKind::Scout, 60.0, UnitOrder::Idle)]);
        snap.tick = 0;
        let early = scout_destination(&snap, Scenario::Playground);
        snap.tick = 8;
        let later = scout_destination(&snap, Scenario::Playground);
        assert_eq!(early, later);
        // Frontiera diversa se l'esplorato cambia: cella vista esclusa.
        snap.explored_cells = vec![
            false;
            super::super::snapshot::EXPLORED_GRID_N
                * super::super::snapshot::EXPLORED_GRID_N
        ];
        let blind = scout_destination(&snap, Scenario::Playground);
        // Marca la cella della meta come vista → la meta si sposta.
        {
            use crate::navigation::HALF_SIZE;
            let n = super::super::snapshot::EXPLORED_GRID_N;
            let side = (HALF_SIZE * 2.0) / n as f32;
            let col = (((blind.x + HALF_SIZE) / side).floor() as usize).min(n - 1);
            let row = (((blind.z + HALF_SIZE) / side).floor() as usize).min(n - 1);
            snap.explored_cells[row * n + col] = true;
        }
        let moved = scout_destination(&snap, Scenario::Playground);
        assert_ne!(blind, moved);
        // Ricordo fresco: lo scout va a confermare lì (come 0.0.14).
        snap.memory.push(AiMemory {
            entity_bits: None,
            pos: Vec3::new(50.0, 0.0, -30.0),
            age_ticks: 10,
            kind: Some(UnitKind::HeavyTank),
            hp: 100.0,
            building: false,
        });
        assert_eq!(
            scout_destination(&snap, Scenario::Playground),
            Vec3::new(50.0, 0.0, -30.0)
        );
        // Attacco su ricordi-truppa quando la vista è vuota.
        assert_eq!(
            attack_destination(&snap, Scenario::Playground),
            Vec3::new(50.0, 0.0, -30.0)
        );
    }

    #[test]
    fn building_memory_redirects_attack() {
        use super::super::snapshot::AiMemory;
        // 0.0.19 — base nemica ricordata (edifici freschi, vista vuota) batte
        // il target statico: marcia su posizioni verificate.
        let mut snap = armed_snapshot(1, &[]);
        let static_target = Scenario::Playground.attack_target(1);
        // Senza memoria: target statico.
        assert_eq!(
            attack_destination(&snap, Scenario::Playground),
            static_target
        );
        // Con edificio ricordato fresco altrove: la meta si sposta lì.
        let base_seen = Vec3::new(200.0, 0.0, 180.0);
        assert!((base_seen - static_target).length() > 50.0);
        snap.memory.push(AiMemory {
            entity_bits: None,
            pos: base_seen,
            age_ticks: 10,
            kind: None,
            hp: 450.0,
            building: true,
        });
        assert_eq!(attack_destination(&snap, Scenario::Playground), base_seen);
        // Ricordo stantio oltre il fresh: si torna allo statico.
        snap.memory[0].age_ticks = MEMORY_FRESH_TICKS + 1;
        assert_eq!(
            attack_destination(&snap, Scenario::Playground),
            static_target
        );
    }

    #[test]
    fn scout_intent_is_free_from_army_count() {
        use crate::units::archetype;
        // 0.0.19 — fix ramo morto: lo scout esce anche con l'esercito vivo
        // (prima `army_count == 0` impossibile col Commander armato).
        let max = archetype(UnitKind::HeavyTank).max_health;
        let snap = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::Scout, 60.0, UnitOrder::Idle),
            ],
        );
        assert!(!has_fresh_eyes(&snap));
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            intents.iter().any(|i| matches!(i, AiIntent::Scout { .. })),
            "scout cieco con armata viva deve uscire: {intents:?}"
        );
    }

    #[test]
    fn scouts_patrol_distinct_frontiers_with_eyes_on() {
        use crate::units::archetype;
        // 0.0.19 — pattuglia continua: con occhi aperti ma scout liberi, gli
        // scout mappano (frontiere distinte) invece di marcire in base; non
        // si immolano nella battaglia live (meta ≠ nemico visibile).
        let max = archetype(UnitKind::HeavyTank).max_health;
        let mut snap = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::Scout, 60.0, UnitOrder::Idle),
                (UnitKind::Scout, 60.0, UnitOrder::Idle),
            ],
        );
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max,
        });
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        let scouts: Vec<Vec3> = intents
            .iter()
            .filter_map(|i| match i {
                AiIntent::Scout { destination } => Some(*destination),
                _ => None,
            })
            .collect();
        assert_eq!(scouts.len(), 2, "due scout liberi = due mete: {intents:?}");
        assert!(
            scouts[0].distance(scouts[1]) > 9.0,
            "frontiere distinte, mai in pila: {scouts:?}"
        );
        for dest in &scouts {
            assert!(
                dest.distance(Vec3::new(10.0, 0.0, 0.0)) > 9.0,
                "niente immolazione sul live: {dest:?}"
            );
        }
    }

    #[test]
    fn scout_diverts_from_hot_route() {
        use crate::units::archetype;
        // 0.0.19 — scout in rotta verso una meta diventata calda (tank
        // avvistato lì dopo l'ordine): devia invece di immolarsi. Rotta
        // fredda: la finisce, nessun intento.
        let max = archetype(UnitKind::HeavyTank).max_health;
        let hot = Vec3::new(100.0, 0.0, 100.0);
        let mut snap = armed_snapshot(
            1,
            &[(UnitKind::Scout, 60.0, UnitOrder::Move { destination: hot })],
        );
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: hot,
            kind: UnitKind::HeavyTank,
            health: max,
        });
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            intents.iter().any(|i| matches!(i, AiIntent::Scout { .. })),
            "rotta calda: devia {intents:?}"
        );
        // Stesso nemico, rotta fredda altrove: nessun re-task.
        let mut cool = armed_snapshot(
            1,
            &[(
                UnitKind::Scout,
                60.0,
                UnitOrder::Move {
                    destination: Vec3::new(-100.0, 0.0, -100.0),
                },
            )],
        );
        cool.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: hot,
            kind: UnitKind::HeavyTank,
            health: max,
        });
        let intents = decide(&cool, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            !intents.iter().any(|i| matches!(i, AiIntent::Scout { .. })),
            "rotta fredda: la finisce {intents:?}"
        );
    }

    #[test]
    fn opponent_shift_moves_courage_and_mix() {
        use super::super::snapshot::{AiBuilding, AiMemory};
        use crate::units::archetype;
        // Vs rusher (massa precoce): courage effettivo scende di 0.1 e il mix
        // sposta verso Heavy/Arty (bias 1.1) contro Light (0.9).
        let max_h = archetype(UnitKind::HeavyTank).max_health;
        let mut snap = armed_snapshot(1, &[(UnitKind::Scout, 60.0, UnitOrder::Idle)]);
        snap.tick = 200;
        for i in 0..3 {
            snap.memory.push(AiMemory {
                entity_bits: None,
                pos: Vec3::new(i as f32 * 5.0, 0.0, 0.0),
                age_ticks: 5,
                kind: Some(UnitKind::HeavyTank),
                hp: max_h,
                building: false,
            });
        }
        snap.memory.push(AiMemory {
            entity_bits: None,
            pos: Vec3::ZERO,
            age_ticks: 150,
            kind: Some(UnitKind::HeavyTank),
            hp: max_h,
            building: false,
        });
        assert_eq!(
            super::super::opponent::classify(&snap),
            super::super::opponent::OpponentKind::Rusher
        );
        assert!(
            (super::super::opponent::adjust_courage(
                0.55,
                super::super::opponent::OpponentKind::Rusher
            ) - 0.45)
                .abs()
                < 1e-6
        );
        // Vs turtle (torretta vista): courage sale e Arty bias 1.1.
        snap.visible_enemy_buildings.push(AiBuilding {
            entity: Entity::from_bits(700),
            kind: BuildingKind::Turret,
            pos: Vec3::new(200.0, 0.0, 200.0),
            under_construction: false,
            health: 450.0,
        });
        assert_eq!(
            super::super::opponent::classify(&snap),
            super::super::opponent::OpponentKind::Turtle
        );
        assert!(
            super::super::opponent::mix_bias(
                UnitKind::Artillery,
                super::super::opponent::OpponentKind::Turtle
            ) > super::super::opponent::mix_bias(
                UnitKind::LightTank,
                super::super::opponent::OpponentKind::Turtle
            )
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
            &[fac(1, 0, false, 1, &[])],
            0,
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
            &[fac(1, 0, true, 1, &[])],
            0,
        );
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Enqueue { .. }))
        );
    }

    fn idle_commander(snap: &mut AiSnapshot) {
        use crate::units::archetype;
        let max_c = archetype(UnitKind::Commander).max_health;
        snap.my_units.push(super::super::snapshot::AiUnit {
            entity: Entity::from_bits(99),
            pos: Vec3::ZERO,
            kind: UnitKind::Commander,
            order: UnitOrder::Idle,
            health: max_c,
            max_health: max_c,
        });
    }

    fn idle_engineer(snap: &mut AiSnapshot) {
        use crate::units::archetype;
        let max_e = archetype(UnitKind::Engineer).max_health;
        snap.my_units.push(super::super::snapshot::AiUnit {
            entity: Entity::from_bits(98),
            pos: Vec3::ZERO,
            kind: UnitKind::Engineer,
            order: UnitOrder::Idle,
            health: max_e,
            max_health: max_e,
        });
    }

    fn eco_snapshot(
        team: u8,
        metals: usize,
        solars: usize,
        factories: usize,
        income: [f64; 2],
        demand: [f64; 2],
    ) -> AiSnapshot {
        let mut snap = empty_snapshot(team);
        let mut bits = 500u64;
        for (kind, n) in [
            (BuildingKind::Metal, metals),
            (BuildingKind::Solar, solars),
            (BuildingKind::Factory, factories),
        ] {
            for _ in 0..n {
                snap.my_buildings.push(super::super::snapshot::AiBuilding {
                    entity: Entity::from_bits(bits),
                    kind,
                    pos: Vec3::ZERO,
                    under_construction: false,
                    health: 100.0,
                });
                bits += 1;
            }
        }
        snap.income = income;
        snap.demand = demand;
        // Un Commander libero: gli intenti Build scalano sui builder liberi.
        let max_c = crate::units::archetype(UnitKind::Commander).max_health;
        snap.my_units.push(super::super::snapshot::AiUnit {
            entity: Entity::from_bits(99),
            pos: Vec3::ZERO,
            kind: UnitKind::Commander,
            order: UnitOrder::Idle,
            health: max_c,
            max_health: max_c,
        });
        snap
    }

    fn give_fresh_eyes(snap: &mut AiSnapshot) {
        use super::super::snapshot::AiMemory;
        snap.memory.push(AiMemory {
            entity_bits: None,
            pos: Vec3::new(50.0, 0.0, -30.0),
            age_ticks: 5,
            kind: Some(UnitKind::HeavyTank),
            hp: 100.0,
            building: false,
        });
    }

    #[test]
    fn starved_metal_requests_metal_until_cap() {
        // Turtle affamata di metal (domanda > offerta): nuovo Metal.
        let snap = eco_snapshot(1, 1, 2, 1, [5.0, 24.0], [12.0, 10.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::Metal)));
        // Al cap (3): basta, anche se affamata.
        let snap = eco_snapshot(1, 3, 2, 1, [15.0, 24.0], [30.0, 10.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Metal)))
        );
        // Domanda soddisfatta: nessun nuovo Metal.
        let snap = eco_snapshot(1, 1, 2, 1, [5.0, 24.0], [4.0, 10.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Metal)))
        );
    }

    #[test]
    fn starved_energy_requests_solar_until_cap() {
        let snap = eco_snapshot(1, 2, 1, 1, [10.0, 12.0], [5.0, 30.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::Solar)));
        // Rusher al suo cap (2): basta.
        let snap = eco_snapshot(1, 2, 2, 1, [10.0, 24.0], [5.0, 60.0]);
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Solar)))
        );
    }

    #[test]
    fn planner_waits_when_broke_e2e() {
        // Debito Punto 5 — collo di bottiglia ma non abbordabile entro 60s
        // (income 0.5/s su costo 100: tta 200s = INF): niente Build, aspetta eco.
        // Stessa fame con income sano (5/s: tta 20s) costruisce.
        let broke = eco_snapshot(1, 1, 2, 1, [0.5, 24.0], [12.0, 10.0]);
        let intents = decide(&broke, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Metal))),
            "broke: aspetta eco {intents:?}"
        );
        let funded = eco_snapshot(1, 1, 2, 1, [5.0, 24.0], [12.0, 10.0]);
        let intents = decide(&funded, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::Metal)));
    }

    #[test]
    fn second_factory_needs_two_metals() {
        // Eco bilanciata ma un solo Metal: niente seconda lab.
        let snap = eco_snapshot(1, 1, 2, 1, [5.0, 24.0], [5.0, 10.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Factory)))
        );
        // Due Metal: via alla seconda (sotto il cap turtle di 2).
        let snap = eco_snapshot(1, 2, 2, 1, [10.0, 24.0], [9.0, 10.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::Factory)));
        // Rusher resta a una sola lab per scelta.
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Factory)))
        );
    }

    #[test]
    fn engineer_and_scout_queues_count_toward_caps() {
        // Turtle: 1 engineer vivo + 1 accodato = cap raggiunto → tech reattivo
        // (ricordo fresco di Heavy: mortaio, non Heavy).
        let mut snap = armed_snapshot(1, &[(UnitKind::Engineer, 70.0, UnitOrder::Idle)]);
        give_fresh_eyes(&mut snap);
        let intents = decide(
            &snap,
            &Personality::TURTLE,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[UnitKind::Engineer])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::MortarTank));
        // Scout accodato ma non ancora uscito: niente doppione (stesso tech).
        let mut snap = armed_snapshot(1, &[]);
        give_fresh_eyes(&mut snap);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[UnitKind::Scout])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::MortarTank));
    }

    #[test]
    fn one_enqueue_per_free_factory() {
        // Due lab libere: due enqueue complementari (Heavy poi Light).
        let mut snap = armed_snapshot(1, &[(UnitKind::Scout, 60.0, UnitOrder::Idle)]);
        give_fresh_eyes(&mut snap);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[]), fac(2, 0, false, 1, &[])],
            0,
        );
        let mut kinds: Vec<UnitKind> = intents
            .iter()
            .filter_map(|i| match i {
                AiIntent::Enqueue { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect();
        kinds.sort_by_key(|k| k.index());
        assert_eq!(kinds.len(), 2);
        let targets: Vec<Entity> = intents
            .iter()
            .filter_map(|i| match i {
                AiIntent::Enqueue { factory, .. } => Some(*factory),
                _ => None,
            })
            .collect();
        assert_ne!(targets[0], targets[1]);
    }

    #[test]
    fn eco_only_never_attacks_or_micros() {
        use crate::units::archetype;
        // Eco completa + esercito + nemico visibile: eco-only resta a casa.
        let mut snap = eco_snapshot(1, 3, 3, 2, [15.0, 36.0], [10.0, 10.0]);
        for i in 0..3 {
            snap.my_units.push(super::super::snapshot::AiUnit {
                entity: Entity::from_bits(100 + i),
                pos: Vec3::ZERO,
                kind: UnitKind::HeavyTank,
                order: UnitOrder::Idle,
                health: archetype(UnitKind::HeavyTank).max_health,
                max_health: archetype(UnitKind::HeavyTank).max_health,
            });
        }
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: archetype(UnitKind::HeavyTank).max_health,
        });
        let intents = decide(&snap, &Personality::ECO_ONLY, Scenario::Playground, &[], 0);
        assert!(!intents.iter().any(|i| matches!(
            i,
            AiIntent::AttackMoveGroup { .. } | AiIntent::FocusFire { .. }
        )));
    }

    #[test]
    fn rush_scripted_attacks_on_schedule_regardless_of_odds() {
        use crate::units::archetype;
        // Un solo heavy contro forza superiore: prima dello schedule, niente.
        let mut snap = eco_snapshot(1, 2, 1, 1, [10.0, 12.0], [9.0, 30.0]);
        // Builder inerme al posto del Commander: serve un builder libero per
        // gli intenti ma senza distorcere la stima forze (1200hp).
        snap.my_units.retain(|u| u.kind != UnitKind::Commander);
        let max_e = archetype(UnitKind::Engineer).max_health;
        snap.my_units.push(super::super::snapshot::AiUnit {
            entity: Entity::from_bits(98),
            pos: Vec3::ZERO,
            kind: UnitKind::Engineer,
            order: UnitOrder::Idle,
            health: max_e,
            max_health: max_e,
        });
        snap.my_units.push(super::super::snapshot::AiUnit {
            entity: Entity::from_bits(100),
            pos: Vec3::ZERO,
            kind: UnitKind::HeavyTank,
            order: UnitOrder::Idle,
            health: archetype(UnitKind::HeavyTank).max_health,
            max_health: archetype(UnitKind::HeavyTank).max_health,
        });
        for i in 0..4 {
            snap.visible_enemies.push(super::super::snapshot::AiEnemy {
                entity: Entity::from_bits(900 + i),
                pos: Vec3::new(10.0 + i as f32, 0.0, 0.0),
                kind: UnitKind::HeavyTank,
                health: archetype(UnitKind::HeavyTank).max_health,
            });
        }
        snap.tick = 0;
        let early = decide(
            &snap,
            &Personality::RUSH_SCRIPTED,
            Scenario::Playground,
            &[],
            0,
        );
        assert!(
            !early
                .iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. }))
        );
        // ...allo schedule: l'ondata parte comunque.
        snap.tick = Personality::RUSH_SCRIPTED.attack_at_tick;
        let late = decide(
            &snap,
            &Personality::RUSH_SCRIPTED,
            Scenario::Playground,
            &[],
            0,
        );
        assert!(
            late.iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. }))
        );
    }

    #[test]
    fn waves_tick_alignment() {
        // Il periodo è multiplo della cadenza strategia/snapshot (1Hz vede
        // ogni quarto tick a 4Hz): i lanci non si perdono mai.
        assert_eq!(WAVE_PERIOD_TICKS % 4, 0);
        assert!(is_wave_tick(0) && is_wave_tick(300) && is_wave_tick(600));
        assert!(!is_wave_tick(1) && !is_wave_tick(299) && !is_wave_tick(301));
        assert_eq!(wave_id(0), 0);
        assert_eq!(wave_id(299), 0);
        assert_eq!(wave_id(300), 1);
    }

    #[test]
    fn waves_monotonic() {
        use crate::units::archetype;
        // Stessa forza dominante: l'ondata parte ai tick d'ondata, mai fuori
        // (impulsi discreti ogni ~75s, rivalutati ogni volta).
        let max = archetype(UnitKind::HeavyTank).max_health;
        let mut snap = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
            ],
        );
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max,
        });
        // Come `ai_tick`: last evolve solo quando l'ondata parte davvero.
        let mut last = 0u64;
        let mut has_group = |tick: u64| {
            let mut snap = snap.clone();
            snap.tick = tick;
            let fire = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], last)
                .iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. }));
            if fire {
                last = wave_id(tick);
            }
            fire
        };
        assert!(has_group(300));
        assert!(!has_group(301));
        assert!(!has_group(450));
        assert!(has_group(600));
        // Monotonia degli id d'ondata.
        assert!(wave_id(300) < wave_id(600));
    }

    #[test]
    fn wave_recalls_when_base_threatened() {
        use crate::units::archetype;
        // Nemico in casa (entro 120m da (260, 260)): a parità niente ondata,
        // il micro scherma a casa (richiamo difensivo).
        let max = archetype(UnitKind::HeavyTank).max_health;
        let raider = || super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(230.0, 0.0, 230.0),
            kind: UnitKind::HeavyTank,
            health: max,
        };
        let mut even = armed_snapshot(1, &[(UnitKind::HeavyTank, max, UnitOrder::Idle)]);
        even.visible_enemies.push(raider());
        assert!(base_under_threat(&even, Scenario::Playground));
        even.tick = 300;
        let intents = decide(&even, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. })),
            "a parità minacciati l'ondata resta a casa: {intents:?}"
        );
        // Il micro invece scherma: linea a casa.
        let micro = decide_micro(&even, &Personality::RUSHER, Scenario::Playground);
        assert!(
            micro.iter().any(|i| matches!(i, AiIntent::Screen { .. })),
            "schermo a casa: {micro:?}"
        );
    }

    #[test]
    fn dominant_counter_pushes_onto_threat_at_home() {
        use crate::units::archetype;
        // Punto 2 — stessa minaccia in casa, ma dominanti 4v1: il richiamo non
        // è più un muro, si contrattacca SULLA minaccia (centroide visibile),
        // senza capitale (qui assente comunque).
        let max = archetype(UnitKind::HeavyTank).max_health;
        let mut snap = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
            ],
        );
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(230.0, 0.0, 230.0),
            kind: UnitKind::HeavyTank,
            health: max,
        });
        assert!(base_under_threat(&snap, Scenario::Playground));
        snap.tick = 300;
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        let dest = intents.iter().find_map(|i| match i {
            AiIntent::AttackMoveGroup { destination, .. } => Some(*destination),
            _ => None,
        });
        let dest = dest.expect("dominanti: contrattacco sulla minaccia");
        assert!(
            (dest.x - 230.0).abs() < 0.01 && (dest.z - 230.0).abs() < 0.01,
            "meta sul raider, non sulla base nemica: {dest:?}"
        );
    }

    #[test]
    fn commander_never_commits_early() {
        use crate::units::archetype;
        // Capitale + scorta con vittoria probabile ma non certa (0.6 < 0.9):
        // l'ondata parte senza Commander. A 0.99 il Commander si unisce, ma
        // mai da solo (scorta presente comunque).
        let max_h = archetype(UnitKind::HeavyTank).max_health;
        let max_c = archetype(UnitKind::Commander).max_health;
        let escort = |snap: &mut AiSnapshot| {
            snap.my_units.push(super::super::snapshot::AiUnit {
                entity: Entity::from_bits(101),
                pos: Vec3::ZERO,
                kind: UnitKind::HeavyTank,
                order: UnitOrder::Idle,
                health: max_h,
                max_health: max_h,
            });
            snap.my_units.push(super::super::snapshot::AiUnit {
                entity: Entity::from_bits(102),
                pos: Vec3::ZERO,
                kind: UnitKind::HeavyTank,
                order: UnitOrder::Idle,
                health: max_h,
                max_health: max_h,
            });
        };
        // wave_group diretto: soglia commit rusher 0.9, base libera.
        let mut snap = armed_snapshot(1, &[(UnitKind::Commander, max_c, UnitOrder::Idle)]);
        escort(&mut snap);
        let cautious = wave_group(&snap, &Personality::RUSHER, 0.6, false);
        assert_eq!(cautious.len(), 2);
        assert!(!cautious.contains(&Entity::from_bits(100)));
        let committed = wave_group(&snap, &Personality::RUSHER, 0.99, false);
        assert_eq!(committed.len(), 3);
        assert!(committed.contains(&Entity::from_bits(100)));
        // Base minacciata: il capitale non si gioca nelle mischie difensive,
        // anche oltre soglia (il torneo lo perdeva proprio lì).
        let defended = wave_group(&snap, &Personality::RUSHER, 1.0, true);
        assert_eq!(defended.len(), 2);
        assert!(!defended.contains(&Entity::from_bits(100)));
        // Solo capitale (niente scorta): mai ondata solitaria.
        let alone = armed_snapshot(1, &[(UnitKind::Commander, max_c, UnitOrder::Idle)]);
        assert!(wave_group(&alone, &Personality::RUSHER, 1.0, false).is_empty());
    }

    #[test]
    fn arty_holds_behind_screen() {
        use crate::units::archetype;
        // Linea + batteria ferme a contatto live: schermo davanti, arty in
        // gittata dietro (hold più lontano dalla minaccia dello screen).
        let max_h = archetype(UnitKind::HeavyTank).max_health;
        let max_a = archetype(UnitKind::Artillery).max_health;
        let mut snap = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max_h, UnitOrder::Idle),
                (UnitKind::Artillery, max_a, UnitOrder::Idle),
            ],
        );
        let foe = Vec3::new(100.0, 0.0, 0.0);
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: foe,
            kind: UnitKind::HeavyTank,
            health: max_h,
        });
        let micro = decide_micro(&snap, &Personality::RUSHER, Scenario::Playground);
        let screen_pos = micro.iter().find_map(|i| match i {
            AiIntent::Screen { position, .. } => Some(*position),
            _ => None,
        });
        let hold_pos = micro.iter().find_map(|i| match i {
            AiIntent::HoldAtMaxRange { position, .. } => Some(*position),
            _ => None,
        });
        let (screen_pos, hold_pos) = (screen_pos.expect("screen"), hold_pos.expect("hold"));
        // La batteria tiene la gittata con margine (~26m per 30m nominali),
        // lo schermo sta davanti alla batteria senza immolarsi sul nemico.
        assert!((hold_pos.distance(foe) - 26.0).abs() < 6.0);
        assert!(screen_pos.distance(foe) < hold_pos.distance(foe));
        assert!(screen_pos.distance(foe) > 5.0);
        // Arty ferita (<50%): niente hold, va in ritirata.
        let mut hurt = armed_snapshot(1, &[(UnitKind::Artillery, max_a * 0.4, UnitOrder::Idle)]);
        hurt.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(901),
            pos: foe,
            kind: UnitKind::HeavyTank,
            health: max_h,
        });
        let micro = decide_micro(&hurt, &Personality::RUSHER, Scenario::Playground);
        assert!(
            !micro
                .iter()
                .any(|i| matches!(i, AiIntent::HoldAtMaxRange { .. })),
            "arty ferita non tiene: {micro:?}"
        );
        assert!(
            micro.iter().any(|i| matches!(i, AiIntent::Retreat { .. })),
            "arty ferita ripiega: {micro:?}"
        );
    }

    #[test]
    fn kite_backoff_inside_line_hold_outside() {
        use crate::units::archetype;
        // Arty sola (30m) + heavy a 15m (< 22.5 linea): kita a ~26m dal
        // nemico lungo la direttrice, e niente Hold per lei.
        let max_a = archetype(UnitKind::Artillery).max_health;
        let max_h = archetype(UnitKind::HeavyTank).max_health;
        let foe = Vec3::new(15.0, 0.0, 0.0);
        let mut snap = armed_snapshot(1, &[(UnitKind::Artillery, max_a, UnitOrder::Idle)]);
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: foe,
            kind: UnitKind::HeavyTank,
            health: max_h,
        });
        let micro = decide_micro(&snap, &Personality::RUSHER, Scenario::Playground);
        let dest = micro.iter().find_map(|i| match i {
            AiIntent::Kite { moves } => Some(moves.clone()),
            _ => None,
        });
        let moves = dest.expect("kite quando pressata");
        assert_eq!(moves.len(), 1);
        let have = moves[0].1;
        // Meta di arretramento: ~26m dal nemico, più lontana di prima.
        assert!((have.distance(foe) - 26.0).abs() < 1.0, "{have:?}");
        assert!(have.distance(foe) > Vec3::ZERO.distance(foe));
        assert!(
            !micro
                .iter()
                .any(|i| matches!(i, AiIntent::HoldAtMaxRange { .. })),
            "chi kita non tiene: {micro:?}"
        );
        // Nemico a 28m (> linea): niente kite, Hold normale.
        let mut far = armed_snapshot(1, &[(UnitKind::Artillery, max_a, UnitOrder::Idle)]);
        far.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(901),
            pos: Vec3::new(28.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max_h,
        });
        let micro = decide_micro(&far, &Personality::RUSHER, Scenario::Playground);
        assert!(
            !micro.iter().any(|i| matches!(i, AiIntent::Kite { .. })),
            "fuori linea tiene: {micro:?}"
        );
        assert!(
            micro
                .iter()
                .any(|i| matches!(i, AiIntent::HoldAtMaxRange { .. })),
            "hold fuori linea: {micro:?}"
        );
    }

    #[test]
    fn snipe_designates_commander_without_suicide() {
        use crate::units::archetype;
        // Selezione pura: il Commander (hp minori) vince su chiunque.
        let max_h = archetype(UnitKind::HeavyTank).max_health;
        let mut snap = armed_snapshot(1, &[(UnitKind::HeavyTank, max_h, UnitOrder::Idle)]);
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(901),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max_h,
        });
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(902),
            pos: Vec3::new(12.0, 0.0, 0.0),
            kind: UnitKind::Commander,
            health: 150.0,
        });
        assert_eq!(snipe_target(&snap), Some(Entity::from_bits(902)));
        assert!(snipe_target(&armed_snapshot(1, &[])).is_none());
        // Dominanti 4v1 + Commander esposto: designato (snipe batte focus).
        let mut strong = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max_h, UnitOrder::Idle),
                (UnitKind::HeavyTank, max_h, UnitOrder::Idle),
                (UnitKind::HeavyTank, max_h, UnitOrder::Idle),
                (UnitKind::HeavyTank, max_h, UnitOrder::Idle),
            ],
        );
        strong
            .visible_enemies
            .push(super::super::snapshot::AiEnemy {
                entity: Entity::from_bits(903),
                pos: Vec3::new(10.0, 0.0, 0.0),
                kind: UnitKind::Commander,
                health: 150.0,
            });
        let micro = decide_micro(&strong, &Personality::RUSHER, Scenario::Playground);
        assert_eq!(
            micro.iter().find_map(|i| match i {
                AiIntent::FocusFire { target } => Some(*target),
                _ => None,
            }),
            Some(Entity::from_bits(903))
        );
        // Debole 1v1+Commander full: silenzio, niente suicidi di prestigio.
        let mut weak = armed_snapshot(1, &[(UnitKind::HeavyTank, max_h, UnitOrder::Idle)]);
        weak.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(904),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max_h,
        });
        weak.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(905),
            pos: Vec3::new(12.0, 0.0, 0.0),
            kind: UnitKind::Commander,
            health: archetype(UnitKind::Commander).max_health,
        });
        let micro = decide_micro(&weak, &Personality::RUSHER, Scenario::Playground);
        assert!(
            !micro
                .iter()
                .any(|i| matches!(i, AiIntent::FocusFire { .. })),
            "debole non snipa: {micro:?}"
        );
    }

    #[test]
    fn retreat_to_turret() {
        // Ancora = torretta completa più vicina a casa; senza torrette, casa.
        let home = Vec3::new(-260.0, 0.0, -260.0);
        assert_eq!(retreat_anchor(&[], home), home);
        let near = Vec3::new(-240.0, 0.0, -240.0);
        let far = Vec3::new(0.0, 0.0, 0.0);
        assert_eq!(retreat_anchor(&[far, near], home), near);
        assert_eq!(retreat_anchor(&[near, far], home), near);
    }

    #[test]
    fn engagement_range_prefers_live_then_memory_then_melee() {
        use crate::units::archetype;
        let max = archetype(UnitKind::HeavyTank).max_health;
        // Armata a ZERO, nemico live a 30m: 30.
        let mut snap = armed_snapshot(1, &[(UnitKind::HeavyTank, max, UnitOrder::Idle)]);
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(30.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max,
        });
        assert!((engagement_range(&snap) - 30.0).abs() < 0.01);
        // Solo ricordo fresco a 40m (niente live): 40.
        let mut ghost = armed_snapshot(1, &[(UnitKind::HeavyTank, max, UnitOrder::Idle)]);
        ghost.memory.push(super::super::snapshot::AiMemory {
            entity_bits: None,
            pos: Vec3::new(40.0, 0.0, 0.0),
            age_ticks: 5,
            kind: Some(UnitKind::HeavyTank),
            hp: max,
            building: false,
        });
        assert!((engagement_range(&ghost) - 40.0).abs() < 0.01);
        // Buio totale o nessuna armata: mischia (vecchia matematica).
        let dark = armed_snapshot(1, &[(UnitKind::HeavyTank, max, UnitOrder::Idle)]);
        assert_eq!(engagement_range(&dark), 0.0);
        let empty = armed_snapshot(1, &[]);
        assert_eq!(engagement_range(&empty), 0.0);
    }

    #[test]
    fn battery_excludes_scout_commander_explicitly() {
        use crate::units::archetype;
        // Anche se Scout/Commander cambiassero gittata in tabella, il filtro
        // batteria non deve mai arruolarli: sono occhi/capitale, mai batteria.
        let max_s = archetype(UnitKind::Scout).max_health;
        let max_c = archetype(UnitKind::Commander).max_health;
        let max_a = archetype(UnitKind::Artillery).max_health;
        let mut snap = armed_snapshot(
            1,
            &[
                (UnitKind::Scout, max_s, UnitOrder::Idle),
                (UnitKind::Commander, max_c, UnitOrder::Idle),
                (UnitKind::Artillery, max_a, UnitOrder::Idle),
            ],
        );
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(100.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: archetype(UnitKind::HeavyTank).max_health,
        });
        let micro = decide_micro(&snap, &Personality::RUSHER, Scenario::Playground);
        let hold_units: Vec<Entity> = micro
            .iter()
            .filter_map(|i| match i {
                AiIntent::HoldAtMaxRange { units, .. } => Some(units.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        // Solo l'arty in batteria (bit 102 se ordine snapshot stabile).
        assert_eq!(hold_units.len(), 1);
        let arty_entity = snap
            .my_units
            .iter()
            .find(|u| u.kind == UnitKind::Artillery)
            .unwrap()
            .entity;
        assert!(hold_units.contains(&arty_entity));
    }

    #[test]
    fn hold_holds_ground_when_already_in_range() {
        // Ancora già dentro (range-4): tiene il terreno, niente avanzate.
        let threat = Vec3::new(100.0, 0.0, 0.0);
        let anchor_close = Vec3::new(110.0, 0.0, 0.0); // dist 10 <= 26
        assert_eq!(
            hold_position(anchor_close, threat, 30.0),
            anchor_close.with_y(0.0)
        );
        // Fuori gittata: avanza a range-4 sul lato armata.
        let anchor_far = Vec3::ZERO; // dist 100 > 26
        let hold = hold_position(anchor_far, threat, 30.0);
        assert!((hold.distance(threat) - 26.0).abs() < 0.01);
    }

    #[test]
    fn from_name_knows_baselines() {
        assert_eq!(Personality::from_name("eco-only"), Personality::ECO_ONLY);
        assert_eq!(
            Personality::from_name("rush-scripted"),
            Personality::RUSH_SCRIPTED
        );
        assert_eq!(Personality::from_name("rusher"), Personality::RUSHER);
        assert_eq!(Personality::from_name("???"), Personality::TURTLE);
        // 0.0.21 — strict: ignoti sono errore, mai fallback silenzioso.
        assert!(Personality::try_from_name("gandalf").is_err());
        assert!(Personality::try_from_name("turtle").is_ok());
    }

    #[test]
    fn utility_weights_gate_build_and_defense() {
        // 0.0.22 — pesi a 0 sopprimono, a 1.0 emettono come prima (continuità).
        let mut snap = empty_snapshot(1);
        idle_commander(&mut snap);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(intents.iter().any(|i| matches!(i, AiIntent::Build(_))));
        let muted = Personality {
            w_build: 0.0,
            w_defense: 0.0,
            ..Personality::TURTLE
        };
        let intents = decide(&snap, &muted, Scenario::Playground, &[], 0);
        assert!(
            !intents.iter().any(|i| matches!(i, AiIntent::Build(_))),
            "w_build=0 deve sopprimere il bootstrap: {intents:?}"
        );
    }

    #[test]
    fn utility_weights_gate_enqueue_and_scout() {
        // Enqueue: factory libera + mix rusher = emette a peso 1, tace a 0.
        let snap = armed_snapshot(
            1,
            &[
                (UnitKind::Scout, 60.0, UnitOrder::Idle),
                (UnitKind::Scout, 60.0, UnitOrder::Idle),
            ],
        );
        let free = fac(1, 0, false, 1, &[]);
        let on = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            std::slice::from_ref(&free),
            0,
        );
        assert!(on.iter().any(|i| matches!(i, AiIntent::Enqueue { .. })));
        let muted = Personality {
            w_enqueue: 0.0,
            ..Personality::RUSHER
        };
        let off = decide(&snap, &muted, Scenario::Playground, &[free], 0);
        assert!(
            !off.iter().any(|i| matches!(i, AiIntent::Enqueue { .. })),
            "w_enqueue=0 deve sopprimere: {off:?}"
        );
        // Scout: 1 scout libero, cieco = emette a peso 1, tace a 0.
        let mut scout_snap = empty_snapshot(1);
        scout_snap.my_units.push(super::super::snapshot::AiUnit {
            entity: Entity::from_bits(42),
            pos: Vec3::ZERO,
            kind: UnitKind::Scout,
            order: UnitOrder::Idle,
            health: 60.0,
            max_health: 60.0,
        });
        let on = decide(
            &scout_snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[],
            0,
        );
        assert!(on.iter().any(|i| matches!(i, AiIntent::Scout { .. })));
        let muted = Personality {
            w_scout: 0.0,
            ..Personality::RUSHER
        };
        let off = decide(&scout_snap, &muted, Scenario::Playground, &[], 0);
        assert!(
            !off.iter().any(|i| matches!(i, AiIntent::Scout { .. })),
            "w_scout=0 deve sopprimere: {off:?}"
        );
    }

    #[test]
    fn decide_with_threat_matches_decide() {
        // 0.0.23 — a mappa uguale (stesso snapshot), la via cached emette gli
        // stessi intenti della via con build interna: la cache non cambia la
        // decisione, solo il costo.
        let mut snap = empty_snapshot(1);
        idle_commander(&mut snap);
        idle_engineer(&mut snap);
        let threat = super::super::threat::build_threat(&snap);
        for pers in [Personality::TURTLE, Personality::RUSHER] {
            let a = decide(&snap, &pers, Scenario::Playground, &[], 0);
            let b = decide_with_threat(&snap, &pers, Scenario::Playground, &[], 0, &threat);
            assert_eq!(a, b);
        }
        // Anche con nemici visibili (hotspot/threat attivi nei rami).
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(910),
            pos: Vec3::new(60.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: crate::units::archetype(UnitKind::HeavyTank).max_health,
        });
        let threat = super::super::threat::build_threat(&snap);
        let a = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        let b = decide_with_threat(
            &snap,
            &Personality::TURTLE,
            Scenario::Playground,
            &[],
            0,
            &threat,
        );
        assert_eq!(a, b);
        assert!(
            base_under_threat(&snap, Scenario::Playground)
                == base_under_threat_with_map(&snap, Scenario::Playground, &threat)
        );
    }

    #[test]
    fn ron_roundtrip() {
        // 0.0.21 — 4 `.ron` bit-identici alle const (stesso cervello, file).
        for (name, want) in [
            ("turtle", Personality::TURTLE),
            ("rusher", Personality::RUSHER),
            ("eco-only", Personality::ECO_ONLY),
            ("rush-scripted", Personality::RUSH_SCRIPTED),
        ] {
            let path = format!("personalities/{name}.ron");
            let text = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("manca {path}"));
            let got = Personality::from_ron(&text).expect("ron deve parsare");
            assert_eq!(got, want, "mismatch {name}");
            // Roundtrip via def: ser -> de stabile.
            let def = PersonalityDef::from(&want);
            let ser = ron::ser::to_string(&def).expect("ron ser");
            let back = Personality::from_ron(&ser).expect("ron de");
            assert_eq!(back, want, "roundtrip {name}");
        }
        assert!(Personality::from_ron("(name: \"x\")").is_err());
    }
}
