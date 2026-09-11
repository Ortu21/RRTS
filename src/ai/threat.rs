//! Threat/influence map multistrato range-aware (0.0.16 → 0.0.23).
//!
//! Legge SOLO [`AiSnapshot`](super::snapshot::AiSnapshot) — quindi è onesta
//! per costruzione (mai query nemiche dirette, mai fog bypassato).
//! 0.0.23 (Mark Pro2 Ch30 modulare, Zielinski Pro3 Ch24: la gittata decide):
//! ogni sorgente proietta `dps×hp` entro la sua gittata da tabella (mai
//! branch per-kind) con falloff lineare, su 3 layer (`live`, `remembered`,
//! `static_def`). `cells` resta la somma dei layer = API combinata invariata
//! (`query`/`hotspot`/`mean`/`max` non cambiano firma né semantica).
//! Tutto puro e deterministico: ordine di accumulo stabile (snapshot poi
//! row-major), niente `HashMap`/rand/time.
//!
//! Cache: `ThreatCache` (chiave = snapshot tick per team) riduce le build a
//! 1 per team per strategy-tick — `decide()`/`executor` ricevono la mappa.

use crate::{
    economy::balance::{BuildingKind, turret_stats},
    navigation::HALF_SIZE,
    units::UnitKind,
};
use bevy::prelude::*;
use std::collections::BTreeMap;

use super::{memory::MEMORY_FRESH_TICKS, snapshot::AiSnapshot, strategy::combat_power};

/// Risoluzione griglia: 32×32 su tutta la mappa (cfr. fog 150×150 da 4m).
/// Grossolana di proposito: stima strategica, non targeting.
pub const THREAT_GRID_N: usize = 32;
/// Costante di decadimento ricordi: peso `1/(1+age/K)`.
pub const THREAT_DECAY_K: f32 = 60.0;
/// Sconto posizioni non verificate (coerente con `remembered_enemy_power`).
pub const THREAT_MEMORY_DISCOUNT: f32 = 0.5;

/// Mappa minaccia per-team, costruita dallo snapshot onesto.
/// 0.0.23 — 3 layer (Mark modulare): `live` (nemici visibili), `remembered`
/// (ricordi freschi, già scontati), `static_def` (torrette nemiche visibili).
/// `cells` = somma dei tre (API combinata invariata da 0.0.16).
#[derive(Clone, Debug)]
pub struct ThreatMap {
    pub n: usize,
    pub half: f32,
    pub cells: Vec<f32>,
    pub live: Vec<f32>,
    pub remembered: Vec<f32>,
    pub static_def: Vec<f32>,
}

impl ThreatMap {
    fn empty() -> Self {
        let zeros = || vec![0.0; THREAT_GRID_N * THREAT_GRID_N];
        Self {
            n: THREAT_GRID_N,
            half: HALF_SIZE,
            cells: zeros(),
            live: zeros(),
            remembered: zeros(),
            static_def: zeros(),
        }
    }

    fn cell_side(&self) -> f32 {
        (self.half * 2.0) / self.n as f32
    }

    fn index_for(&self, pos: Vec3) -> usize {
        let side = self.cell_side();
        let mut col = ((pos.x + self.half) / side).floor() as isize;
        let mut row = ((pos.z + self.half) / side).floor() as isize;
        col = col.clamp(0, self.n as isize - 1);
        row = row.clamp(0, self.n as isize - 1);
        row as usize * self.n + col as usize
    }

    /// 0.0.23 — deposita `weight` con gittata `range` sul `layer`
    /// (0=live, 1=remembered, 2=static) + sul combinato: cella propria piena,
    /// vicine entro gittata con falloff lineare `w*(1-dist/(range+side))`.
    /// Iterazione row-major sul bounding box = deterministica. `range <= 0`
    /// (mischia/sconosciuta) = solo cella propria (vecchia matematica).
    /// Aritmetica su copie locali (niente `&self` a mappa mutuata).
    fn deposit(&mut self, pos: Vec3, weight: f32, range: f32, layer: usize) {
        if weight <= 0.0 || !weight.is_finite() {
            return;
        }
        let n = self.n;
        let half = self.half;
        let side = (half * 2.0) / n as f32;
        let col_of = |v: f32| ((v + half) / side).floor() as isize;
        let own_col = col_of(pos.x).clamp(0, n as isize - 1);
        let own_row = col_of(pos.z).clamp(0, n as isize - 1);
        let own = own_row as usize * n + own_col as usize;
        let center = |idx: usize| {
            let row = idx / n;
            let col = idx % n;
            Vec3::new(
                -half + (col as f32 + 0.5) * side,
                0.0,
                -half + (row as f32 + 0.5) * side,
            )
        };
        // Raggio celle coperto dalla gittata (bound strutturale per i test).
        let mut touched: Vec<(usize, f32)> = vec![(own, weight)];
        if range.is_finite() && range > 0.0 {
            let radius = (range / side).ceil() as isize;
            for row in (own_row - radius).max(0)..=(own_row + radius).min(n as isize - 1) {
                for col in (own_col - radius).max(0)..=(own_col + radius).min(n as isize - 1) {
                    let idx = row as usize * n + col as usize;
                    if idx == own {
                        continue;
                    }
                    let dist = center(idx).xz().distance(pos.xz());
                    if dist <= range {
                        let c = weight * (1.0 - dist / (range + side));
                        if c > 0.0 {
                            touched.push((idx, c));
                        }
                    }
                }
            }
        }
        // Ordine row-major = deterministico (l'own può precedere i vicini).
        touched.sort_by_key(|(idx, _)| *idx);
        let target: &mut Vec<f32> = match layer {
            0 => &mut self.live,
            1 => &mut self.remembered,
            _ => &mut self.static_def,
        };
        for (idx, c) in touched {
            target[idx] += c;
            self.cells[idx] += c;
        }
    }

    /// Minaccia nella cella che contiene `pos` (0 fuori mappa → clamp al bordo).
    /// Uso diretto in 0.0.17+ (torrette/scout); in 0.0.16 solo test+metriche.
    #[allow(dead_code)]
    pub fn query(&self, pos: Vec3) -> f32 {
        self.cells[self.index_for(pos)]
    }

    /// 0.0.23 — minaccia statica (torrette nemiche) nella cella di `pos`:
    /// l'anchor torrette guarda qui quando la fanteria è lontana.
    pub fn query_static(&self, pos: Vec3) -> f32 {
        self.static_def[self.index_for(pos)]
    }

    /// 0.0.23 — medie per layer (live, remembered, static) per telemetria
    /// (director/league): valori alti sono informazione, mai errore.
    pub fn means(&self) -> (f32, f32, f32) {
        let mean = |v: &[f32]| {
            if v.is_empty() {
                0.0
            } else {
                v.iter().sum::<f32>() / v.len() as f32
            }
        };
        (
            mean(&self.live),
            mean(&self.remembered),
            mean(&self.static_def),
        )
    }

    /// 0.0.23 — i layer sono sani se finiti, non-negativi e sommano al
    /// combinato (gate `threat-sane` del director). Puro.
    pub fn layers_sane(&self) -> bool {
        if self.live.len() != self.cells.len()
            || self.remembered.len() != self.cells.len()
            || self.static_def.len() != self.cells.len()
        {
            return false;
        }
        self.cells
            .iter()
            .zip(self.live.iter())
            .zip(self.remembered.iter())
            .zip(self.static_def.iter())
            .all(|(((c, l), r), s)| {
                c.is_finite()
                    && *c >= 0.0
                    && *l >= 0.0
                    && *r >= 0.0
                    && *s >= 0.0
                    && (*c - (*l + *r + *s)).abs() < 0.01
            })
    }

    /// Media su tutte le celle (0 se vuota).
    pub fn mean(&self) -> f32 {
        if self.cells.is_empty() {
            return 0.0;
        }
        let sum: f32 = self.cells.iter().sum();
        sum / self.cells.len() as f32
    }

    /// Massimo su tutte le celle.
    pub fn max(&self) -> f32 {
        self.cells.iter().fold(0.0f32, |a, b| a.max(*b))
    }

    /// Centro della cella più minacciosa. Pareggi → indice minore
    /// (deterministico). `None` se tutto zero.
    /// Uso diretto in 0.0.17+ (difesa/attacco); in 0.0.16 solo test.
    #[allow(dead_code)]
    pub fn hotspot(&self) -> Option<Vec3> {
        let mut best_idx: Option<usize> = None;
        let mut best_val = 0.0f32;
        for (i, v) in self.cells.iter().enumerate() {
            if *v > best_val {
                best_val = *v;
                best_idx = Some(i);
            }
        }
        let idx = best_idx?;
        let side = self.cell_side();
        let row = idx / self.n;
        let col = idx % self.n;
        Some(Vec3::new(
            -self.half + (col as f32 + 0.5) * side,
            0.0,
            -self.half + (row as f32 + 0.5) * side,
        ))
    }
}

/// Peso per età: 1 a vista live, decade per ricordi. Puro.
pub fn age_decay(age_ticks: u64) -> f32 {
    1.0 / (1.0 + age_ticks as f32 / THREAT_DECAY_K)
}

/// Minaccia di un edificio nemico visibile: solo torrette (da tabella
/// `turret_stats`, mai branch per-kind qui fuori). Muri/eco = 0.
pub fn building_threat(kind: BuildingKind, hp: f32) -> f32 {
    if hp <= 0.0 {
        return 0.0;
    }
    match turret_stats(kind) {
        Some(gun) => hp * (gun.damage / gun.cooldown.max(0.05)) * 0.8,
        None => 0.0,
    }
}

/// 0.0.23 — gittata di proiezione della minaccia di un'unità: max tra
/// primaria e secondaria (stessa regola di `combat::max_range`). Disarmati
/// (Engineer) = 0 (peso comunque 0: mai depositati). Da tabella, mai branch
/// per-kind. Pura.
pub fn threat_range(kind: UnitKind) -> f32 {
    let stats = crate::units::archetype(kind);
    if !stats.armed {
        return 0.0;
    }
    let mut range = stats.range;
    if let Some(sec) = crate::units::archetype::secondary_stats(kind) {
        range = range.max(sec.range);
    }
    if range.is_finite() {
        range.max(0.0)
    } else {
        0.0
    }
}

/// 0.0.23 — gittata di proiezione di un edificio nemico: torrette da tabella,
/// resto 0 (muri/eco non proiettano: nessun DPS, lo channeling è del nav).
/// Pura.
pub fn building_range(kind: BuildingKind) -> f32 {
    turret_stats(kind).map(|g| g.range).unwrap_or(0.0)
}

/// Costruisce la mappa dallo snapshot onesto, 0.0.23 multistrato range-aware:
/// nemici visibili sul layer live (peso 1, spread in gittata) + torrette
/// visibili sullo static (spread in gittata torretta) + ricordi freschi di
/// unità sul remembered (peso `decay*discount`, spread in gittata ricordata).
/// Edifici ricordati senza kind: ignorati (nessuna info).
/// Ordine di accumulo = ordine snapshot (già ordinato per determinismo).
pub fn build_threat(snapshot: &AiSnapshot) -> ThreatMap {
    let mut map = ThreatMap::empty();
    for e in &snapshot.visible_enemies {
        let w = threat_unit_weight(e.kind, e.health, 0);
        map.deposit(e.pos, w, threat_range(e.kind), 0);
    }
    for b in &snapshot.visible_enemy_buildings {
        let w = building_threat(b.kind, b.health);
        map.deposit(b.pos, w, building_range(b.kind), 2);
    }
    for m in snapshot.remembered_troop_memory(MEMORY_FRESH_TICKS) {
        let Some(kind) = m.kind else { continue };
        let w = threat_unit_weight(kind, m.hp, m.age_ticks) * THREAT_MEMORY_DISCOUNT;
        map.deposit(m.pos, w, threat_range(kind), 1);
    }
    map
}

fn threat_unit_weight(kind: UnitKind, hp: f32, age_ticks: u64) -> f32 {
    if hp <= 0.0 {
        return 0.0;
    }
    combat_power(kind, hp) * age_decay(age_ticks)
}

/// 0.0.23 — cache threat per-team (Lewis: le query tattiche a bassa frequenza
/// non ricostruiscono mai). Chiave = snapshot tick: entro lo stesso tick la
/// mappa è bit-identica (stesso snapshot), al tick dopo si ricostruisce.
/// Riduce le build da ~5 a 1 per team per strategy-tick (`decide` + `executor`
/// condividono la stessa mappa). Vive in `AiState` (reset su `R`, niente slot
/// sistema extra). `BTreeMap` per determinismo d'iterazione.
#[derive(Default, Debug)]
pub struct ThreatCache {
    entries: BTreeMap<u8, (u64, ThreatMap)>,
}

impl ThreatCache {
    /// Mappa del team al tick: riusa se già costruita, altrimenti costruisce
    /// e memoizza. Il riferimento vive quanto il prestito della cache.
    pub fn get_or_build(&mut self, team: u8, tick: u64, snapshot: &AiSnapshot) -> &ThreatMap {
        let stale = self.entries.get(&team).is_none_or(|(t, _)| *t != tick);
        if stale {
            self.entries.insert(team, (tick, build_threat(snapshot)));
        }
        &self.entries.get(&team).expect("appena inserita").1
    }

    /// Lettura senza costruire: `Some` se il team ha una mappa fresca al tick
    /// (il micro la riusa gratis), `None` altrimenti (l'executor costruisce
    /// lazy solo se un intento Build/Scout la richiede davvero).
    pub fn get(&self, team: u8, tick: u64) -> Option<&ThreatMap> {
        self.entries
            .get(&team)
            .filter(|(t, _)| *t == tick)
            .map(|(_, m)| m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_map_is_zero_without_hotspot() {
        let snap = AiSnapshot {
            team: 1,
            ..Default::default()
        };
        let map = build_threat(&snap);
        assert_eq!(map.mean(), 0.0);
        assert_eq!(map.max(), 0.0);
        assert_eq!(map.hotspot(), None);
        assert_eq!(map.query(Vec3::ZERO), 0.0);
        // Fuori mappa: clamp al bordo, comunque 0.
        assert_eq!(map.query(Vec3::new(9999.0, 0.0, -9999.0)), 0.0);
    }

    #[test]
    fn confidence_matches_threat_decay() {
        // 0.0.19 — `memory::confidence` e `age_decay` stessa formula, stesso K:
        // percezione e threat non divergono mai.
        assert!((super::super::memory::CONFIDENCE_K - THREAT_DECAY_K).abs() < 1e-6);
        for age in [0u64, 1, 10, 60, 120, 240] {
            assert!(
                (super::super::memory::confidence(age) - age_decay(age)).abs() < 1e-6,
                "age {age}"
            );
        }
    }

    #[test]
    fn threat_decays_with_age() {
        assert!((age_decay(0) - 1.0).abs() < 1e-6);
        assert!(age_decay(60) < 1.0 && age_decay(60) > 0.0);
        assert!(age_decay(120) < age_decay(60));
        // Ricordo fresco pesa meno del visibile a pari kind/hp.
        let hp = crate::units::archetype(UnitKind::HeavyTank).max_health;
        let live = threat_unit_weight(UnitKind::HeavyTank, hp, 0);
        let remembered = threat_unit_weight(UnitKind::HeavyTank, hp, 60) * THREAT_MEMORY_DISCOUNT;
        assert!(remembered < live);
        assert!(remembered > 0.0);
    }

    #[test]
    fn threat_counts_turret_but_not_walls() {
        use crate::economy::balance::BuildingKind;
        assert!(building_threat(BuildingKind::Turret, 450.0) > 0.0);
        assert_eq!(building_threat(BuildingKind::Wall, 700.0), 0.0);
        assert_eq!(building_threat(BuildingKind::Metal, 350.0), 0.0);
        assert_eq!(building_threat(BuildingKind::Turret, 0.0), 0.0);
        // Engineer disarmato = 0 anche live.
        assert_eq!(threat_unit_weight(UnitKind::Engineer, 70.0, 0), 0.0);
        let tank = threat_unit_weight(UnitKind::HeavyTank, 170.0, 0);
        let scout = threat_unit_weight(UnitKind::Scout, 60.0, 0);
        assert!(tank > scout);
    }

    #[test]
    fn build_threat_spots_memory_and_hotspot_is_deterministic() {
        use super::super::snapshot::{AiEnemy, AiMemory};
        let mut snap = AiSnapshot {
            team: 1,
            tick: 10,
            ..Default::default()
        };
        snap.visible_enemies.push(AiEnemy {
            entity: Entity::from_bits(1),
            pos: Vec3::new(50.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: 170.0,
        });
        snap.memory.push(AiMemory {
            entity_bits: None,
            pos: Vec3::new(-80.0, 0.0, 20.0),
            age_ticks: 10,
            kind: Some(UnitKind::HeavyTank),
            hp: 170.0,
            building: false,
        });
        // Ricordo vecchio oltre TTL fresca: ignorato.
        snap.memory.push(AiMemory {
            entity_bits: None,
            pos: Vec3::new(0.0, 0.0, 100.0),
            age_ticks: MEMORY_FRESH_TICKS + 1,
            kind: Some(UnitKind::HeavyTank),
            hp: 170.0,
            building: false,
        });
        let a = build_threat(&snap);
        let b = build_threat(&snap);
        assert_eq!(a.cells, b.cells);
        assert!(a.max() > 0.0);
        // Hotspot sulla cella del visibile (peso maggiore del ricordo).
        let hot = a.hotspot().expect("deve esserci un hotspot");
        assert!((hot.x - 50.0).abs() < a.cell_side());
        // Il ricordo vecchio non ha lasciato traccia.
        assert_eq!(a.query(Vec3::new(0.0, 0.0, 100.0)), 0.0);
        // Ma il ricordo fresco sì.
        assert!(a.query(Vec3::new(-80.0, 0.0, 20.0)) > 0.0);
    }

    #[test]
    fn live_contact_is_not_counted_again_from_memory() {
        use super::super::snapshot::{AiEnemy, AiMemory};
        let entity = Entity::from_bits(77);
        let mut snap = AiSnapshot::default();
        snap.visible_enemies.push(AiEnemy {
            entity,
            pos: Vec3::ZERO,
            kind: UnitKind::HeavyTank,
            health: 170.0,
        });
        snap.memory.push(AiMemory {
            entity_bits: Some(entity.to_bits()),
            pos: Vec3::ZERO,
            age_ticks: 0,
            kind: Some(UnitKind::HeavyTank),
            hp: 170.0,
            building: false,
        });
        let expected = threat_unit_weight(UnitKind::HeavyTank, 170.0, 0);
        assert!((build_threat(&snap).max() - expected).abs() < 0.001);
    }

    #[test]
    fn spread_falls_off_with_distance() {
        // 0.0.23 — l'artiglieria (30m) proietta oltre la propria cella, con
        // falloff monotono; la cella propria resta il massimo esatto.
        // Sorgente al centro cella (celle da 18.75m): ortogonali a 18.75m,
        // diagonali a 26.5m, secondo anello a 37.5m (fuori gittata).
        use super::super::snapshot::AiEnemy;
        let center = Vec3::new(9.375, 0.0, 9.375);
        let mut snap = AiSnapshot::default();
        snap.visible_enemies.push(AiEnemy {
            entity: Entity::from_bits(5),
            pos: center,
            kind: UnitKind::Artillery,
            health: crate::units::archetype(UnitKind::Artillery).max_health,
        });
        let map = build_threat(&snap);
        let own = map.query(center);
        assert!(own > 0.0);
        assert_eq!(map.max(), own);
        // Vicino dentro gittata: qualcosa arriva; più lontano = meno.
        let orth = map.query(center + Vec3::new(18.75, 0.0, 0.0));
        let diag = map.query(center + Vec3::new(18.75, 0.0, 18.75));
        assert!(orth > 0.0 && orth < own, "{orth} vs {own}");
        assert!(diag > 0.0 && diag < orth, "{diag} vs {orth}");
        assert_eq!(map.query(Vec3::new(200.0, 0.0, 0.0)), 0.0);
        // Solo layer live popolato.
        let (live, rem, stat) = map.means();
        assert!(live > 0.0 && rem == 0.0 && stat == 0.0);
    }

    #[test]
    fn spread_touches_bounded_cells() {
        // 0.0.23 — bound strutturale (perf): una sorgente tocca al massimo il
        // bounding box (2*ceil(range/side)+1)^2 celle. Misura per profile.sh,
        // bound per unit test.
        use super::super::snapshot::AiEnemy;
        let mut snap = AiSnapshot::default();
        snap.visible_enemies.push(AiEnemy {
            entity: Entity::from_bits(6),
            pos: Vec3::ZERO,
            kind: UnitKind::Artillery,
            health: 100.0,
        });
        let map = build_threat(&snap);
        let side = (HALF_SIZE * 2.0) / THREAT_GRID_N as f32;
        let range = threat_range(UnitKind::Artillery);
        let side_cells = (range / side).ceil() as usize * 2 + 1;
        let bound = side_cells * side_cells;
        let touched = map.cells.iter().filter(|v| **v > 0.0).count();
        assert!(touched > 1 && touched <= bound, "{touched} vs {bound}");
    }

    #[test]
    fn layers_sum_to_combined() {
        // 0.0.23 — live + remembered + static = combinato, ovunque.
        use super::super::snapshot::{AiBuilding, AiEnemy, AiMemory};
        use crate::economy::balance::BuildingKind;
        let mut snap = AiSnapshot::default();
        snap.visible_enemies.push(AiEnemy {
            entity: Entity::from_bits(7),
            pos: Vec3::new(40.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: 120.0,
        });
        snap.visible_enemy_buildings.push(AiBuilding {
            entity: Entity::from_bits(8),
            kind: BuildingKind::Turret,
            pos: Vec3::new(-60.0, 0.0, 0.0),
            under_construction: false,
            health: 300.0,
        });
        snap.memory.push(AiMemory {
            entity_bits: None,
            pos: Vec3::new(0.0, 0.0, 80.0),
            age_ticks: 5,
            kind: Some(UnitKind::LightTank),
            hp: 80.0,
            building: false,
        });
        let map = build_threat(&snap);
        assert!(map.layers_sane());
        assert!(map.query_static(Vec3::new(-60.0, 0.0, 0.0)) > 0.0);
        // La statica non sporca il live altrove.
        assert_eq!(map.query_static(Vec3::new(40.0, 0.0, 0.0)), 0.0);
        let (live, rem, stat) = map.means();
        assert!(live > 0.0 && rem > 0.0 && stat > 0.0);
    }

    #[test]
    fn threat_ranges_come_from_tables() {
        // 0.0.23 — gittate da tabella, mai costanti: arty > tank, disarmati 0,
        // torrette 24/28, muri/eco 0.
        use crate::economy::balance::BuildingKind;
        assert_eq!(threat_range(UnitKind::Engineer), 0.0);
        assert!(threat_range(UnitKind::Artillery) > threat_range(UnitKind::HeavyTank));
        assert_eq!(building_range(BuildingKind::Turret), 24.0);
        assert_eq!(building_range(BuildingKind::Lance), 28.0);
        assert_eq!(building_range(BuildingKind::Wall), 0.0);
        assert_eq!(building_range(BuildingKind::Metal), 0.0);
    }

    #[test]
    fn cache_reuses_same_tick_and_rebuilds_on_new_tick() {
        // 0.0.23 — stesso tick = stesso puntatore (zero rebuild), tick nuovo =
        // rebuild col nuovo snapshot; get senza build = None se mai costruita.
        // (Il nodo BTreeMap può riusare l'indirizzo tra tick: si confrontano
        // i contenuti, mai i puntatori, sul rebuild.)
        use super::super::snapshot::AiEnemy;
        let mut snap = AiSnapshot {
            team: 2,
            tick: 40,
            ..Default::default()
        };
        snap.visible_enemies.push(AiEnemy {
            entity: Entity::from_bits(9),
            pos: Vec3::ZERO,
            kind: UnitKind::HeavyTank,
            health: 100.0,
        });
        let mut cache = super::ThreatCache::default();
        assert!(cache.get(2, 40).is_none());
        let a = cache.get_or_build(2, 40, &snap) as *const _;
        let b = cache.get_or_build(2, 40, &snap) as *const _;
        assert_eq!(a, b);
        let max_before = cache.get(2, 40).expect("memoizzata").max();
        assert!(max_before > 0.0);
        assert!(cache.get(2, 41).is_none());
        // Nuovo tick + secondo nemico: rebuild con contenuti nuovi.
        snap.tick = 41;
        snap.visible_enemies.push(AiEnemy {
            entity: Entity::from_bits(10),
            pos: Vec3::new(100.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: 100.0,
        });
        let max_after = cache.get_or_build(2, 41, &snap).max();
        assert!(max_after >= max_before);
        // Il rebuild vede il nuovo snapshot: il secondo tank c'è.
        assert!(
            cache
                .get(2, 41)
                .expect("fresca")
                .query(Vec3::new(100.0, 0.0, 0.0))
                > 0.0
        );
        assert!(cache.get(2, 40).is_none());
    }
}
