//! Scouting a frontiera (0.0.19): novelty − minaccia×peso, max 2 scout.
//!
//! Welsh Pro2 Ch27 (frontiera + ultimo contatto + pattern a ventaglio,
//! uno-due scout ben diretti) + Zielinski Pro1 Ch33 (query dichiarative:
//! filtro esplorato/walkable/threat + score novelty/distanza/minaccia).
//! Legge SOLO [`AiSnapshot`](super::snapshot::AiSnapshot) + threat map
//! onesta: mai query nemiche dirette, mai scritture su `VisibilityMap`.
//! Tutto puro e deterministico (row-major, `total_cmp`, niente HashMap/rand).

use crate::scenario::Scenario;
use bevy::prelude::*;

use super::{
    snapshot::{AiSnapshot, EXPLORED_GRID_N},
    strategy::Personality,
    threat::ThreatMap,
};

/// Scout vivi+accodati massimi per team (0.0.19: uno-due ben diretti battono
/// molti random — Welsh). La produzione in `strategy::decide` si ferma qui.
pub const MAX_SCOUTS: usize = 2;
/// Scala per normalizzare la minaccia grezza (hp×dps, migliaia) a unità di
/// novelty 0..1: `score = novelty − (threat/SCALA)×peso`. Con pesi 0.5..1.2
/// un tank (~2000) vale ~1-2.4 novelty: hotspot evitati, celle libere prese.
pub const SCOUT_THREAT_SCALE: f32 = 1000.0;
/// Soglia threat oltre la quale il waypoint devia (executor): sopra un singolo
/// scout (~500) e sotto un tank+torretta, così i corridoi caldi si aggirano
/// ma le scaramucce non bloccano l'esplorazione.
pub const SCOUT_THREAT_THRESHOLD: f32 = 800.0;
/// Deviazione laterale del waypoint in metri (una cella nav da 2.5m ×8:
/// fuori dalla threat ma ancora in rotta).
pub const SCOUT_DETOUR_M: f32 = 20.0;
/// Peso diversificazione tra i 2 scout (ventaglio Welsh): penalità di score
/// che decade con la distanza dalle mete già assegnate E dai corridoi in
/// corso (segmenti pos→dest degli scout in `Move`, 0 oltre il range).
/// Senza, i due scout marciano in pila sullo stesso corridoio (stesso score,
/// celle adiacenti o stesso asse base→nemico) e la copertura non raddoppia.
pub const SCOUT_SPREAD_WEIGHT: f32 = 0.6;
pub const SCOUT_SPREAD_RANGE: f32 = 150.0;

/// Score frontiera: novelty (1 = mai visto, 0 = esplorato) meno minaccia
/// normalizzata pesata per personalità. Puro.
pub fn frontier_score(novelty: f32, threat: f32, weight: f32) -> f32 {
    novelty - (threat / SCOUT_THREAT_SCALE) * weight
}

/// Centro mondo della cella frontiera (col,row). Stessa geometria di
/// `snapshot::sample_explored_cells` e `threat::ThreatMap`: le tre restano
/// allineate per costruzione (test sotto). Puro.
pub fn frontier_cell_center(col: usize, row: usize) -> Vec3 {
    use crate::navigation::HALF_SIZE;
    let n = EXPLORED_GRID_N;
    let side = (HALF_SIZE * 2.0) / n as f32;
    Vec3::new(
        -HALF_SIZE + (col.min(n - 1) as f32 + 0.5) * side,
        0.0,
        -HALF_SIZE + (row.min(n - 1) as f32 + 0.5) * side,
    )
}

/// Meta scout: conferma del contatto perso + celle frontiera.
/// `frontier_targets(..., 1)` (compat: la matematica vive qui).
/// Pura; vedi `frontier_targets` per la dottrina completa.
pub fn frontier_target(
    snapshot: &AiSnapshot,
    scenario: Scenario,
    threat: &ThreatMap,
    personality: &Personality,
) -> Vec3 {
    frontier_targets(snapshot, scenario, threat, personality, 1)
        .into_iter()
        .next()
        .unwrap_or_else(|| scenario.attack_target(snapshot.team as usize))
}

/// Top-`n` mete scout distinte (Welsh: uno-due ben diretti su assi diversi,
/// mai in pila sulla stessa meta). Dottrina, in ordine:
/// 1. Conferma: a vista live vuota, il primo scout torna sul contatto perso
///    FRESCO (età ≤ fresh, ≤30s) — riacquisire una traccia calda guida
///    l'ondata (`attack_destination` insegue lo stesso baricentro). Contatti
///    stantii non si inseguono in linea retta (si finisce nei cannoni a
///    occhi chiusi): li ritrova la frontiera. Saltata se uno scout è già in
///    rotta lì (≤20m). A nemico visibile niente conferme suicide: gli scout
///    esplorano altrove.
/// 2. Frontiera: migliori celle `novelty − minaccia_norm×peso` (spec 0.0.19),
///    pareggi → più vicina alla base nemica (first-blood), poi indice minore.
///    Celle esplorate saltate finché resta una vergine; a mappa piena si
///    pattuglia il fronte (novelty 0 ovunque, vince minaccia+distanza).
/// 3. Ventaglio: le mete si spreadano sia tra loro che dalle rotte in corso
///    (scout già in `Move`: letti dallo snapshot, ordinati per determinismo),
///    così il secondo scout non rifà il corridoio del primo uscito prima.
///
/// Pura e deterministica.
pub fn frontier_targets(
    snapshot: &AiSnapshot,
    scenario: Scenario,
    threat: &ThreatMap,
    personality: &Personality,
    n: usize,
) -> Vec<Vec3> {
    let mut targets = Vec::new();
    // Rotte in corso (segmenti pos→dest degli scout in `Move`): le nuove mete
    // ci si spreadano (il secondo scout esce dopo il primo: senza, rifarebbe
    // lo stesso corridoio base→nemico). Ordinati per determinismo.
    let mut inflight: Vec<(Vec3, Vec3)> = snapshot
        .my_units
        .iter()
        .filter(|u| u.kind == crate::units::UnitKind::Scout)
        .filter_map(|u| match u.order {
            crate::orders::UnitOrder::Move { destination } => Some((u.pos, destination)),
            _ => None,
        })
        .collect();
    inflight.sort_by(|a, b| {
        a.1.x
            .to_bits()
            .cmp(&b.1.x.to_bits())
            .then_with(|| a.1.z.to_bits().cmp(&b.1.z.to_bits()))
            .then_with(|| a.0.x.to_bits().cmp(&b.0.x.to_bits()))
            .then_with(|| a.0.z.to_bits().cmp(&b.0.z.to_bits()))
    });
    // Conferma solo a vista live vuota (mai dentro una battaglia in corso),
    // solo tracce fresche (niente inseguimenti di fantasmi stantii nei
    // cannoni) e solo se nessuno ci sta già andando (rotta ≤20m).
    if snapshot.visible_enemies.is_empty()
        && let Some(lost) = snapshot
            .memory
            .iter()
            .find(|m| !m.building && m.age_ticks <= super::memory::MEMORY_FRESH_TICKS)
        && inflight.iter().all(|(_, d)| d.distance(lost.pos) > 20.0)
    {
        targets.push(lost.pos);
    }
    let foe_base = scenario.attack_target(snapshot.team as usize);
    let n_grid = EXPLORED_GRID_N;
    let mut any_unseen = false;
    for row in 0..n_grid {
        for col in 0..n_grid {
            if !snapshot.explored_cell(col, row) {
                any_unseen = true;
                break;
            }
        }
        if any_unseen {
            break;
        }
    }
    let mut picked: Vec<usize> = Vec::new();
    // Spread dai corridoi in corso + dalla conferma: il ventaglio copre
    // entrambi (i punti assegnati contano come segmenti degeneri).
    let mut lanes: Vec<(Vec3, Vec3)> = inflight;
    if let Some(first) = targets.first() {
        lanes.push((*first, *first));
    }
    while targets.len() < n.max(1) {
        let mut best: Option<(f32, f32, usize, Vec3)> = None;
        for row in 0..n_grid {
            for col in 0..n_grid {
                let idx = row * n_grid + col;
                if picked.contains(&idx) {
                    continue;
                }
                let explored = snapshot.explored_cell(col, row);
                if any_unseen && explored {
                    continue;
                }
                let center = frontier_cell_center(col, row);
                let novelty = if explored { 0.0 } else { 1.0 };
                let score = frontier_score(
                    novelty,
                    threat.query(center),
                    personality.scout_threat_weight,
                );
                // Ventaglio: penalità per vicinanza ai corridoi coperti
                // (rotte in corso + mete assegnate, 0 oltre il range).
                // Il primo scout non è penalizzato.
                let mut spread = 0.0f32;
                for (a, b) in &lanes {
                    let d = seg_dist(center, *a, *b);
                    if d < SCOUT_SPREAD_RANGE {
                        spread = spread.max(1.0 - d / SCOUT_SPREAD_RANGE);
                    }
                }
                let score = score - spread * SCOUT_SPREAD_WEIGHT;
                let dist = center.distance_squared(foe_base);
                // Score maggiore vince; a pari score distanza minore vince;
                // a pari distanza indice minore (deterministico).
                let wins = match &best {
                    None => true,
                    Some((bs, bd, bi, _)) => {
                        use std::cmp::Ordering as O;
                        match score.total_cmp(bs) {
                            O::Greater => true,
                            O::Less => false,
                            O::Equal => match dist.total_cmp(bd) {
                                O::Less => true,
                                O::Greater => false,
                                O::Equal => idx < *bi,
                            },
                        }
                    }
                };
                if wins {
                    best = Some((score, dist, idx, center));
                }
            }
        }
        match best {
            Some((_, _, idx, pos)) => {
                picked.push(idx);
                lanes.push((pos, pos));
                // Mai due mete sulla conferma: la cella della conferma resta
                // valida come frontiera solo se è la migliore libera.
                targets.push(pos);
            }
            None => break,
        }
    }
    targets
}

/// Distanza punto-segmento su xz (corridoi scout). Segmento degenere (a==b)
/// = distanza punto-punto. Pura.
fn seg_dist(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let ab = Vec3::new(ab.x, 0.0, ab.z);
    let ap = Vec3::new(p.x - a.x, 0.0, p.z - a.z);
    let len_sq = ab.length_squared();
    if len_sq < 1e-6 {
        return ap.length();
    }
    let t = (ap.dot(ab) / len_sq).clamp(0.0, 1.0);
    (ap - ab * t).length()
}

/// Waypoint threat-aware (Welsh routing): se il punto medio del segmento
/// supera la soglia, devia perpendicolare di `SCOUT_DETOUR_M` dal lato meno
/// minaccioso (pareggi → +lato, deterministico). Altrimenti dritto alla meta.
/// Puro; la riparazione walkable (`clear_point_for`) resta all'executor che
/// ha la `NavGrid`.
pub fn scout_waypoint(from: Vec3, to: Vec3, threat: &ThreatMap, threshold: f32) -> Vec3 {
    let mut mid = (from + to) * 0.5;
    mid.y = 0.0;
    if threat.query(mid) <= threshold {
        return to;
    }
    let mut dir = to - from;
    dir.y = 0.0;
    let dir = if dir.length_squared() > 1e-6 {
        dir.normalize()
    } else {
        Vec3::X
    };
    let side = Vec3::new(-dir.z, 0.0, dir.x);
    let a = (mid + side * SCOUT_DETOUR_M).with_y(0.0);
    let b = (mid - side * SCOUT_DETOUR_M).with_y(0.0);
    if threat.query(a) <= threat.query(b) {
        a
    } else {
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{orders::UnitOrder, units::UnitKind};

    fn empty_snapshot(team: u8) -> AiSnapshot {
        AiSnapshot {
            team,
            ..Default::default()
        }
    }

    fn explored_snapshot(team: u8, explored: &[(usize, usize)]) -> AiSnapshot {
        let mut snap = empty_snapshot(team);
        snap.explored_cells = vec![false; EXPLORED_GRID_N * EXPLORED_GRID_N];
        for (c, r) in explored {
            snap.explored_cells[r * EXPLORED_GRID_N + c] = true;
        }
        snap
    }

    fn threat_with_tank_at(pos: Vec3) -> (AiSnapshot, ThreatMap) {
        // Threat costruita onestamente dallo snapshot (stesso percorso del gioco).
        let mut snap = empty_snapshot(1);
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos,
            kind: UnitKind::HeavyTank,
            health: crate::units::archetype(UnitKind::HeavyTank).max_health,
        });
        let map = super::super::threat::build_threat(&snap);
        (snap, map)
    }

    #[test]
    fn frontier_score_math() {
        // Novelty pura senza minaccia; la minaccia pesa per peso.
        assert!((frontier_score(1.0, 0.0, 1.0) - 1.0).abs() < 1e-6);
        assert!((frontier_score(0.0, 0.0, 1.0)).abs() < 1e-6);
        let hot = frontier_score(1.0, SCOUT_THREAT_SCALE, 1.0);
        assert!((hot - 0.0).abs() < 1e-6);
        // Peso maggiore = penalità maggiore a pari minaccia.
        assert!(frontier_score(1.0, 500.0, 1.2) < frontier_score(1.0, 500.0, 0.5));
    }

    #[test]
    fn frontier_prefers_unseen() {
        use crate::scenario::Scenario;
        // Tutte esplorate tranne una: la frontiera deve puntarla anche se
        // lontana dalla base nemica (novelty batte distanza).
        let mut snap = explored_snapshot(1, &[]);
        // Marca tutte tranne l'angolo (0,0) come viste.
        for cell in snap.explored_cells.iter_mut() {
            *cell = true;
        }
        snap.explored_cells[0] = false;
        let threat = super::super::threat::build_threat(&snap);
        let dest = frontier_target(&snap, Scenario::Playground, &threat, &Personality::RUSHER);
        let want = frontier_cell_center(0, 0);
        assert!(
            (dest.x - want.x).abs() < 0.01 && (dest.z - want.z).abs() < 0.01,
            "frontiera dovrebbe puntare l'unica cella vergine, got {dest:?} want {want:?}"
        );
    }

    #[test]
    fn scout_avoids_hotspot() {
        use crate::scenario::Scenario;
        // Due celle vergini (tutte vergini in realtà): una con un tank sopra,
        // l'altra libera. La frontiera deve evitare l'hotspot.
        let hot_center = frontier_cell_center(20, 20);
        let (_seen, threat) = threat_with_tank_at(hot_center);
        // Snapshot cieco (tutto inesplorato) ma threat calda su (20,20).
        let snap = empty_snapshot(1);
        assert!(threat.query(hot_center) > SCOUT_THREAT_THRESHOLD);
        let dest = frontier_target(&snap, Scenario::Playground, &threat, &Personality::TURTLE);
        // Non deve cadere sulla cella calda (distanza > mezza cella).
        assert!(
            dest.distance(hot_center) > 9.0,
            "scout dovrebbe evitare l'hotspot, got {dest:?} hot {hot_center:?}"
        );
        // E lo score della meta deve battere quello dell'hotspot.
        let dest_score = frontier_score(
            1.0,
            threat.query(dest),
            Personality::TURTLE.scout_threat_weight,
        );
        let hot_score = frontier_score(
            1.0,
            threat.query(hot_center),
            Personality::TURTLE.scout_threat_weight,
        );
        assert!(dest_score > hot_score);
    }

    #[test]
    fn waypoint_deviates_around_threat_but_not_in_open() {
        let hot = frontier_cell_center(16, 16);
        let (_seen, threat) = threat_with_tank_at(hot);
        let from = hot + Vec3::new(-40.0, 0.0, 0.0);
        let to = hot + Vec3::new(40.0, 0.0, 0.0);
        // Il medio è caldo: devia laterale di SCOUT_DETOUR_M.
        let wp = scout_waypoint(from, to, &threat, SCOUT_THREAT_THRESHOLD);
        let mid = (from + to) * 0.5;
        assert!(
            ((wp - mid).length() - SCOUT_DETOUR_M).abs() < 0.01,
            "deviazione laterale di {SCOUT_DETOUR_M}m dal medio: {wp:?} mid {mid:?}"
        );
        assert!(threat.query(wp) < threat.query(mid));
        // In campo aperto: dritto alla meta.
        let empty_snap = empty_snapshot(1);
        let calm = super::super::threat::build_threat(&empty_snap);
        let direct = scout_waypoint(from, to, &calm, SCOUT_THREAT_THRESHOLD);
        assert_eq!(direct, to);
    }

    #[test]
    fn confirm_goes_to_fresh_memory_first() {
        use crate::scenario::Scenario;
        // Contatto perso (vista live vuota) > frontiera: conferma come 0.0.14.
        let mut snap = empty_snapshot(1);
        snap.memory.push(super::super::snapshot::AiMemory {
            entity_bits: None,
            pos: Vec3::new(123.0, 0.0, 45.0),
            age_ticks: 10,
            kind: Some(UnitKind::HeavyTank),
            hp: 100.0,
            building: false,
        });
        let threat = super::super::threat::build_threat(&snap);
        let dest = frontier_target(&snap, Scenario::Playground, &threat, &Personality::RUSHER);
        assert!((dest.x - 123.0).abs() < 0.001 && (dest.z - 45.0).abs() < 0.001);
        let _ = UnitOrder::Idle;
    }

    #[test]
    fn no_suicide_confirm_into_live_battle() {
        use crate::scenario::Scenario;
        // Nemico visibile ora: niente conferma sul live, solo frontiera
        // (gli scout non si immolano nella battaglia in corso).
        let hot_center = frontier_cell_center(20, 20);
        let (seen, threat) = threat_with_tank_at(hot_center);
        let mut snap = seen;
        snap.memory.push(super::super::snapshot::AiMemory {
            entity_bits: None,
            pos: hot_center,
            age_ticks: 5,
            kind: Some(UnitKind::HeavyTank),
            hp: 100.0,
            building: false,
        });
        assert!(!snap.visible_enemies.is_empty());
        let dest = frontier_target(&snap, Scenario::Playground, &threat, &Personality::TURTLE);
        assert!(
            dest.distance(hot_center) > 9.0,
            "con live visibile, frontiera altrove: {dest:?}"
        );
    }

    #[test]
    fn second_scout_spreads_from_inflight_route() {
        use crate::scenario::Scenario;
        // Il primo scout è in rotta sul miglior corridoio: il secondo (n=1
        // libero) deve aprire un altro asse, non accodarsi.
        let first_dest = frontier_cell_center(31, 31);
        let mut snap = empty_snapshot(1);
        snap.my_units.push(super::super::snapshot::AiUnit {
            entity: Entity::from_bits(41),
            pos: Vec3::ZERO,
            kind: UnitKind::Scout,
            order: crate::orders::UnitOrder::Move {
                destination: first_dest,
            },
            health: 60.0,
            max_health: 60.0,
        });
        let threat = super::super::threat::build_threat(&snap);
        let targets = frontier_targets(
            &snap,
            Scenario::Playground,
            &threat,
            &Personality::RUSHER,
            1,
        );
        assert_eq!(targets.len(), 1);
        assert!(
            targets[0].distance(first_dest) > SCOUT_SPREAD_RANGE,
            "secondo scout lontano dalla rotta in corso: {:?} vs {first_dest:?}",
            targets
        );
    }

    #[test]
    fn two_scouts_get_distinct_frontiers() {
        use crate::scenario::Scenario;
        // Max 2 scout su assi diversi: mai in pila sulla stessa meta.
        let snap = empty_snapshot(1);
        let threat = super::super::threat::build_threat(&snap);
        let targets = frontier_targets(
            &snap,
            Scenario::Playground,
            &threat,
            &Personality::RUSHER,
            2,
        );
        assert_eq!(targets.len(), 2);
        assert!(
            targets[0].distance(targets[1]) >= SCOUT_SPREAD_RANGE - 1.0,
            "ventaglio su corridoi separati: {:?}",
            targets
        );
        // Determinismo: stesso input, stesse mete.
        let again = frontier_targets(
            &snap,
            Scenario::Playground,
            &threat,
            &Personality::RUSHER,
            2,
        );
        assert_eq!(targets, again);
        // Con conferma: primo conferma, secondo frontiera libera.
        let mut snap = empty_snapshot(1);
        snap.memory.push(super::super::snapshot::AiMemory {
            entity_bits: None,
            pos: Vec3::new(123.0, 0.0, 45.0),
            age_ticks: 10,
            kind: Some(UnitKind::HeavyTank),
            hp: 100.0,
            building: false,
        });
        let threat = super::super::threat::build_threat(&snap);
        let targets = frontier_targets(
            &snap,
            Scenario::Playground,
            &threat,
            &Personality::RUSHER,
            2,
        );
        assert_eq!(targets.len(), 2);
        assert!((targets[0].x - 123.0).abs() < 0.001);
        assert!(targets[1].distance(targets[0]) > 9.0);
    }

    #[test]
    fn frontier_geometry_matches_snapshot_and_threat() {
        // Le tre geometrie (snapshot sample, threat index, frontier center)
        // devono concordare: il centro campionato cade nella stessa cella.
        use crate::navigation::HALF_SIZE;
        assert_eq!(EXPLORED_GRID_N, super::super::threat::THREAT_GRID_N);
        let side = (HALF_SIZE * 2.0) / EXPLORED_GRID_N as f32;
        assert!((side - 18.75).abs() < 1e-6);
        let c = frontier_cell_center(0, 0);
        assert!((c.x - (-HALF_SIZE + 0.5 * side)).abs() < 1e-4);
        assert!(c.x.is_finite() && c.z.is_finite());
    }
}
