//! Team impersonato dal player + interruttore fog spettatore.
//!
//! Storicamente selezione, ordini, industria e rendering fog erano fissati su
//! `PLAYER_TEAM` (blu). `ViewState` li rende commutabili: il pulsante VIEW
//! sposta il controllo su un altro team (i builder di quel team costruiscono
//! dal pannello), FOG OFF mostra tutto come spettatore. La simulazione
//! (targeting, economia, placement rules) resta per-team e invariata; le AI
//! leggono sempre `VisibilityMap` del proprio team, mai questa risorsa.

use bevy::prelude::*;

/// Team giocato dal player (default blu) + fog spettatore.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewState {
    pub team: u8,
    /// true = nebbia attiva per il team visto; false = tutto visibile.
    pub fog_on: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            team: 0,
            fog_on: true,
        }
    }
}

impl ViewState {
    pub fn team_name(team: u8) -> &'static str {
        if team == 0 { "BLUE" } else { "RED" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_views_blue_with_fog() {
        let view = ViewState::default();
        assert_eq!(view.team, 0);
        assert!(view.fog_on);
        assert_eq!(ViewState::team_name(0), "BLUE");
        assert_eq!(ViewState::team_name(1), "RED");
    }
}
