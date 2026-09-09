//! Replay deterministico (MVP): registra checksum per tick.
//!
//! Il benchmark già produce un checksum di stato deterministico. Questo modulo
//! lo rende riusabile fuori dal benchmark: ogni sistema può spingere un hash
//! (`push_tick`) e il runner headless lo scarica a fine run per confronti
//! storici. Nessuna logica gameplay qui, solo telemetria.
//!
//! Uso futuro: salvare ordini input per tick → re-simula bit-identica.
//! Oggi: solo contenitore `TickChecksums` + plugin che lo inizializza.

use bevy::prelude::*;

/// Checksums FNV-1a per tick, in ordine di tick. Puro dato, nessun sistema lo scrive da solo.
#[derive(Resource, Default, Debug, Clone)]
pub struct TickChecksums {
    /// `(tick, checksum)` in ordine crescente di tick.
    pub entries: Vec<(u64, u64)>,
}

impl TickChecksums {
    /// Aggiunge un checksum. Ignora tick duplicati o fuori ordine per non
    /// avvelenare i confronti storici con riavvii (`R`) a metà partita.
    pub fn push_tick(&mut self, tick: u64, checksum: u64) {
        if self.entries.last().is_some_and(|(last, _)| tick <= *last) {
            return;
        }
        self.entries.push((tick, checksum));
    }

    /// Hash combinato di tutti i tick (FNV-1a a catena). `0` se vuoto.
    pub fn combined(&self) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for (tick, checksum) in &self.entries {
            hash ^= tick.wrapping_add(checksum.rotate_left(1));
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// FNV-1a a 64 bit su bytes arbitrari. Stessa funzione usata dai futuri recorder.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub struct ReplayPlugin;

impl Plugin for ReplayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TickChecksums>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combined_is_order_sensitive_and_ignores_rewinds() {
        let mut replay = TickChecksums::default();
        replay.push_tick(1, 0xAAAA);
        replay.push_tick(2, 0xBBBB);
        let first = replay.combined();
        assert_eq!(replay.len(), 2);
        // Rewind/duplicate mai registrati: il combined non cambia.
        replay.push_tick(2, 0xCCCC);
        replay.push_tick(0, 0xDDDD);
        assert_eq!(replay.len(), 2);
        assert_eq!(replay.combined(), first);
        // Ordine diverso = hash diverso.
        let mut swapped = TickChecksums::default();
        swapped.push_tick(2, 0xBBBB);
        swapped.push_tick(3, 0xAAAA);
        assert_ne!(swapped.combined(), first);
    }

    #[test]
    fn fnv1a_is_stable() {
        assert_eq!(fnv1a_64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a_64(b"a"), fnv1a_64(b"a"));
        assert_ne!(fnv1a_64(b"a"), fnv1a_64(b"b"));
    }
}
