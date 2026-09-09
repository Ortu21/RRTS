//! Opponent modeling leggero (0.0.19): classifica rusher/turtle/eco.
//!
//! Zielinski Pro1 Ch33 (smart questions: conteggi freschi + timing primo
//! contatto + edifici visti) + Borovikov autoplay (stessi cervelli, shift
//! pesi, niente ML). Legge SOLO [`AiSnapshot`](super::snapshot::AiSnapshot):
//! mai query nemiche dirette, mai fog bypassato. Tutto puro e deterministico
//! (conteggi interi, `total_cmp` dove serve, tabelle const, niente
//! HashMap/rand/time).
//!
//!Output = shift piccoli da tabella (courage ±0.1, mix ×(1±0.1)): niente
//! switch totale di strategia mid-game.

use crate::{economy::balance::BuildingKind, units::UnitKind};

use super::{memory::MEMORY_FRESH_TICKS, snapshot::AiSnapshot};

/// Classi avversarie (soglie, niente ML).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OpponentKind {
    Rusher,
    Turtle,
    Eco,
    Unknown,
}

impl OpponentKind {
    /// Tutte le classi (ordine = righe tabelle). Uso diretto nei test di
    /// accuratezza; oggi solo test: allow per clippy `-D warnings`.
    #[allow(dead_code)]
    pub const ALL: [Self; 4] = [Self::Rusher, Self::Turtle, Self::Eco, Self::Unknown];

    fn index(self) -> usize {
        match self {
            Self::Rusher => 0,
            Self::Turtle => 1,
            Self::Eco => 2,
            Self::Unknown => 3,
        }
    }
}

/// Tick snapshot (4Hz) entro cui il primo contatto marca pressione precoce:
/// 240 = 60s. Chi si fa vedere con truppe entro il minuto gioca d'attacco.
pub const EARLY_CONTACT_TICK: u64 = 240;
/// Truppe fresche (live max ricordi) che marcano massa rusher.
pub const RUSHER_TROOP_MIN: usize = 3;
/// Edifici eco visti che marcano economia (con poche truppe).
pub const ECO_BUILDING_MIN: usize = 2;

/// Shift courage da tabella (±0.1): vs rusher più prudenza, vs turtle/eco più
/// pressione (punire chi si chiude o chi grida). Unknown = neutro.
/// Ordine tabella = `OpponentKind::ALL`.
pub const COURAGE_SHIFT: [f32; 4] = [-0.1, 0.1, 0.1, 0.0];

/// Bias mix da tabella (×(1±0.1)): righe = avversario (`OpponentKind::ALL`),
/// colonne = `UnitKind`. Quasi tutto 1.0; solo celle motivate:
/// - vs Rusher (massa Heavy veloce): Heavy/Heavy2 e Arty reggono, Light muore
///   (Heavy-vs-Light 1.15 da `COUNTER_TABLE`).
/// - vs Turtle (torrette + Arty dietro): Arty/Arty2 fuori-rangeano la torretta
///   (30m vs 24m), Light si schianta sulle mura.
/// - vs Eco (poche truppe, tanta eco): Light veloci (10 m/s) a punire, Arty
///   lente sprecate.
///
/// Dato, non logica: i sistemi leggono tramite `mix_bias`, mai branch.
/// Nuove truppe = 1.0 neutro (l'AI non le costruisce ancora: restano appannaggio
/// del player finché un mix T1/T2 non le adotta).
pub const MIX_BIAS: [[f32; 15]; 4] = [
    //              Scout  Heavy  Arty   Cmdr   Eng    Light  Hvy2   Arty2  Mg     Laser  Rockt  Mssl   Mortar SkyArt Vang
    /* Rusher  */
    [
        1.0, 1.1, 1.1, 1.0, 1.0, 0.9, 1.1, 1.1, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    ],
    /* Turtle  */
    [
        1.0, 1.0, 1.1, 1.0, 1.0, 0.9, 1.0, 1.1, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    ],
    /* Eco     */
    [
        1.0, 1.0, 0.9, 1.0, 1.0, 1.1, 1.0, 0.9, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    ],
    /* Unknown */
    [
        1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    ],
];

/// Classifica l'avversario da conteggi freschi + timing primo contatto +
/// edifici visti. Pura.
///
/// - Truppe = max(live, ricordi freschi): evita doppi conteggi live+memoria
///   (lo snapshot non ha entity sui ricordi, non si può dedupare).
/// - Primo contatto = ricordo più vecchio (qualsiasi età, anche stantio):
///   se la memoria è vuota nessun contatto è mai avvenuto.
/// - Edifici = quelli visibili ora (con kind) — i ricordi edifici non hanno
///   kind (solo pos), quindi guidano l'attacco ma non la classifica.
///
/// Tabella decisionale (ordine = priorità):
/// 1. Torretta/Muro visti → Turtle (solo il turtle li costruisce).
/// 2. Massa precoce (≥3 truppe e contatto <60s o tick <120s) → Rusher.
/// 3. Eco (≥2 Metal/Solar/Factory e ≤2 truppe) → Eco.
/// 4. Massa tardiva (≥4 truppe) → Rusher.
/// 5. Altrimenti Unknown.
pub fn classify(snapshot: &AiSnapshot) -> OpponentKind {
    let live_troops = snapshot.visible_enemies.len();
    let remembered_troops = snapshot.fresh_troop_memory(MEMORY_FRESH_TICKS).len();
    let troops = live_troops.max(remembered_troops);

    // Primo contatto: età massima in memoria (anche stantia, oltre il fresh).
    let first_seen_tick = snapshot
        .memory
        .iter()
        .map(|m| m.age_ticks)
        .max()
        .map(|max_age| snapshot.tick.saturating_sub(max_age));
    let early = first_seen_tick.is_some_and(|t| t < EARLY_CONTACT_TICK);

    let has_static_defense = snapshot.visible_enemy_buildings.iter().any(|b| {
        matches!(
            b.kind,
            BuildingKind::Turret | BuildingKind::Wall | BuildingKind::Lance
        )
    });
    if has_static_defense {
        return OpponentKind::Turtle;
    }

    if troops >= RUSHER_TROOP_MIN && (early || snapshot.tick < EARLY_CONTACT_TICK * 2) {
        return OpponentKind::Rusher;
    }

    let eco_buildings = snapshot
        .visible_enemy_buildings
        .iter()
        .filter(|b| {
            matches!(
                b.kind,
                BuildingKind::Metal | BuildingKind::Solar | BuildingKind::Factory
            )
        })
        .count();
    if eco_buildings >= ECO_BUILDING_MIN && troops <= 2 {
        return OpponentKind::Eco;
    }

    if troops > RUSHER_TROOP_MIN {
        return OpponentKind::Rusher;
    }

    OpponentKind::Unknown
}

/// Shift courage da tabella per l'avversario. Puro.
pub fn courage_shift(opponent: OpponentKind) -> f32 {
    COURAGE_SHIFT[opponent.index()]
}

/// Courage effettivo: base + shift, clampato a [0.0, 1.2] così le baseline
/// "mai" (eco-only 1.1) restano mai anche con +0.1. Puro.
pub fn adjust_courage(base: f32, opponent: OpponentKind) -> f32 {
    (base + courage_shift(opponent)).clamp(0.0, 1.2)
}

/// Bias mix da tabella per (kind, avversario). Puro, lookup da tabella.
pub fn mix_bias(kind: UnitKind, opponent: OpponentKind) -> f32 {
    MIX_BIAS[opponent.index()][kind.index()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{orders::UnitOrder, units::archetype};
    use bevy::prelude::*;

    fn empty_snapshot(team: u8) -> AiSnapshot {
        AiSnapshot {
            team,
            ..Default::default()
        }
    }

    fn troop_snapshot(team: u8, n: usize, age: u64, tick: u64) -> AiSnapshot {
        let mut snap = empty_snapshot(team);
        snap.tick = tick;
        for i in 0..n {
            snap.memory.push(super::super::snapshot::AiMemory {
                entity_bits: None,
                pos: Vec3::new(i as f32 * 5.0, 0.0, 0.0),
                age_ticks: age,
                kind: Some(UnitKind::HeavyTank),
                hp: archetype(UnitKind::HeavyTank).max_health,
                building: false,
            });
        }
        snap
    }

    #[test]
    fn tables_are_shaped() {
        assert_eq!(OpponentKind::ALL.len(), 4);
        assert_eq!(COURAGE_SHIFT.len(), 4);
        for row in &MIX_BIAS {
            assert_eq!(row.len(), UnitKind::ALL.len());
            for v in row {
                assert!(v.is_finite() && *v > 0.0);
            }
        }
        // Unknown = neutro su tutto.
        assert_eq!(courage_shift(OpponentKind::Unknown), 0.0);
        for kind in UnitKind::ALL {
            assert_eq!(mix_bias(kind, OpponentKind::Unknown), 1.0);
        }
        // Shift entro ±0.1 come da spec.
        for s in COURAGE_SHIFT {
            assert!(s.abs() <= 0.11, "{s}");
        }
        for row in &MIX_BIAS {
            for v in row {
                assert!((*v - 1.0).abs() <= 0.11, "{v}");
            }
        }
    }

    #[test]
    fn courage_never_breaks_eco_only() {
        // Eco-only 1.1 + shift deve restare "mai" (win_prob ≤ 1.0 non supera).
        for opp in OpponentKind::ALL {
            let c = adjust_courage(1.1, opp);
            assert!(c >= 1.0, "{opp:?} courage {c}");
            assert!(c <= 1.2);
        }
        assert!((adjust_courage(0.55, OpponentKind::Rusher) - 0.45).abs() < 1e-6);
        assert!((adjust_courage(0.65, OpponentKind::Turtle) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn classify_rusher() {
        // Massa precoce con contatto al minuto 1: rusher da manuale.
        let mut snap = troop_snapshot(1, 4, 10, 300);
        // Ricordo vecchio = primo contatto a tick 100 (< 240 = precoce).
        snap.memory.push(super::super::snapshot::AiMemory {
            entity_bits: None,
            pos: Vec3::new(99.0, 0.0, 99.0),
            age_ticks: 200,
            kind: Some(UnitKind::LightTank),
            hp: 70.0,
            building: false,
        });
        assert_eq!(classify(&snap), OpponentKind::Rusher);
    }

    #[test]
    fn classify_turtle_on_turret() {
        // Una torretta vista batte qualsiasi conteggio truppe.
        let mut snap = troop_snapshot(1, 4, 10, 300);
        snap.visible_enemy_buildings
            .push(super::super::snapshot::AiBuilding {
                entity: Entity::from_bits(700),
                kind: BuildingKind::Turret,
                pos: Vec3::new(200.0, 0.0, 200.0),
                under_construction: false,
                health: 450.0,
            });
        assert_eq!(classify(&snap), OpponentKind::Turtle);
    }

    #[test]
    fn classify_eco_and_unknown() {
        // Eco: 2+ edifici economici visti e poche truppe.
        let mut snap = empty_snapshot(1);
        snap.tick = 500;
        for (i, kind) in [BuildingKind::Metal, BuildingKind::Solar]
            .into_iter()
            .enumerate()
        {
            snap.visible_enemy_buildings
                .push(super::super::snapshot::AiBuilding {
                    entity: Entity::from_bits(710 + i as u64),
                    kind,
                    pos: Vec3::new(i as f32 * 10.0, 0.0, 0.0),
                    under_construction: false,
                    health: 300.0,
                });
        }
        assert_eq!(classify(&snap), OpponentKind::Eco);
        // Buio totale: unknown, mai una classe a caso.
        let dark = empty_snapshot(1);
        assert_eq!(classify(&dark), OpponentKind::Unknown);
    }

    #[test]
    fn classify_accuracy_above_80_percent() {
        // Batteria di 10 casi etichettati (rusher/turtle/eco/unknown):
        // il classificatore a soglie deve indovinarne ≥8 (≥80% da spec).
        struct Case {
            snap: AiSnapshot,
            want: OpponentKind,
        }
        let mut cases: Vec<Case> = Vec::new();
        // 3 rusher: masse precoci con primo contatto precoce.
        for i in 0..3 {
            let mut snap = troop_snapshot(1, 3 + i, 5, 200 + i as u64 * 10);
            snap.memory.push(super::super::snapshot::AiMemory {
                entity_bits: None,
                pos: Vec3::ZERO,
                age_ticks: 150 + i as u64 * 10,
                kind: Some(UnitKind::HeavyTank),
                hp: 100.0,
                building: false,
            });
            cases.push(Case {
                snap,
                want: OpponentKind::Rusher,
            });
        }
        // 3 turtle: torretta o muro visti.
        for (i, kind) in [
            BuildingKind::Turret,
            BuildingKind::Wall,
            BuildingKind::Turret,
        ]
        .into_iter()
        .enumerate()
        {
            let mut snap = empty_snapshot(1);
            snap.tick = 400 + i as u64;
            snap.visible_enemy_buildings
                .push(super::super::snapshot::AiBuilding {
                    entity: Entity::from_bits(800 + i as u64),
                    kind,
                    pos: Vec3::ZERO,
                    under_construction: false,
                    health: 400.0,
                });
            cases.push(Case {
                snap,
                want: OpponentKind::Turtle,
            });
        }
        // 2 eco: doppia eco vista, poche truppe.
        for i in 0..2 {
            let mut snap = empty_snapshot(1);
            snap.tick = 600 + i as u64;
            for (j, kind) in [BuildingKind::Metal, BuildingKind::Solar]
                .into_iter()
                .enumerate()
            {
                snap.visible_enemy_buildings
                    .push(super::super::snapshot::AiBuilding {
                        entity: Entity::from_bits(900 + (i * 10 + j) as u64),
                        kind,
                        pos: Vec3::new(j as f32, 0.0, 0.0),
                        under_construction: false,
                        health: 300.0,
                    });
            }
            cases.push(Case {
                snap,
                want: OpponentKind::Eco,
            });
        }
        // 2 unknown: buio o singolo contatto tardivo isolato.
        cases.push(Case {
            snap: empty_snapshot(1),
            want: OpponentKind::Unknown,
        });
        {
            let snap = troop_snapshot(1, 1, 5, 900);
            // Primo contatto tardivo (900-5=895): non precoce, poche truppe.
            cases.push(Case {
                snap,
                want: OpponentKind::Unknown,
            });
        }
        assert_eq!(cases.len(), 10);
        let mut hits = 0;
        for (i, case) in cases.iter().enumerate() {
            let got = classify(&case.snap);
            assert!(
                got == case.want,
                "caso {i}: want {:?} got {got:?}",
                case.want
            );
            if got == case.want {
                hits += 1;
            }
        }
        let acc = hits as f32 / cases.len() as f32;
        assert!(acc >= 0.8, "accuracy {acc:.2} < 0.80 ({hits}/10)");
        let _ = UnitOrder::Idle;
    }
}
