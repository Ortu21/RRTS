//! TestDirector: assert funzionali (fail = nonzero exit) + metriche
//! bilanciamento (descrittive con soglie morbide). Valutati nell'harness
//! headless leggendo il World dopo ogni tick — stesso pattern di
//! `economy/benchmark.rs`, nessun sistema Bevy aggiuntivo.

use crate::{
    combat::Health,
    economy::{Economy, balance::BuildingKind},
    orders::UnitOrder,
    production::Factory,
    structures::{Building, Construction},
    units::{Team, Unit, UnitKind},
};
use bevy::prelude::*;

#[derive(Clone, Debug)]
pub struct CheckResult {
    pub name: &'static str,
    pub pass: bool,
    pub detail: String,
}

pub struct AiTestReport {
    pub checks: Vec<CheckResult>,
    #[allow(dead_code)]
    pub ticks: usize,
    pub checksum: u64,
    pub units_team: usize,
    pub buildings_team: usize,
    pub stocks: [f64; 2],
    pub income: [f64; 2],
    pub orders_issued: u64,
    pub builds_done: u64,
    pub enqueues_done: u64,
    pub nav_failed: usize,
    #[allow(dead_code)]
    pub kills: usize,
}

impl AiTestReport {
    pub fn all_pass(&self) -> bool {
        self.checks.iter().all(|c| c.pass)
    }
}

fn count_buildings(world: &mut World, team: u8, kind: BuildingKind, complete_only: bool) -> usize {
    world
        .query_filtered::<(&Team, &BuildingKind, Has<Construction>), With<Building>>()
        .iter(world)
        .filter(|(t, k, _)| t.0 == team && **k == kind)
        .filter(|(_, _, site)| !complete_only || !site)
        .count()
}

fn count_units(world: &mut World, team: u8, kind: Option<UnitKind>) -> usize {
    world
        .query_filtered::<(&Team, &UnitKind, &Health), With<Unit>>()
        .iter(world)
        .filter(|(t, k, h)| t.0 == team && h.current > 0.0 && kind.is_none_or(|w| **k == w))
        .count()
}

/// Snapshot funzionale a fine run per il team AI.
pub fn evaluate_functional(world: &mut World, team: u8, ticks: usize) -> Vec<CheckResult> {
    let mut checks = Vec::new();

    let metal_total = count_buildings(world, team, BuildingKind::Metal, false);
    let metal_done = count_buildings(world, team, BuildingKind::Metal, true);
    checks.push(CheckResult {
        name: "build-metal-site-spawned",
        pass: metal_total >= 1,
        detail: format!("metal sites total={metal_total}"),
    });
    checks.push(CheckResult {
        name: "build-metal-completed",
        pass: metal_done >= 1,
        detail: format!("metal complete={metal_done}/{metal_total}"),
    });

    let solar_done = count_buildings(world, team, BuildingKind::Solar, true);
    checks.push(CheckResult {
        name: "build-solar-completed",
        pass: solar_done >= 1,
        detail: format!("solar complete={solar_done}"),
    });

    let factory_done = count_buildings(world, team, BuildingKind::Factory, true);
    checks.push(CheckResult {
        name: "build-factory-completed",
        pass: factory_done >= 1,
        detail: format!("factory complete={factory_done}"),
    });

    let combat_units = world
        .query_filtered::<(&Team, &UnitKind, &Health), With<Unit>>()
        .iter(world)
        .filter(|(t, k, h)| t.0 == team && h.current > 0.0 && crate::units::archetype(**k).armed)
        .count();
    let engineers = count_units(world, team, Some(UnitKind::Engineer));
    let produced = combat_units + engineers;
    checks.push(CheckResult {
        name: "factory-produced-unit",
        pass: produced >= 1,
        detail: format!("combat={combat_units} engineers={engineers}"),
    });

    // Anti-bypass tier: mai unità T2 senza LabT2 completo.
    let labt2_done = count_buildings(world, team, BuildingKind::LabT2, true);
    let t2_units = [UnitKind::HeavyTank2, UnitKind::Artillery2]
        .into_iter()
        .map(|k| count_units(world, team, Some(k)))
        .sum::<usize>();
    checks.push(CheckResult {
        name: "no-t2-without-labt2",
        pass: labt2_done > 0 || t2_units == 0,
        detail: format!("labt2 complete={labt2_done} t2 units={t2_units}"),
    });

    // Economia: un Metal completo deve alzare l'income oltre lo zero.
    let income_metal = world
        .resource::<Economy>()
        .0
        .get(&team)
        .map(|a| a.income[0])
        .unwrap_or(0.0);
    checks.push(CheckResult {
        name: "metal-income-flows",
        pass: income_metal >= 4.9,
        detail: format!("metal income={income_metal:.2}/s"),
    });

    // Fog honesty: nessun ordine Attack diretto verso un nemico invisibile.
    // (L'AI usa AttackMove; questo catches future regression che barano.)
    let fog_fail = world
        .query_filtered::<(&Team, &UnitOrder), With<Unit>>()
        .iter(world)
        .filter(|(t, _)| t.0 == team)
        .filter_map(|(_, o)| match o {
            UnitOrder::Attack { target } => Some(*target),
            _ => None,
        })
        .any(|target| {
            let pos = world.get::<Transform>(target).map(|t| t.translation);
            let map = world.get_resource::<crate::fog::VisibilityMap>();
            match (pos, map) {
                (Some(p), Some(m)) => !m.visible(team, p),
                _ => false,
            }
        });
    checks.push(CheckResult {
        name: "fog-honest-no-blind-focus-fire",
        pass: !fog_fail,
        detail: if fog_fail {
            "AI issued Attack on unseen target".to_owned()
        } else {
            "no blind Attack orders".to_owned()
        },
    });

    // Pathfinding: zero failure è il contratto dei workload esistenti.
    let failed = world
        .resource::<crate::navigation::NavigationStats>()
        .failed;
    checks.push(CheckResult {
        name: "nav-zero-failures",
        pass: failed == 0,
        detail: format!("failed={failed} ticks={ticks}"),
    });

    checks
}

/// Metriche bilanciamento: descrittive, con soglie morbide anti-flake.
pub fn evaluate_balance(world: &mut World, team: u8) -> Vec<CheckResult> {
    let mut checks = Vec::new();
    let foe = 1 - team;
    let my_army = count_units(world, team, None);
    let foe_army = count_units(world, foe, None);
    // Il nemico (team 0, senza AI in harness) resta col solo Commander:
    // l'AI deve aver espanso l'esercito oltre il singolo Commander iniziale.
    checks.push(CheckResult {
        name: "balance-army-expanded",
        pass: my_army >= 2,
        detail: format!("ai army={my_army} foe army={foe_army}"),
    });
    let stocks = world
        .resource::<Economy>()
        .0
        .get(&team)
        .map(|a| a.stock)
        .unwrap_or([0.0, 0.0]);
    let sane = stocks.iter().all(|v| v.is_finite() && *v >= 0.0);
    checks.push(CheckResult {
        name: "balance-stocks-sane",
        pass: sane,
        detail: format!("stocks=[{:.0},{:.0}]", stocks[0], stocks[1]),
    });
    // 0.0.16 — threat shadow sana: costruita dallo snapshot onesto, deve
    // essere finita e >= 0. Solo osservazione: mai fail su valori alti
    // (un threat alto è informazione, non errore).
    let (threat_mean, threat_max, explored) = world
        .get_resource::<super::AiSnapshots>()
        .and_then(|s| s.0.get(&team))
        .map(|snap| {
            let map = super::threat::build_threat(snap);
            (map.mean(), map.max(), snap.explored_pct)
        })
        .unwrap_or((0.0, 0.0, 0.0));
    let threat_sane = threat_mean.is_finite()
        && threat_max.is_finite()
        && threat_mean >= 0.0
        && threat_max >= 0.0
        && explored.is_finite()
        && (0.0..=1.0).contains(&explored);
    checks.push(CheckResult {
        name: "threat-sane",
        pass: threat_sane,
        detail: format!(
            "threat mean={threat_mean:.1} max={threat_max:.1} explored={:.1}%",
            explored * 100.0
        ),
    });
    checks
}

/// Checksum deterministico posizioni+hp (stesso schema dei benchmark).
pub fn state_checksum(world: &mut World) -> u64 {
    let mut checksum = 0xcbf29ce484222325_u64;
    let mut units: Vec<(u32, Vec3, u32)> = world
        .query::<(&Unit, &Transform, &Health)>()
        .iter(world)
        .map(|(u, t, h)| (u.0, t.translation, h.current.to_bits()))
        .collect();
    units.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.x.to_bits().cmp(&b.1.x.to_bits()))
            .then(a.1.z.to_bits().cmp(&b.1.z.to_bits()))
            .then(a.2.cmp(&b.2))
    });
    for (id, pos, hp) in units {
        for value in [
            id as u64,
            pos.x.to_bits() as u64,
            pos.z.to_bits() as u64,
            hp as u64,
        ] {
            checksum = (checksum ^ value).wrapping_mul(0x100000001b3);
        }
    }
    checksum
}

/// Query factory con coda per la strategia (vista completa), ordinate.
#[allow(dead_code)]
pub fn factory_queues(world: &mut World, team: u8) -> Vec<crate::ai::strategy::FactoryView> {
    let mut out: Vec<_> = world
        .query_filtered::<(Entity, &Team, &Factory), (With<Building>, Without<Construction>)>()
        .iter(world)
        .filter(|(_, t, _)| t.0 == team)
        .map(|(e, _, f)| crate::ai::strategy::FactoryView {
            entity: e,
            queue_len: f.queue.len(),
            blocked: f.blocked,
            tier: f.tier,
            queued: f.queue.iter().map(|job| job.kind).collect(),
        })
        .collect();
    out.sort_by_key(|v| v.entity.to_bits());
    out
}
