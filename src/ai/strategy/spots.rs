//! Utility strategy + tactics (pure, testable).
//!
//! Decisioni data-driven: ogni opzione ha scorer 0..1, la personalità pesa
//! le soglie. Aggiungere unità/edifici futuri = nuove righe in tabella +
//! pesi in [`Personality`], mai `if kind == X` nel core.

//! Spot: wall/metal/build/rally — validati su NavGrid.

use crate::{
    economy::balance::BuildingKind,
    scenario::Scenario,
    structures::{
        building_obstacle, factory_spawn_ok, placement_rule, site_approach, snap_to_grid,
        valid_ground,
    },
};
use bevy::prelude::*;

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
    (grid.is_walkable(repaired) && grid.has_clearance_for(repaired, 0.5)).then_some(repaired)
}

/// 0.0.18 — slot muro davanti alla torretta verso la minaccia: 3
/// caselle perpendicolari a 10m, snappate alla build grid (schermo che
/// rallenta, non sigillo). Puro e deterministico.
pub fn wall_slots(turret_pos: Vec3, threat_dir: Vec3) -> Vec<Vec3> {
    let mut dir = threat_dir;
    dir.y = 0.0;
    let dir = if dir.length_squared() > 1e-6 {
        dir.normalize()
    } else {
        Vec3::X
    };
    let side = Vec3::new(-dir.z, 0.0, dir.x);
    [-1.0, 0.0, 1.0]
        .into_iter()
        .map(|s| {
            let p = turret_pos + dir * 10.0 + side * (s * 2.0);
            snap_to_grid(p.with_y(0.0))
        })
        .collect()
}

/// 0.0.18 — muro mirato: prima torretta completa propria + slot
/// verso la minaccia che (a) stanno su `valid_ground`, (b) restano
/// raggiungibili dal builder, (c) non murano NESSUNA factory propria
/// (porte verificate sulla grid col muro aggiunto). Fallback: nessuno spot
/// (il chiamante salta il tick, mai muri a caso).
#[allow(clippy::too_many_arguments)]
pub fn find_wall_spot(
    grid: &crate::navigation::NavGrid,
    team: u8,
    buildings: &[(crate::units::Team, BuildingKind, Vec3, bool)],
    builder_pos: Vec3,
    units: &[(Vec3, f32)],
    threat_pos: Vec3,
    home: Vec3,
) -> Option<Vec3> {
    // Prima torretta completa (xz minima = deterministico).
    let turret = buildings
        .iter()
        .filter(|(t, k, _, site)| t.0 == team && *k == BuildingKind::Turret && !site)
        .map(|(_, _, p, _)| *p)
        .min_by(|a, b| a.x.total_cmp(&b.x).then_with(|| a.z.total_cmp(&b.z)))?;
    let mut dir = threat_pos - turret;
    dir.y = 0.0;
    let dir = if dir.length_squared() > 1.0 {
        dir.normalize()
    } else {
        (home - turret).normalize_or_zero()
    };
    for slot in wall_slots(turret, dir) {
        if valid_ground(grid, BuildingKind::Wall, slot, units).is_err() {
            continue;
        }
        if !approach_ok(grid, BuildingKind::Wall, slot, builder_pos) {
            continue;
        }
        // Porte factory proprie ancora libere col muro aggiunto.
        let probe = grid.cloned_with_obstacle(building_obstacle(BuildingKind::Wall, slot));
        let mut seals = false;
        for (t, k, p, _) in buildings {
            if t.0 != team {
                continue;
            }
            if factory_spawn_ok(&probe, *k, *p).is_err() {
                seals = true;
                break;
            }
        }
        if seals {
            continue;
        }
        return Some(slot);
    }
    None
}
/// Stand-off reale + path builder (bordo footprint, non centro) sulla grid
/// CON il futuro edificio. Condiviso da spirale e spot Metal: stessa garanzia
/// anti-tasche per entrambi (vedi doc di `find_build_spot`).
fn approach_ok(
    grid: &crate::navigation::NavGrid,
    kind: BuildingKind,
    point: Vec3,
    builder_pos: Vec3,
) -> bool {
    // Raggio scafo conservativo (commander 1.4) così vale per tutti.
    // Validato sulla grid CON il futuro edificio: il suo stesso ostacolo può
    // sigillare l'approach (lab grandi in basi dense). Body-aware: lo scafo
    // grande deve poter eseguire il percorso, non solo il margine scout.
    let probe = grid.cloned_with_obstacle(building_obstacle(kind, point));
    let approach = site_approach(&probe, point, builder_pos, kind.stats().half, 1.4);
    probe.find_path_for(builder_pos, approach, 1.4).is_some()
}

/// G1 — spot Metal: libero più vicino al builder (parità → mult maggiore,
/// poi xz), con le stesse garanzie della spirale (`valid_ground` + approach).
/// Occupati = Metal vivi di qualsiasi team (conteso = chiuso). Mai spirale:
/// la regola è hard, senza spot niente Build (il chiamante salta il tick).
pub fn find_metal_spot(
    grid: &crate::navigation::NavGrid,
    deposits: &[crate::structures::MetalDeposit],
    metals: &[Vec3],
    builder_pos: Vec3,
    units: &[(Vec3, f32)],
) -> Option<Vec3> {
    let mut remaining = crate::structures::free_deposits(deposits, metals);
    while let Some(dep) = crate::structures::nearest_free(&remaining, &[], builder_pos) {
        remaining.retain(|d| d.pos != dep.pos);
        if valid_ground(grid, BuildingKind::Metal, dep.pos, units).is_err() {
            continue;
        }
        if approach_ok(grid, BuildingKind::Metal, dep.pos, builder_pos) {
            return Some(dep.pos);
        }
    }
    None
}

/// Ricerca deterministica dello spot edificabile: spirale dal centro base
/// (0.0.18: per le torrette, prima spirale sull'anchor hotspot se dato).
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
    anchor: Option<Vec3>,
) -> Option<Vec3> {
    use std::f32::consts::PI;
    let base = scenario.center(team as usize);
    // 0.0.18 — torrette: prima spirale sull'anchor (hotspot minaccia),
    // poi sulla base come fallback. Altri edifici sempre dalla base.
    let mut centers = vec![base];
    if kind == BuildingKind::Turret
        && let Some(a) = anchor
        && a.distance_squared(base) > 1.0
    {
        centers.insert(0, a);
    }
    // Raggi crescenti deterministici dalla base: prima vicino (difendibile),
    // poi espansione. Angoli fissi 16 per anello = ordine stabile.
    for base in centers {
        for radius in [10.0, 14.0, 18.0, 24.0, 32.0, 42.0, 56.0] {
            for step in 0..16 {
                let angle = step as f32 / 16.0 * 2.0 * PI;
                let ideal = base + Vec3::new(angle.cos() * radius, 0.0, angle.sin() * radius);
                let point = snap_to_grid(ideal.with_y(0.0));
                if valid_ground(grid, kind, point, units).is_err() {
                    continue;
                }
                // Impronte esistenti (incluse quelle aperte in questo stesso
                // tick, fuse via `probe` dall'executor): mai due edifici
                // sovrapposti, mai due cantieri sullo stesso spot.
                let clash = buildings.iter().any(|(_, bk, bp, _)| {
                    let bh = bk.stats().half;
                    let kh = kind.stats().half;
                    (bp.x - point.x).abs() < bh.x + kh.x && (bp.z - point.z).abs() < bh.y + kh.y
                });
                if clash {
                    continue;
                }
                if factory_spawn_ok(grid, kind, point).is_err() {
                    continue; // porte murate: la spirale cerca un punto libero
                }
                if placement_rule(crate::units::Team(team), point, buildings, builders).is_err() {
                    continue; // nessun builder vivo: inutile cercare oltre
                }
                // Stand-off reale del builder (bordo footprint, non centro):
                // raggio scafo conservativo (commander 1.4) così vale per tutti.
                if !approach_ok(grid, kind, point, builder_pos) {
                    continue;
                }
                return Some(point);
            }
        }
    }
    None
}
