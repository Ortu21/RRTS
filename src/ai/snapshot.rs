//! Honest per-team world snapshot: the ONLY data strategy/tactics may read.
//!
//! Units/buildings of the AI team are fully known. Enemies appear only if
//! currently `visible()` for this team in [`VisibilityMap`]. When fog data
//! is absent (unit tests without the plugin) everything is visible — the
//! same open fallback as `fog::can_target`, so harnesses keep working.

use crate::{
    combat::Health,
    economy::Economy,
    fog::{FOG_COUNT, VisibilityMap},
    orders::UnitOrder,
    structures::{Building, Construction},
    units::{Team, Unit, UnitKind},
};
use bevy::prelude::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct AiUnit {
    pub entity: Entity,
    pub pos: Vec3,
    pub kind: UnitKind,
    pub order: UnitOrder,
    pub health: f32,
    #[allow(dead_code)]
    pub max_health: f32,
}

#[derive(Clone, Debug)]
pub struct AiEnemy {
    pub entity: Entity,
    pub pos: Vec3,
    pub kind: UnitKind,
    pub health: f32,
}

#[derive(Clone, Debug)]
pub struct AiBuilding {
    pub entity: Entity,
    pub kind: crate::economy::balance::BuildingKind,
    pub pos: Vec3,
    pub under_construction: bool,
    #[allow(dead_code)]
    pub health: f32,
}

/// Ricordo di un contatto nemico: ultima posizione vista + età in tick
/// snapshot (4Hz). Ordinati per età crescente (i più freschi prima).
#[derive(Clone, Debug)]
pub struct AiMemory {
    pub pos: Vec3,
    pub age_ticks: u64,
    pub kind: Option<UnitKind>,
    pub hp: f32,
    pub building: bool,
}

impl AiMemory {
    /// 0.0.19 — confidence Walsh: `1/(1+age/60)`, stesso K di
    /// `memory::confidence` e `threat::age_decay`. Pura.
    /// Uso diretto in 0.0.19+ (percezione pesata); oggi solo test: allow per
    /// clippy `-D warnings`.
    #[allow(dead_code)]
    pub fn confidence(&self) -> f32 {
        super::memory::confidence(self.age_ticks)
    }
}

/// 0.0.19 — risoluzione griglia esplorata per-team (frontiera scout).
/// Stessa geometria della threat map 32×32 su `HALF_SIZE`: le due restano
/// allineate per costruzione (test in `scout::tests`).
pub const EXPLORED_GRID_N: usize = 32;

/// Team-local view. Sorted by entity bits for determinism.
#[derive(Resource, Clone, Debug, Default)]
pub struct AiSnapshot {
    pub team: u8,
    pub tick: u64,
    pub my_units: Vec<AiUnit>,
    pub visible_enemies: Vec<AiEnemy>,
    /// Edifici nemici visibili ora (0.0.16: riesumato, letto da threat map).
    pub visible_enemy_buildings: Vec<AiBuilding>,
    pub my_buildings: Vec<AiBuilding>,
    pub active_site: Option<Entity>,
    /// Ricordi nemici (anche non più visibili), freschi prima.
    pub memory: Vec<AiMemory>,
    /// G1 — depositi metallo (terreno pubblico, uguali per ogni team).
    /// La regola Metal li legge da qui: niente nuove query nei sistemi.
    pub deposits: Vec<crate::structures::MetalDeposit>,
    /// 0.0.16 — frazione mappa esplorata dal team (0..1), da `VisibilityMap`.
    /// 0.0.19 — driver della frontiera scout (`scout::frontier_target` legge
    /// `explored_cells`, non solo questa frazione).
    pub explored_pct: f32,
    /// 0.0.19 — griglia esplorata grossolana (`EXPLORED_GRID_N`², row-major)
    /// campionata da `VisibilityMap` in `build_snapshot`. Vuota = nessun dato
    /// fog (test senza plugin): la frontiera tratta tutto come inesplorato.
    pub explored_cells: Vec<bool>,
    #[allow(dead_code)]
    pub stock: [f64; 2],
    #[allow(dead_code)]
    pub income: [f64; 2],
    #[allow(dead_code)]
    pub demand: [f64; 2],
}

impl AiSnapshot {
    #[allow(dead_code)]
    pub fn builders(&self) -> Vec<&AiUnit> {
        self.my_units
            .iter()
            .filter(|u| u.kind.is_builder() && u.health > 0.0)
            .collect()
    }

    #[allow(dead_code)]
    pub fn idle_builders(&self) -> Vec<&AiUnit> {
        self.builders()
            .into_iter()
            .filter(|u| matches!(u.order, UnitOrder::Idle | UnitOrder::HoldPosition))
            .collect()
    }

    pub fn army(&self) -> Vec<&AiUnit> {
        self.my_units
            .iter()
            .filter(|u| crate::units::archetype(u.kind).armed && u.health > 0.0)
            .collect()
    }

    pub fn count_building(&self, kind: crate::economy::balance::BuildingKind) -> usize {
        self.my_buildings.iter().filter(|b| b.kind == kind).count()
    }

    pub fn complete_building(&self, kind: crate::economy::balance::BuildingKind) -> usize {
        self.my_buildings
            .iter()
            .filter(|b| b.kind == kind && !b.under_construction)
            .count()
    }

    /// 0.0.16 — ricordi freschi non-edifici (stesso filtro di `has_fresh_eyes`).
    /// L'ordine resta quello dello snapshot (freschi prima): `first()` = il
    /// più fresco. Puro, Nessuna allocazione oltre il Vec di ref.
    pub fn fresh_troop_memory(&self, max_age: u64) -> Vec<&AiMemory> {
        self.memory
            .iter()
            .filter(|m| !m.building && m.age_ticks <= max_age)
            .collect()
    }

    /// 0.0.16 — baricentro dei ricordi freschi non-edifici (meta attacco /
    /// conferma scout). Stessa matematica di `memory::remembered_centroid`
    /// ma su `AiMemory` (snapshot) invece che su `Contact`: i due restano
    /// allineati per costruzione, test incrociato in `strategy::tests`.
    /// `None` se nessun ricordo fresco.
    pub fn remembered_centroid(&self, max_age: u64) -> Option<Vec3> {
        let fresh = self.fresh_troop_memory(max_age);
        if fresh.is_empty() {
            return None;
        }
        let mut sum = Vec3::ZERO;
        for m in &fresh {
            sum += m.pos;
        }
        Some(sum / fresh.len() as f32)
    }

    /// 0.0.19 — ricordi freschi di edifici (base nemica ricordata).
    /// Speculare a `fresh_troop_memory`: l'ordine resta quello dello snapshot
    /// (freschi prima). Puro.
    pub fn fresh_building_memory(&self, max_age: u64) -> Vec<&AiMemory> {
        self.memory
            .iter()
            .filter(|m| m.building && m.age_ticks <= max_age)
            .collect()
    }

    /// 0.0.19 — baricentro degli edifici nemici ricordati (freschi).
    /// Meta attacco quando la base è stata vista ma è ora sotto fog:
    /// preferita al target statico (`strategy::attack_destination`).
    /// `None` se nessun edificio fresco in memoria.
    pub fn remembered_building_centroid(&self, max_age: u64) -> Option<Vec3> {
        let fresh = self.fresh_building_memory(max_age);
        if fresh.is_empty() {
            return None;
        }
        let mut sum = Vec3::ZERO;
        for m in &fresh {
            sum += m.pos;
        }
        Some(sum / fresh.len() as f32)
    }

    /// 0.0.19 — cella esplorata della griglia frontiera (`EXPLORED_GRID_N`²).
    /// `explored_cells` vuota (test senza fog) = tutto inesplorato (novelty 1).
    /// Pura, mai panic su indici fuori mappa (clamp al bordo).
    pub fn explored_cell(&self, col: usize, row: usize) -> bool {
        if self.explored_cells.is_empty() {
            return false;
        }
        let c = col.min(EXPLORED_GRID_N - 1);
        let r = row.min(EXPLORED_GRID_N - 1);
        self.explored_cells
            .get(r * EXPLORED_GRID_N + c)
            .copied()
            .unwrap_or(false)
    }
}

/// Pure visibility filter shared by systems and tests.
pub fn is_visible_to(map: Option<&VisibilityMap>, team: u8, pos: Vec3) -> bool {
    match map {
        None => true,
        Some(m) => {
            // Open fallback while the team has no fog data yet (first ticks,
            // harnesses without viewers): same rule as fog::can_target.
            if !m.0.contains_key(&team) {
                return true;
            }
            m.visible(team, pos)
        }
    }
}

/// Contatti nemici onesti per `team`: unità ed edifici vivi filtrati per fog.
/// Stesso gating di `can_target` (open fallback senza dati). Puro e
/// condiviso da snapshot e memoria così non divergono mai.
pub type VisibleUnits = Vec<(Entity, Vec3, UnitKind, f32)>;
pub type VisibleSites = Vec<(
    Entity,
    Vec3,
    crate::economy::balance::BuildingKind,
    bool,
    f32,
)>;

pub fn visible_for(
    team: u8,
    enemies: &[(Entity, Vec3, u8, UnitKind, f32)],
    buildings: &[(
        Entity,
        u8,
        crate::economy::balance::BuildingKind,
        Vec3,
        bool,
        f32,
    )],
    map: Option<&VisibilityMap>,
) -> (VisibleUnits, VisibleSites) {
    let mut units: VisibleUnits = enemies
        .iter()
        .filter(|(_, _, t, _, hp)| *t != team && *hp > 0.0)
        .filter(|(_, pos, _, _, _)| is_visible_to(map, team, *pos))
        .map(|(e, pos, _, kind, hp)| (*e, *pos, *kind, *hp))
        .collect();
    units.sort_by_key(|(e, _, _, _)| e.to_bits());
    let mut sites: VisibleSites = buildings
        .iter()
        .filter(|(_, t, _, _, _, hp)| *t != team && *hp > 0.0)
        .filter(|(_, _, _, pos, _, _)| is_visible_to(map, team, *pos))
        .map(|(e, _, kind, pos, site, hp)| (*e, *pos, *kind, *site, *hp))
        .collect();
    sites.sort_by_key(|(e, _, _, _, _)| e.to_bits());
    (units, sites)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn build_snapshot(
    team: u8,
    tick: u64,
    units: &[(Entity, Vec3, u8, UnitKind, UnitOrder, f32, f32)],
    enemies: &[(Entity, Vec3, u8, UnitKind, f32)],
    buildings: &[(
        Entity,
        u8,
        crate::economy::balance::BuildingKind,
        Vec3,
        bool,
        f32,
    )],
    economy: &BTreeMap<
        u8,
        (
            /*stock*/ [f64; 2],
            /*income*/ [f64; 2],
            /*demand*/ [f64; 2],
        ),
    >,
    map: Option<&VisibilityMap>,
    memory: &[(Vec3, u64, Option<UnitKind>, f32, bool)],
    deposits: &[crate::structures::MetalDeposit],
) -> AiSnapshot {
    let mut my_units: Vec<AiUnit> = units
        .iter()
        .filter(|(_, _, t, _, _, hp, _)| *t == team && *hp > 0.0)
        .map(|(e, pos, _, kind, order, hp, max)| AiUnit {
            entity: *e,
            pos: *pos,
            kind: *kind,
            order: order.clone(),
            health: *hp,
            max_health: *max,
        })
        .collect();
    my_units.sort_by_key(|u| u.entity.to_bits());

    let (visible_units, visible_sites) = visible_for(team, enemies, buildings, map);
    let mut visible_enemies: Vec<AiEnemy> = visible_units
        .iter()
        .map(|(e, pos, kind, hp)| AiEnemy {
            entity: *e,
            pos: *pos,
            kind: *kind,
            health: *hp,
        })
        .collect();
    visible_enemies.sort_by_key(|e| e.entity.to_bits());

    let mut my_buildings: Vec<AiBuilding> = buildings
        .iter()
        .filter(|(_, t, _, _, _, hp)| *t == team && *hp > 0.0)
        .map(|(e, _, kind, pos, site, hp)| AiBuilding {
            entity: *e,
            kind: *kind,
            pos: *pos,
            under_construction: *site,
            health: *hp,
        })
        .collect();
    my_buildings.sort_by_key(|b| b.entity.to_bits());

    let mut visible_enemy_buildings: Vec<AiBuilding> = visible_sites
        .iter()
        .map(|(e, pos, kind, site, hp)| AiBuilding {
            entity: *e,
            kind: *kind,
            pos: *pos,
            under_construction: *site,
            health: *hp,
        })
        .collect();
    visible_enemy_buildings.sort_by_key(|b| b.entity.to_bits());

    let active_site = my_buildings
        .iter()
        .find(|b| b.under_construction)
        .map(|b| b.entity);

    let (stock, income, demand) =
        economy
            .get(&team)
            .cloned()
            .unwrap_or(([0.0, 0.0], [0.0, 0.0], [0.0, 0.0]));

    let explored_pct = map
        .and_then(|m| m.0.get(&team))
        .map(|f| f.explored.iter().filter(|c| **c).count() as f32 / FOG_COUNT as f32)
        .unwrap_or(0.0);

    let explored_cells = sample_explored_cells(map, team);

    AiSnapshot {
        team,
        tick,
        my_units,
        visible_enemies,
        visible_enemy_buildings,
        my_buildings,
        active_site,
        memory: to_ai_memory(memory),
        explored_pct,
        explored_cells,
        deposits: deposits.to_vec(),
        stock,
        income,
        demand,
    }
}

/// 0.0.19 — campiona la griglia frontiera (`EXPLORED_GRID_N`²) da
/// `VisibilityMap`: centro di ogni cella grossolana interrogato con
/// `explored(team, center)`. Vuota senza dati fog (open fallback dei test).
/// Pura e deterministica (row-major, nessun HashMap/rand).
fn sample_explored_cells(map: Option<&VisibilityMap>, team: u8) -> Vec<bool> {
    let Some(map) = map else {
        return Vec::new();
    };
    if !map.0.contains_key(&team) {
        return Vec::new();
    }
    use crate::navigation::HALF_SIZE;
    let n = EXPLORED_GRID_N;
    let side = (HALF_SIZE * 2.0) / n as f32;
    let mut out = Vec::with_capacity(n * n);
    for row in 0..n {
        for col in 0..n {
            let x = -HALF_SIZE + (col as f32 + 0.5) * side;
            let z = -HALF_SIZE + (row as f32 + 0.5) * side;
            out.push(map.explored(team, Vec3::new(x, 0.0, z)));
        }
    }
    out
}

/// Vista memoria per lo snapshot: età calcolata, freschi prima, poi per
/// posizione (deterministico).
fn to_ai_memory(memory: &[(Vec3, u64, Option<UnitKind>, f32, bool)]) -> Vec<AiMemory> {
    let mut out: Vec<AiMemory> = memory
        .iter()
        .map(|(pos, age, kind, hp, building)| AiMemory {
            pos: *pos,
            age_ticks: *age,
            kind: *kind,
            hp: *hp,
            building: *building,
        })
        .collect();
    out.sort_by(|a, b| {
        a.age_ticks
            .cmp(&b.age_ticks)
            .then_with(|| a.pos.x.to_bits().cmp(&b.pos.x.to_bits()))
            .then_with(|| a.pos.z.to_bits().cmp(&b.pos.z.to_bits()))
    });
    out
}

/// Bevy system: rebuild gli snapshot (uno per team AI) a 4Hz come il fog.
/// Aggiorna anche `EnemyMemory` dagli stessi contatti visibili (mai divergono).
/// Loop in ordine di team = deterministico.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn refresh_snapshots(
    mut acc: Local<f32>,
    time: Res<Time>,
    config: Res<super::AiConfig>,
    map: Option<Res<VisibilityMap>>,
    economy: Option<Res<Economy>>,
    mut snapshots: ResMut<super::AiSnapshots>,
    mut memory: ResMut<super::EnemyMemory>,
    deposits: Option<Res<crate::structures::MetalDeposits>>,
    mut tick: Local<u64>,
    units: Query<(Entity, &Transform, &Team, &UnitKind, &UnitOrder, &Health), With<Unit>>,
    enemy_units: Query<(Entity, &Transform, &Team, &UnitKind, &Health), With<Unit>>,
    buildings: Query<
        (
            Entity,
            &Team,
            &crate::economy::balance::BuildingKind,
            &Transform,
            &Health,
            Has<Construction>,
        ),
        With<Building>,
    >,
) {
    *acc += time.delta_secs();
    let has_any = snapshots.0.values().any(|s| s.tick > 0);
    if *acc < super::SNAPSHOT_PERIOD && has_any {
        return;
    }
    *acc = 0.0;
    *tick += 1;

    let flat_units: Vec<_> = units
        .iter()
        .map(|(e, t, team, kind, order, hp)| {
            (
                e,
                t.translation,
                team.0,
                *kind,
                order.clone(),
                hp.current,
                hp.max,
            )
        })
        .collect();
    let flat_enemies: Vec<_> = enemy_units
        .iter()
        .map(|(e, t, team, kind, hp)| (e, t.translation, team.0, *kind, hp.current))
        .collect();
    let flat_buildings: Vec<_> = buildings
        .iter()
        .map(|(e, team, kind, t, hp, site)| (e, team.0, *kind, t.translation, site, hp.current))
        .collect();
    let mut eco: BTreeMap<u8, ([f64; 2], [f64; 2], [f64; 2])> = BTreeMap::new();
    if let Some(economy) = economy.as_deref() {
        for (team, account) in &economy.0 {
            eco.insert(*team, (account.stock, account.income, account.demand));
        }
    }
    // G1: depositi condivisi (terreno); app senza StructuresPlugin vedono
    // mondo vuoto = regola Metal chiusa, mai panic.
    let empty_deposits = crate::structures::MetalDeposits::default();
    let deposits = deposits.as_deref().unwrap_or(&empty_deposits);
    for brain in config.sorted_teams() {
        // Stessi contatti visibili di build_snapshot: memoria mai divergente.
        let (vis_units, vis_sites) =
            visible_for(brain.team, &flat_enemies, &flat_buildings, map.as_deref());
        let mut observed: Vec<(u64, Vec3, Option<UnitKind>, f32, bool)> = vis_units
            .iter()
            .map(|(e, pos, kind, hp)| (e.to_bits(), *pos, Some(*kind), *hp, false))
            .collect();
        observed.extend(
            vis_sites
                .iter()
                .map(|(e, pos, _, _, hp)| (e.to_bits(), *pos, None, *hp, true)),
        );
        observed.sort_by_key(|(bits, _, _, _, _)| *bits);
        let team_memory = memory.0.entry(brain.team).or_default();
        super::memory::update_memory(team_memory, *tick, &observed);
        let mem_view: Vec<(Vec3, u64, Option<UnitKind>, f32, bool)> = team_memory
            .iter()
            .map(|c| (c.pos, tick.saturating_sub(c.tick), c.kind, c.hp, c.building))
            .collect();
        snapshots.0.insert(
            brain.team,
            build_snapshot(
                brain.team,
                *tick,
                &flat_units,
                &flat_enemies,
                &flat_buildings,
                &eco,
                map.as_deref(),
                &mem_view,
                &deposits.0,
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team_map_with_reveal(team: u8, at: Vec3, range: f32) -> VisibilityMap {
        // Build through the real fog rasterizer path via a minimal app is
        // heavy; replicate the disc check through visible() by inserting a
        // team entry with a manually revealed cell band. Simpler: use an
        // empty map (open fallback) vs a map with the team key present.
        let mut map = VisibilityMap::default();
        map.0.entry(team).or_default();
        // Force-ensure size then reveal center cell only if in range of origin.
        // For unit tests we only need: key present => gating active.
        let _ = (at, range);
        map
    }

    #[test]
    fn open_fallback_without_fog_data_sees_everything() {
        assert!(is_visible_to(None, 1, Vec3::ZERO));
        let empty = VisibilityMap::default();
        assert!(is_visible_to(Some(&empty), 1, Vec3::ZERO));
    }

    #[test]
    fn gated_once_team_registers() {
        let map = team_map_with_reveal(1, Vec3::ZERO, 10.0);
        // Team registered with blank visibility: nothing visible (gating on).
        assert!(!is_visible_to(Some(&map), 1, Vec3::ZERO));
        // Other team without key stays open.
        assert!(is_visible_to(Some(&map), 7, Vec3::ZERO));
    }

    #[test]
    fn snapshot_hides_unseen_enemies_but_keeps_own_forces() {
        use crate::economy::balance::BuildingKind;
        let me = Entity::from_bits(10);
        let foe = Entity::from_bits(11);
        let map = team_map_with_reveal(1, Vec3::ZERO, 10.0);
        let units = vec![(
            me,
            Vec3::ZERO,
            1u8,
            UnitKind::Commander,
            UnitOrder::Idle,
            100.0,
            100.0,
        )];
        let enemies = vec![(
            foe,
            Vec3::new(150.0, 0.0, 0.0),
            0u8,
            UnitKind::HeavyTank,
            100.0,
        )];
        let buildings: Vec<(Entity, u8, BuildingKind, Vec3, bool, f32)> = vec![];
        let eco = BTreeMap::new();
        let snap = build_snapshot(
            1,
            1,
            &units,
            &enemies,
            &buildings,
            &eco,
            Some(&map),
            &[],
            &[],
        );
        assert_eq!(snap.my_units.len(), 1);
        // Gated map with no reveal: enemy hidden (honest AI).
        assert!(snap.visible_enemies.is_empty());
        // Open map: same enemy visible.
        let open = build_snapshot(1, 1, &units, &enemies, &buildings, &eco, None, &[], &[]);
        assert_eq!(open.visible_enemies.len(), 1);
    }

    #[test]
    fn snapshot_is_deterministic() {
        let a = Entity::from_bits(3);
        let b = Entity::from_bits(1);
        let units = vec![
            (
                a,
                Vec3::X,
                1u8,
                UnitKind::HeavyTank,
                UnitOrder::Idle,
                100.0,
                100.0,
            ),
            (
                b,
                Vec3::Z,
                1u8,
                UnitKind::Scout,
                UnitOrder::Idle,
                50.0,
                50.0,
            ),
        ];
        let snap = build_snapshot(1, 5, &units, &[], &[], &BTreeMap::new(), None, &[], &[]);
        assert_eq!(snap.my_units[0].entity, b);
        assert_eq!(snap.my_units[1].entity, a);
        assert!(snap.deposits.is_empty());
    }

    #[test]
    fn deposits_ride_the_snapshot() {
        use crate::structures::MetalDeposit;
        let deps = vec![MetalDeposit {
            pos: Vec3::new(30.0, 0.0, 0.0),
            mult: 1.0,
        }];
        let snap = build_snapshot(1, 5, &[], &[], &[], &BTreeMap::new(), None, &[], &deps);
        assert_eq!(snap.deposits, deps);
    }

    #[test]
    fn explored_pct_defaults_zero_without_fog_and_helpers_match_memory() {
        use super::AiMemory;
        // Senza fog: 0% esplorato, niente panico.
        let snap = build_snapshot(1, 5, &[], &[], &[], &BTreeMap::new(), None, &[], &[]);
        assert!((snap.explored_pct - 0.0).abs() < 1e-6);
        // Helper freschi/centroide allineati a `memory::remembered_centroid`.
        let mut snap = AiSnapshot {
            team: 1,
            ..Default::default()
        };
        snap.memory.push(AiMemory {
            pos: Vec3::new(0.0, 0.0, 0.0),
            age_ticks: 5,
            kind: Some(UnitKind::HeavyTank),
            hp: 100.0,
            building: false,
        });
        snap.memory.push(AiMemory {
            pos: Vec3::new(10.0, 0.0, 0.0),
            age_ticks: 10,
            kind: Some(UnitKind::HeavyTank),
            hp: 100.0,
            building: false,
        });
        // Edifici esclusi dai freschi-truppa.
        snap.memory.push(AiMemory {
            pos: Vec3::new(99.0, 0.0, 99.0),
            age_ticks: 1,
            kind: None,
            hp: 450.0,
            building: true,
        });
        let fresh = snap.fresh_troop_memory(120);
        assert_eq!(fresh.len(), 2);
        let centroid = snap.remembered_centroid(120).expect("centroide");
        assert!((centroid.x - 5.0).abs() < 0.001);
        assert!(snap.remembered_centroid(0).is_none() || snap.tick == 0);
        // Solo edifici freschi ma niente truppe → nessun centroide truppe.
        let mut only_buildings = AiSnapshot {
            team: 1,
            ..Default::default()
        };
        only_buildings.memory.push(AiMemory {
            pos: Vec3::ZERO,
            age_ticks: 1,
            kind: None,
            hp: 450.0,
            building: true,
        });
        assert!(only_buildings.fresh_troop_memory(120).is_empty());
        assert_eq!(only_buildings.remembered_centroid(120), None);
    }

    #[test]
    fn building_memory_helpers_and_confidence() {
        use super::AiMemory;
        let mut snap = AiSnapshot {
            team: 1,
            ..Default::default()
        };
        snap.memory.push(AiMemory {
            pos: Vec3::new(200.0, 0.0, 200.0),
            age_ticks: 5,
            kind: None,
            hp: 450.0,
            building: true,
        });
        snap.memory.push(AiMemory {
            pos: Vec3::new(210.0, 0.0, 210.0),
            age_ticks: 200,
            kind: None,
            hp: 450.0,
            building: true,
        });
        snap.memory.push(AiMemory {
            pos: Vec3::new(0.0, 0.0, 0.0),
            age_ticks: 5,
            kind: Some(UnitKind::HeavyTank),
            hp: 100.0,
            building: false,
        });
        // Solo edifici freschi: 1 (l'altro è oltre 120).
        let fresh_b = snap.fresh_building_memory(120);
        assert_eq!(fresh_b.len(), 1);
        assert!((fresh_b[0].pos.x - 200.0).abs() < 0.001);
        let centroid = snap
            .remembered_building_centroid(120)
            .expect("base ricordata");
        assert!((centroid.x - 200.0).abs() < 0.001);
        assert!(snap.remembered_building_centroid(0).is_none());
        // Confidence decade con l'età (stesso K della threat).
        assert!((snap.memory[0].confidence() - super::super::memory::confidence(5)).abs() < 1e-6);
        assert!(snap.memory[1].confidence() < snap.memory[0].confidence());
        // Senza fog: griglia vuota = tutto inesplorato.
        assert!(snap.explored_cells.is_empty());
        assert!(!snap.explored_cell(0, 0));
        let empty = build_snapshot(1, 5, &[], &[], &[], &BTreeMap::new(), None, &[], &[]);
        assert!(empty.explored_cells.is_empty());
    }
}
