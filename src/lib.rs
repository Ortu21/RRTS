//! RRTS — libreria del prototipo RTS.
//!
//! Questo crate espone ogni dominio di gioco come modulo pubblico così che
//! test di integrazione, benchmark esterni e tool AI possano importare
//! `rust_rts::navigation::NavGrid` senza passare dal binario.
//!
//! Il binario (`src/main.rs`) resta solo composizione: parsing CLI + `App`.
//!
//! Mappa rapida (dettagli in `ARCHITECTURE.md`):
//! - `scenario`, `session` — setup partita e chi comanda cosa (player/bot).
//! - `world`, `camera`, `picking`, `selection`, `orders`, `movement` — input → ordini → locomozione.
//! - `navigation`, `spatial`, `formation` — pathfinding Theta*, hash spaziale, slot.
//! - `combat`, `economy`, `structures`, `production`, `fog`, `game_over` — simulazione.
//! - `units` — spawning + dati visivi (niente logica ordini qui).
//! - `ai` — snapshot onesto (fog) → strategia 1Hz → micro 4Hz, stessi attuatori del player.
//! - `benchmark`, `replay` — misura/diagnosi, mai logica gameplay.
//! - `ui` — solo presentazione (`shell` layout, `industry` controlli, `debug` overlay).

pub mod ai;
pub mod benchmark;
pub mod camera;
pub mod combat;
pub mod economy;
pub mod fog;
pub mod formation;
pub mod game_over;
pub mod movement;
pub mod navigation;
pub mod orders;
pub mod picking;
pub mod production;
pub mod replay;
pub mod scenario;
pub mod selection;
pub mod session;
pub mod spatial;
pub mod structures;
pub mod ui;
pub mod units;
pub mod world;

/// Alias storico: `view` → `session`.
///
/// `view.rs` è stato rinominato `session.rs` perché il modulo non descrive la
/// telecamera ma *chi comanda quale team* (`SessionControl`) + *cosa può vedere*
/// (`ViewState`). Tenuto per compatibilità con branch/tool esterni.
pub mod view {
    pub use super::session::*;
}
