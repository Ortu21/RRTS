//! Utility strategy + tactics (pure, testable).
//!
//! Decisioni data-driven: ogni opzione ha scorer 0..1, la personalità pesa
//! le soglie. Aggiungere unità/edifici futuri = nuove righe in tabella +
//! pesi in [`Personality`], mai `if kind == X` nel core.

use crate::{
    economy::balance::{BuildingKind, MAX_QUEUE},
    orders::UnitOrder,
    scenario::Scenario,
    structures::{
        building_obstacle, factory_spawn_ok, placement_rule, site_approach, snap_to_grid,
        valid_ground,
    },
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
    /// Mix produttivo T1 (kind, peso): la coda insegue queste proporzioni.
    pub mix: [(UnitKind, u32); 3],
    /// Torrette difensive massime (0 = mai, solo turtle).
    pub max_turrets: usize,
    /// 0.0.15 — scaling: massimi per tipo (oltre il bootstrap).
    pub max_metals: usize,
    pub max_solars: usize,
    pub max_factories: usize,
    /// 0.0.15 — ingegneri massimi vivi+accodati (code non intasate).
    pub max_engineers: usize,
    /// 0.0.13 — ritirata sotto questa frazione HP (0 = mai).
    pub retreat_hp_frac: f32,
    /// 0.0.13 — focus fire sul nemico più debole quando dominante.
    pub focus_fire: bool,
    /// Tick snapshot (4Hz) oltre il quale si attacca comunque (scripted
    /// baseline). `u64::MAX` = mai: solo potenza/soglie decidono.
    pub attack_at_tick: u64,
}

impl Personality {
    pub const TURTLE: Self = Self {
        name: "turtle",
        army_threshold: 4,
        attack_power_mult: 1.5,
        engineer_first: true,
        second_solar: true,
        mix: [
            (UnitKind::HeavyTank, 5),
            (UnitKind::LightTank, 2),
            (UnitKind::Artillery, 3),
        ],
        max_turrets: 2,
        max_metals: 3,
        max_solars: 3,
        max_factories: 2,
        max_engineers: 2,
        retreat_hp_frac: 0.35,
        focus_fire: true,
        attack_at_tick: u64::MAX,
    };
    pub const RUSHER: Self = Self {
        name: "rusher",
        army_threshold: 2,
        attack_power_mult: 1.0,
        engineer_first: false,
        second_solar: false,
        mix: [
            (UnitKind::HeavyTank, 6),
            (UnitKind::LightTank, 3),
            (UnitKind::Artillery, 1),
        ],
        max_turrets: 0,
        max_metals: 2,
        max_solars: 2,
        max_factories: 1,
        max_engineers: 1,
        retreat_hp_frac: 0.25,
        focus_fire: true,
        attack_at_tick: u64::MAX,
    };
    /// Baseline eco-only per la league: costruisce economia fino ai cap, non
    /// attacca mai (soglia impossibile), nessun micro aggressivo. Sacco da
    /// boxe: perderci è regressione critica.
    pub const ECO_ONLY: Self = Self {
        name: "eco-only",
        army_threshold: usize::MAX,
        attack_power_mult: 1e9,
        engineer_first: false,
        second_solar: true,
        mix: [
            (UnitKind::Engineer, 2),
            (UnitKind::Scout, 1),
            (UnitKind::HeavyTank, 0),
        ],
        max_turrets: 0,
        max_metals: 3,
        max_solars: 3,
        max_factories: 2,
        max_engineers: 3,
        retreat_hp_frac: 0.0,
        focus_fire: false,
        attack_at_tick: u64::MAX,
    };
    /// Baseline rush-scripted per la league: bootstrap + solo HeavyTank,
    /// ondata a tempo fisso comunque vada, niente ritirate né focus.
    /// Pugile prevedibile per misurare la difesa.
    pub const RUSH_SCRIPTED: Self = Self {
        name: "rush-scripted",
        army_threshold: 2,
        attack_power_mult: 1.0,
        engineer_first: false,
        second_solar: false,
        mix: [
            (UnitKind::HeavyTank, 1),
            (UnitKind::LightTank, 0),
            (UnitKind::Artillery, 0),
        ],
        max_turrets: 0,
        max_metals: 2,
        max_solars: 2,
        max_factories: 1,
        max_engineers: 0,
        retreat_hp_frac: 0.0,
        focus_fire: false,
        attack_at_tick: 480, // 120s: l'ondata parte a tempo, comunque vada
    };

    pub fn from_name(name: &str) -> Self {
        match name {
            "rusher" => Self::RUSHER,
            "eco-only" => Self::ECO_ONLY,
            "rush-scripted" => Self::RUSH_SCRIPTED,
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
/// 0.0.16: riusa `AiSnapshot::fresh_troop_memory` (stessa matematica di prima,
/// niente cambi di comportamento).
pub fn scout_destination(snapshot: &AiSnapshot, scenario: Scenario) -> Vec3 {
    let foe_base = scenario.attack_target(snapshot.team as usize);
    // Ricordo fresco di truppe: lo scout va a confermare lì.
    if let Some(first) = snapshot.fresh_troop_memory(MEMORY_FRESH_TICKS).first() {
        return first.pos;
    }
    let flank = Vec3::new(0.0, 0.0, 60.0);
    let points = [foe_base, Vec3::ZERO, foe_base + flank, foe_base - flank];
    points[(snapshot.tick as usize / 8) % points.len()]
}

/// Meta attacco: baricentro dei ricordi freschi se il nemico è sparito dalla
/// vista (inseguimento onesto), altrimenti base nemica.
/// 0.0.16: riusa `AiSnapshot::remembered_centroid` (stessa matematica di
/// `memory::remembered_centroid` su `AiMemory`, niente cambi di comportamento).
pub fn attack_destination(snapshot: &AiSnapshot, scenario: Scenario) -> Vec3 {
    if snapshot.visible_enemies.is_empty()
        && let Some(centroid) = snapshot.remembered_centroid(MEMORY_FRESH_TICKS)
        && centroid.is_finite()
    {
        return centroid;
    }
    scenario.attack_target(snapshot.team as usize)
}

/// Tick snapshot (4Hz) minimo per il LabT2: 360 = 90s di partita. Prima il
/// T1 deve chiudersi (placeholder della futura logica a domanda 0.0.15).
pub const LABT2_MIN_TICK: u64 = 360;

/// Mix produttivo T2 (kind, peso): i LabT2 inseguono queste proporzioni.
pub const T2_MIX: [(UnitKind, u32); 2] = [(UnitKind::HeavyTank2, 2), (UnitKind::Artillery2, 1)];

/// Vista minima di una factory per la strategia: coda, blocco porte e tier.
#[derive(Clone, Debug)]
pub struct FactoryView {
    pub entity: Entity,
    pub queue_len: usize,
    pub blocked: bool,
    pub tier: u8,
    pub queued: Vec<UnitKind>,
}

/// Unità vive + accodate di un kind (la coda conta: evita di riaccodare lo
/// stesso kind ogni tick mentre la produzione è in corso).
fn count_kind(snapshot: &AiSnapshot, queued: &[UnitKind], kind: UnitKind) -> usize {
    snapshot.my_units.iter().filter(|u| u.kind == kind).count()
        + queued.iter().filter(|k| **k == kind).count()
}

/// Kind col rapporto di copertura più basso (conteggio/peso); pari → primo.
/// Deterministico: a parità vince l'ordine di tabella.
fn pick_deficit(cands: &[(UnitKind, u32)], count: impl Fn(UnitKind) -> usize) -> UnitKind {
    let mut best = cands[0].0;
    let mut best_ratio = f32::MAX;
    for (kind, weight) in cands {
        let ratio = count(*kind) as f32 / (*weight as f32).max(1.0);
        if ratio < best_ratio {
            best_ratio = ratio;
            best = *kind;
        }
    }
    best
}

/// Utility scoring: ritorna intenti ordinati per priorità. Puro e deterministico.
pub fn decide(
    snapshot: &AiSnapshot,
    personality: &Personality,
    scenario: Scenario,
    factories: &[FactoryView],
) -> Vec<AiIntent> {
    let mut intents = Vec::new();

    // 1. Macro a loop chiuso: bootstrap + scaling a domanda.
    // Legge stock/income/demand invece di costruire "a memoria".
    if snapshot.active_site.is_none() {
        let metal = snapshot.count_building(BuildingKind::Metal);
        let solar = snapshot.count_building(BuildingKind::Solar);
        let factory = snapshot.count_building(BuildingKind::Factory);
        // Domanda insoddisfatta (isteresi 10%): l'eco chiede più di quanto entra.
        let metal_starved =
            snapshot.demand[0] > snapshot.income[0] * 1.1 && snapshot.income[0] > 0.0;
        let energy_starved =
            snapshot.demand[1] > snapshot.income[1] * 1.1 && snapshot.income[1] > 0.0;
        if metal == 0 {
            intents.push(AiIntent::Build(BuildingKind::Metal));
        } else if solar == 0 {
            intents.push(AiIntent::Build(BuildingKind::Solar));
        } else if factory == 0 {
            intents.push(AiIntent::Build(BuildingKind::Factory));
        } else if personality.second_solar && solar < 2 {
            intents.push(AiIntent::Build(BuildingKind::Solar));
        } else if metal_starved && metal < personality.max_metals {
            intents.push(AiIntent::Build(BuildingKind::Metal));
        } else if energy_starved && solar < personality.max_solars {
            intents.push(AiIntent::Build(BuildingKind::Solar));
        } else if factory < personality.max_factories && metal >= 2 {
            // Seconda lab solo a eco metal avviata (2 Metal): raddoppia il
            // throughput, ma va nutrita.
            intents.push(AiIntent::Build(BuildingKind::Factory));
        } else if snapshot.complete_building(BuildingKind::LabT2) == 0
            && snapshot.complete_building(BuildingKind::Factory) > 0
            && snapshot.tick >= LABT2_MIN_TICK
        {
            intents.push(AiIntent::Build(BuildingKind::LabT2));
        }
    }
    // Difesa statica: torrette vicino alla base (l'executor cerca lo spot in
    // spirale dal centro). Conta anche i siti: niente doppie richieste.
    if personality.max_turrets > 0
        && snapshot.complete_building(BuildingKind::Factory) > 0
        && snapshot.count_building(BuildingKind::Turret) < personality.max_turrets
    {
        intents.push(AiIntent::Build(BuildingKind::Turret));
    }

    // 2. Produzione: una enqueue per factory libera (N lab = N code in
    // parallelo). Le bloccate si saltano: accodare lì brucia solo eco.
    // I conteggi includono vivi + accodati così le code non si intasano di
    // Engineer/Scout mentre il primo esce ancora dalla factory.
    let mut queued_all: Vec<UnitKind> = factories
        .iter()
        .flat_map(|f| f.queued.iter().copied())
        .collect();
    for view in factories {
        if view.queue_len >= MAX_QUEUE || view.blocked {
            continue;
        }
        let engineers = count_kind(snapshot, &queued_all, UnitKind::Engineer);
        let scouts = count_kind(snapshot, &queued_all, UnitKind::Scout);
        let kind = if personality.engineer_first
            && engineers < personality.max_engineers
            && snapshot.complete_building(BuildingKind::Factory) > 0
        {
            UnitKind::Engineer
        } else if scouts < 1
            && !has_fresh_eyes(snapshot)
            && snapshot.complete_building(BuildingKind::Factory) > 0
            && UnitKind::PRODUCIBLE.contains(&UnitKind::Scout)
        {
            // 0.0.14 — occhi prima di muscoli: uno Scout esploratore.
            UnitKind::Scout
        } else if view.tier >= 2 {
            // LabT2: mix pesante T2. Il gate di `enqueue` lo ribadisce, ma
            // qui non si emette mai un T2 verso una T1.
            pick_deficit(&T2_MIX, |k| count_kind(snapshot, &queued_all, k))
        } else {
            // Mix T1 per deficit di copertura (vivi + accodati). Pesi 0 =
            // mai (baseline eco): mix vuoto → nessuna enqueue.
            let mix: Vec<(UnitKind, u32)> = personality
                .mix
                .iter()
                .copied()
                .filter(|(_, w)| *w > 0)
                .collect();
            if mix.is_empty() {
                continue;
            }
            pick_deficit(&mix, |k| count_kind(snapshot, &queued_all, k))
        };
        // Accoda solo se producibile (mai Commander).
        if UnitKind::PRODUCIBLE.contains(&kind) {
            queued_all.push(kind);
            intents.push(AiIntent::Enqueue {
                factory: view.entity,
                kind,
            });
        }
    }

    // 3. Tattica: attacco quando soglia truppe + potenza stimata, oppure a
    // tempo fisso per le baseline scripted (comunque vada, se c'è un esercito).
    // La potenza nemica stimata unisce vista live e ricordi freschi scontati.
    let army_count = snapshot.army().len();
    let my_power = army_power(snapshot);
    let foe_power = visible_enemy_power(snapshot) + remembered_enemy_power(snapshot);
    let power_ok = if snapshot.visible_enemies.is_empty() && !has_fresh_eyes(snapshot) {
        // Nemico mai visto: serve massa critica per marciare alla cieca.
        // Saturating: soglie "mai" (usize::MAX delle baseline) non devono
        // andare in overflow.
        army_count >= personality.army_threshold.saturating_add(2)
    } else {
        my_power > foe_power * personality.attack_power_mult.max(0.1)
            && army_count >= personality.army_threshold
    };
    let force_attack = army_count > 0 && snapshot.tick >= personality.attack_at_tick;
    if (power_ok || force_attack) && army_count > 0 {
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

/// Rally default riparato: 18m verso il fronte, ma mai dentro roccia o senza
/// clearance. Un rally murato blocca la factory PER SEMPRE (`free_exit`
/// richiede path porta->rally per tutte le porte e la AI salta le factory
/// bloccate): meglio nessun rally (fallback apron) che uno murato.
pub fn default_rally(grid: &crate::navigation::NavGrid, from: Vec3, target: Vec3) -> Option<Vec3> {
    let dir = (target - from).normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }
    let repaired = grid.clear_point_for(from + dir * 18.0, 0.5);
    (grid.is_walkable(repaired) && grid.has_clearance(repaired)).then_some(repaired)
}

/// Ricerca deterministica dello spot edificabile: spirale dal centro base.
/// Ritorna il primo punto con `valid_ground` + `placement_rule` + path dal
/// builder — e per le Factory anche una porta d'uscita libera, così le truppe
/// in coda spawnano sempre (niente lab murati vivi).
/// Il path è validato verso lo stand-off sul bordo (`site_approach`), cioè
/// dove il builder andrà davvero: validare il centro darebbe falsi positivi
/// (il centro è libero finché l'edificio non esiste) e tasche irraggiungibili
/// manderebbero l'ordine Build in loop MoveTarget/fail a ogni frame.
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
            // Stand-off reale del builder (bordo footprint, non centro):
            // raggio scafo conservativo (commander 1.4) così vale per tutti.
            // Validato sulla grid CON il futuro edificio: il suo stesso
            // ostacolo può sigillare l'approach (lab grandi in basi dense).
            let probe = grid.cloned_with_obstacle(building_obstacle(kind, point));
            let approach = site_approach(&probe, point, builder_pos, kind.stats().half, 1.4);
            if probe.find_path(builder_pos, approach).is_none() {
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
        assert!(grid.is_walkable(rally) && grid.has_clearance(rally));
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
            mix: [
                (UnitKind::Commander, 1),
                (UnitKind::HeavyTank, 1),
                (UnitKind::LightTank, 1),
            ],
            ..Personality::RUSHER
        };
        let snap = empty_snapshot(1);
        let intents = decide(&snap, &p, Scenario::Playground, &[fac(1, 0, false, 1, &[])]);
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
        // Scout vivo (niente ramo scout): a conteggi vuoti vince il peso
        // maggiore, poi il deficit guida le scelte successive.
        let scout = &[(UnitKind::Scout, 60.0, UnitOrder::Idle)];
        let snap = armed_snapshot(1, scout);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::HeavyTank));
        // 6 heavy saturano il 60%: tocca al LightTank (0/3).
        let mut heavies: Vec<(UnitKind, f32, UnitOrder)> =
            scout.iter().map(|(k, h, o)| (*k, *h, o.clone())).collect();
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
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::LightTank));
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
        );
        let kind = enqueue_kind(&intents).unwrap();
        assert_eq!(archetype(kind).tier, 1);
        // LabT2: mix pesante T2, prima scelta HeavyTank2.
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(2, 0, false, 2, &[])],
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::HeavyTank2));
    }

    #[test]
    fn labt2_needs_time_and_factory() {
        let mut snap = armed_snapshot(1, &[]);
        snap.tick = 0;
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[]);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::LabT2)))
        );
        snap.tick = LABT2_MIN_TICK;
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[]);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::LabT2)));
    }

    #[test]
    fn turret_defense_is_turtle_only_and_capped() {
        let snap = armed_snapshot(1, &[]);
        let turtle = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[]);
        assert!(turtle.contains(&AiIntent::Build(BuildingKind::Turret)));
        let rush = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[]);
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
        let intents = decide(&capped, &Personality::TURTLE, Scenario::Playground, &[]);
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
            let intents = decide(&snap, &p, Scenario::Playground, &[]);
            assert!(
                intents
                    .iter()
                    .any(|i| matches!(i, AiIntent::Retreat { .. })),
                "{p:?} dovrebbe ritirare il tank al 20%"
            );
        }
        // Tank sano: nessuna ritirata.
        let healthy = armed_snapshot(1, &[(UnitKind::HeavyTank, max, UnitOrder::Idle)]);
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
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[]);
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
            kind: Some(UnitKind::HeavyTank),
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
            &[fac(1, 0, false, 1, &[])],
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
        );
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Enqueue { .. }))
        );
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
        snap
    }

    fn give_fresh_eyes(snap: &mut AiSnapshot) {
        use super::super::snapshot::AiMemory;
        snap.memory.push(AiMemory {
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
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[]);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::Metal)));
        // Al cap (3): basta, anche se affamata.
        let snap = eco_snapshot(1, 3, 2, 1, [15.0, 24.0], [30.0, 10.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[]);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Metal)))
        );
        // Domanda soddisfatta: nessun nuovo Metal.
        let snap = eco_snapshot(1, 1, 2, 1, [5.0, 24.0], [4.0, 10.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[]);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Metal)))
        );
    }

    #[test]
    fn starved_energy_requests_solar_until_cap() {
        let snap = eco_snapshot(1, 2, 1, 1, [10.0, 12.0], [5.0, 30.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[]);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::Solar)));
        // Rusher al suo cap (2): basta.
        let snap = eco_snapshot(1, 2, 2, 1, [10.0, 24.0], [5.0, 60.0]);
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[]);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Solar)))
        );
    }

    #[test]
    fn second_factory_needs_two_metals() {
        // Eco bilanciata ma un solo Metal: niente seconda lab.
        let snap = eco_snapshot(1, 1, 2, 1, [5.0, 24.0], [5.0, 10.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[]);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Factory)))
        );
        // Due Metal: via alla seconda (sotto il cap turtle di 2).
        let snap = eco_snapshot(1, 2, 2, 1, [10.0, 24.0], [9.0, 10.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[]);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::Factory)));
        // Rusher resta a una sola lab per scelta.
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[]);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Factory)))
        );
    }

    #[test]
    fn engineer_and_scout_queues_count_toward_caps() {
        // Turtle: 1 engineer vivo + 1 accodato = cap raggiunto → truppa.
        let mut snap = armed_snapshot(1, &[(UnitKind::Engineer, 70.0, UnitOrder::Idle)]);
        give_fresh_eyes(&mut snap);
        let intents = decide(
            &snap,
            &Personality::TURTLE,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[UnitKind::Engineer])],
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::HeavyTank));
        // Scout accodato ma non ancora uscito: niente doppione.
        let mut snap = armed_snapshot(1, &[]);
        give_fresh_eyes(&mut snap);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[UnitKind::Scout])],
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::HeavyTank));
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
        let intents = decide(&snap, &Personality::ECO_ONLY, Scenario::Playground, &[]);
        assert!(!intents.iter().any(|i| matches!(
            i,
            AiIntent::AttackMoveAll { .. } | AiIntent::FocusFire { .. }
        )));
    }

    #[test]
    fn rush_scripted_attacks_on_schedule_regardless_of_odds() {
        use crate::units::archetype;
        // Un solo heavy contro forza superiore: prima dello schedule, niente.
        let mut snap = eco_snapshot(1, 2, 1, 1, [10.0, 12.0], [9.0, 30.0]);
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
        );
        assert!(
            !early
                .iter()
                .any(|i| matches!(i, AiIntent::AttackMoveAll { .. }))
        );
        // ...allo schedule: l'ondata parte comunque.
        snap.tick = Personality::RUSH_SCRIPTED.attack_at_tick;
        let late = decide(
            &snap,
            &Personality::RUSH_SCRIPTED,
            Scenario::Playground,
            &[],
        );
        assert!(
            late.iter()
                .any(|i| matches!(i, AiIntent::AttackMoveAll { .. }))
        );
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
    }
}
