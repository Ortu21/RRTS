//! Metal deposits (G1): terrain resource nodes. Metal goes ONLY on spots.
//!
//! Layout deciso: 3 home per lato (coppie speculari) + 1 coppia mid + 1 spot
//! centrale con moltiplicatore (totale 9). La simmetria perfetta richiede
//! conteggi pari per le coppie + centro auto-speculare: "3 mid" non è
//! realizzabile sotto riflessione, quindi mid = 1 coppia (estendibile a 2).
//! I depositi sono terreno pubblico (sempre visibili, mai fog-gated); le
//! costruzioni sopra restano coperte dal fog normale.
//!
//! Generazione pura e deterministica da `(grid, spawn_a, spawn_b, mirror)`:
//! niente seed da plumbare (stessi ostacoli → stessi spot, anche in league
//! con mappe seedate). Simmetria di PROCEDURA (stessi offset riparati allo
//! stesso modo), non di coordinate (le rocce sono casuali).

use crate::{
    economy::balance::{BuildingKind, CENTER_YIELD_MULT},
    navigation::NavGrid,
};
use bevy::prelude::*;

/// Un deposito: posizione (y=0) + moltiplicatore income per il Metal sopra.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MetalDeposit {
    pub pos: Vec3,
    pub mult: f32,
}

/// Tutti i depositi della mappa. Resource popolata a Startup da `grid` +
/// spawn (vedi `setup_deposits` in `super`).
#[derive(Resource, Default, Clone, Debug)]
pub struct MetalDeposits(pub Vec<MetalDeposit>);

/// Moltiplicatore income ereditato dallo spot (solo Metal). Assente = 1.0
/// (spawn diretti di test/scenari senza validazione restano compatibili).
#[derive(Component, Clone, Copy, Debug)]
pub struct MetalYield(pub f32);

/// Tolleranza di validazione attorno allo spot (AI usa la pos esatta).
pub const SPOT_SNAP: f32 = 4.0;
/// Raggio di occupazione: un Metal vivo entro questo raggio chiude lo spot.
pub const SPOT_CAPTURE: f32 = 8.0;
/// Magnete del ghost player: snappa allo spot entro questo raggio.
pub const SPOT_MAGNET: f32 = 14.0;
/// Distanza minima tra depositi dopo la riparazione (= raggio di cattura:
/// due spot più vicini sarebbero comunque entrambi usabili).
pub const SPOT_SPACING: f32 = 8.0;

/// Specchio spawn_A → spawn_B. G1: `Point` (basi SW/NE); G2 (lati E/O): `X`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mirror {
    Point,
    /// G2 (spawn E/O): specchio asse X applicato all'offset.
    /// Allow: usato solo da G2, testato qui sotto.
    #[allow(dead_code)]
    X,
}

impl Mirror {
    fn map(self, base: Vec2, off: Vec2) -> Vec2 {
        match self {
            // Rotazione 180° attorno all'origine applicata all'offset.
            Mirror::Point => base - off,
            // Specchio asse X applicato all'offset.
            Mirror::X => base + Vec2::new(-off.x, off.y),
        }
    }
}

/// Genera i 9 depositi: 3 home per lato (offset dallo spawn riparati),
/// 1 coppia mid, centro con mult. Salta anchor irriparabili e collisioni
/// (ordine deterministico: homeA, homeB, mid, centro).
pub fn generate_deposits(
    grid: &NavGrid,
    spawn_a: Vec2,
    spawn_b: Vec2,
    mirror: Mirror,
) -> Vec<MetalDeposit> {
    // G1: 3 home per lato + 1 coppia mid + centro con mult = 9.
    // NOTA simmetria: conteggi pari per le coppie + centro auto-speculare.
    // "3 mid" non è realizzabile sotto riflessione (insieme dispari senza
    // punto fisso): mid = 1 coppia, estendibile a 2 coppie (11 spot totali).
    let home_offs = [
        Vec2::new(16.0, 0.0),
        Vec2::new(10.0, 12.0),
        Vec2::new(10.0, -12.0),
    ];
    let mut anchors: Vec<(Vec2, f32)> = Vec::with_capacity(9);
    for off in home_offs {
        anchors.push((spawn_a + off, 1.0));
    }
    for off in home_offs {
        anchors.push((mirror.map(spawn_b, off), 1.0));
    }
    let mid: Vec2 = (spawn_a + spawn_b) * 0.5;
    anchors.push((mid + Vec2::new(-70.0, 70.0), 1.0));
    anchors.push((mid + Vec2::new(70.0, -70.0), 1.0));
    anchors.push((mid, CENTER_YIELD_MULT));

    let mut out: Vec<MetalDeposit> = Vec::with_capacity(anchors.len());
    for (anchor, mult) in anchors {
        let pos = repair_spot(grid, anchor);
        out.push(MetalDeposit {
            pos: Vec3::new(pos.x, 0.0, pos.y),
            mult,
        });
    }
    // Spacing: tiene il primo, scarta i colliding — MA a coppie speculari:
    // se cade un membro, cade anche il gemello. Fairness prima del conteggio
    // (rocce casuali ≠ simmetriche, la procedura sì). Indici: homeA 0-2,
    // homeB 3-5, mid 6-7, centro 8 (singolo auto-speculare).
    let mut dropped = vec![false; out.len()];
    for (i, d) in out.iter().enumerate() {
        if out[..i]
            .iter()
            .any(|o| o.pos.xz().distance(d.pos.xz()) < SPOT_SPACING)
        {
            dropped[i] = true;
        }
    }
    for (a, b) in [(0, 3), (1, 4), (2, 5), (6, 7)] {
        if dropped[a] || dropped[b] {
            dropped[a] = true;
            dropped[b] = true;
        }
    }
    out.into_iter()
        .enumerate()
        .filter(|(i, _)| !dropped[*i])
        .map(|(_, d)| d)
        .collect()
}

/// Ripara un anchor sul walkable con nudge deterministici; se nulla va,
/// tiene il punto riparato grezzo (la validazione a placement gestisce).
/// 0.0.19 — Metal-aware: il punto deve passare anche `valid_ground` per un
/// Metal (footprint 5×5m + margini), non solo clearance da punto. Prima un
/// anchor poteva riparare su un punto "walkable" dove il Metal non entra
/// (visto dal vivo: home blu nella roccia → commander in marcia 70s verso il
/// centro, mai factory/scout in 120s, `scout-blitz` inchiodato al 4%).
/// L'ordine dei candidati è invariato (anchor prima): gli spot già validi
/// restano bit-identici, si spostano solo quelli morti.
fn repair_spot(grid: &NavGrid, anchor: Vec2) -> Vec2 {
    let mut cands = vec![anchor];
    for r in [4.0, 8.0, 12.0, 16.0, 20.0, 24.0] {
        for (dx, dz) in [
            (r, 0.0),
            (-r, 0.0),
            (0.0, r),
            (0.0, -r),
            (r, r),
            (-r, -r),
            (r, -r),
            (-r, r),
        ] {
            cands.push(anchor + Vec2::new(dx, dz));
        }
    }
    let mut fallback: Option<Vec2> = None;
    for c in cands {
        let p = grid.clear_point_for(Vec3::new(c.x, 0.0, c.y), 3.0);
        if !(grid.is_walkable(p) && grid.has_clearance(p)) {
            continue;
        }
        // Primo punto walkable = fallback storico (invariato se nulla passa
        // il footprint).
        fallback.get_or_insert(p.xz());
        if super::valid_ground(grid, BuildingKind::Metal, p, &[]).is_ok() {
            return p.xz();
        }
    }
    fallback.unwrap_or_else(|| {
        grid.clear_point_for(Vec3::new(anchor.x, 0.0, anchor.y), 3.0)
            .xz()
    })
}

/// Regola di piazzamento Metal (4ª della catena, vale per player e AI):
/// solo su spot libero entro `SPOT_SNAP`. Occupati = Metal vivi entro
/// `SPOT_CAPTURE` (qualsiasi team: conteso = chiuso). Altri kind: sempre Ok.
pub fn metal_spot_ok(
    deposits: &[MetalDeposit],
    kind: BuildingKind,
    point: Vec3,
    metals: &[Vec3],
) -> Result<(), &'static str> {
    if kind != BuildingKind::Metal {
        return Ok(());
    }
    let Some(dep) = nearest_deposit(deposits, point) else {
        return Err("Metal needs a deposit — build on highlighted spots");
    };
    if point.xz().distance(dep.pos.xz()) > SPOT_SNAP {
        return Err("Metal needs a deposit — build on highlighted spots");
    }
    if !spot_free(&dep, metals) {
        return Err("Deposit occupied");
    }
    Ok(())
}

/// Deposito più vicino (distanza xz). Pareggi → mult maggiore, poi xz
/// (deterministico).
pub fn nearest_deposit(deposits: &[MetalDeposit], point: Vec3) -> Option<MetalDeposit> {
    deposits.iter().copied().min_by(|a, b| {
        let da = a.pos.xz().distance_squared(point.xz());
        let db = b.pos.xz().distance_squared(point.xz());
        da.total_cmp(&db).then_with(|| {
            b.mult
                .total_cmp(&a.mult)
                .then_with(|| a.pos.x.to_bits().cmp(&b.pos.x.to_bits()))
                .then_with(|| a.pos.z.to_bits().cmp(&b.pos.z.to_bits()))
        })
    })
}

/// Spot libero: nessun Metal entro `SPOT_CAPTURE`.
pub fn spot_free(dep: &MetalDeposit, metals: &[Vec3]) -> bool {
    metals
        .iter()
        .all(|p| p.xz().distance(dep.pos.xz()) > SPOT_CAPTURE)
}

/// Depositi liberi in ordine di generazione (deterministico).
pub fn free_deposits(deposits: &[MetalDeposit], metals: &[Vec3]) -> Vec<MetalDeposit> {
    deposits
        .iter()
        .copied()
        .filter(|d| spot_free(d, metals))
        .collect()
}

/// Quanti spot liberi (macro AI: `min(cap, liberi)`).
pub fn count_free(deposits: &[MetalDeposit], metals: &[Vec3]) -> usize {
    free_deposits(deposits, metals).len()
}

/// Libero più vicino a `from` (AI Metal). Pareggi → mult maggiore, poi xz.
pub fn nearest_free(
    deposits: &[MetalDeposit],
    metals: &[Vec3],
    from: Vec3,
) -> Option<MetalDeposit> {
    free_deposits(deposits, metals).into_iter().min_by(|a, b| {
        let da = a.pos.xz().distance_squared(from.xz());
        let db = b.pos.xz().distance_squared(from.xz());
        da.total_cmp(&db).then_with(|| {
            b.mult
                .total_cmp(&a.mult)
                .then_with(|| a.pos.x.to_bits().cmp(&b.pos.x.to_bits()))
                .then_with(|| a.pos.z.to_bits().cmp(&b.pos.z.to_bits()))
        })
    })
}

/// Eredita il mult dallo spot al sito appena spawnato (solo Metal su spot).
/// Path diretti di test/scenari senza validazione: nessun componente = 1.0.
pub fn apply_deposit_yield(
    commands: &mut Commands,
    site: Entity,
    kind: BuildingKind,
    deposits: &[MetalDeposit],
    pos: Vec3,
) {
    if kind != BuildingKind::Metal {
        return;
    }
    if let Some(dep) = nearest_deposit(deposits, pos)
        && pos.xz().distance(dep.pos.xz()) <= SPOT_SNAP
    {
        commands.entity(site).insert(MetalYield(dep.mult));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::navigation::NavGrid;

    fn spawns() -> (Vec2, Vec2) {
        (Vec2::new(-260.0, -260.0), Vec2::new(260.0, 260.0))
    }

    #[test]
    fn mirrors_map_offsets_as_documented() {
        // Point: rotazione 180° dell'offset. X: specchio asse X (G2, spawn E/O).
        assert_eq!(
            Mirror::Point.map(Vec2::new(260.0, 260.0), Vec2::new(16.0, 4.0)),
            Vec2::new(244.0, 256.0)
        );
        assert_eq!(
            Mirror::X.map(Vec2::new(260.0, 0.0), Vec2::new(16.0, 4.0)),
            Vec2::new(244.0, 4.0)
        );
    }

    #[test]
    fn layout_is_symmetric_walkable_and_deterministic() {
        let grid = NavGrid::default();
        let (a, b) = spawns();
        let d1 = generate_deposits(&grid, a, b, Mirror::Point);
        let d2 = generate_deposits(&grid, a, b, Mirror::Point);
        assert_eq!(d1, d2, "bit-identico tra repeat");
        // Fairness vera (non coordinate speculari: la riparazione deriva in
        // modo asimmetrico su rocce casuali): 3 home entro 80m per spawn,
        // conteggi pari per lato, centro con mult sempre presente.
        let home_a = d1.iter().filter(|d| d.pos.xz().distance(a) < 80.0).count();
        let home_b = d1.iter().filter(|d| d.pos.xz().distance(b) < 80.0).count();
        assert_eq!(home_a, home_b, "lati dispari: {d1:?}");
        assert!(home_a >= 2, "home insufficienti: {home_a}");
        assert!(d1.len() > 2 * home_a, "mid/centro spariti: {}", d1.len());
        let centers: Vec<_> = d1.iter().filter(|d| d.mult > 1.0).collect();
        assert_eq!(centers.len(), 1);
        assert_eq!(centers[0].mult, CENTER_YIELD_MULT);
        // Tutti walkable con clearance + spacing.
        for d in &d1 {
            let p = Vec3::new(d.pos.x, 0.0, d.pos.z);
            assert!(grid.is_walkable(p) && grid.has_clearance(p), "{d:?}");
        }
        // 0.0.19 — ogni spot deve ospitare davvero un Metal (footprint +
        // margini): home morte = commander in marcia verso il centro.
        for d in &d1 {
            let p = Vec3::new(d.pos.x, 0.0, d.pos.z);
            assert!(
                super::super::valid_ground(&grid, BuildingKind::Metal, p, &[]).is_ok(),
                "spot senza footprint Metal: {d:?}"
            );
        }
        for (i, x) in d1.iter().enumerate() {
            for y in &d1[i + 1..] {
                assert!(
                    x.pos.xz().distance(y.pos.xz()) >= SPOT_SPACING - 0.01,
                    "{x:?} vs {y:?}"
                );
            }
        }
    }

    #[test]
    fn rule_accepts_only_free_spots_for_metal() {
        let deps = vec![
            MetalDeposit {
                pos: Vec3::new(10.0, 0.0, 0.0),
                mult: 1.0,
            },
            MetalDeposit {
                pos: Vec3::new(0.0, 0.0, 0.0),
                mult: CENTER_YIELD_MULT,
            },
        ];
        let at = |x: f32| Vec3::new(x, 0.0, 0.0);
        // Altri kind sempre Ok.
        assert!(metal_spot_ok(&deps, BuildingKind::Solar, at(99.0), &[]).is_ok());
        // Lontano dagli spot: rifiuto.
        assert!(metal_spot_ok(&deps, BuildingKind::Metal, at(99.0), &[]).is_err());
        // Sullo spot libero: ok (snap tollerato).
        assert!(metal_spot_ok(&deps, BuildingKind::Metal, at(11.0), &[]).is_ok());
        // Occupato (qualsiasi team): chiuso.
        assert!(metal_spot_ok(&deps, BuildingKind::Metal, at(10.0), &[at(10.5)]).is_err());
        // Centro dà mult ma vale come gli altri per la regola.
        assert!(metal_spot_ok(&deps, BuildingKind::Metal, at(0.5), &[]).is_ok());
        assert_eq!(count_free(&deps, &[]), 2);
        assert_eq!(count_free(&deps, &[at(10.0)]), 1);
        // nearest_free preferisce il vicino; a pari distanza il mult maggiore.
        assert_eq!(
            nearest_free(&deps, &[], at(5.0)).map(|d| d.pos.x),
            Some(0.0)
        );
    }
}
