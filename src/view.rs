//! Presentation perspective is independent from who may command each team.
use bevy::prelude::*;

#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewState {
    pub team: u8,
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
    pub fn permits_private_data(&self, team: u8) -> bool {
        !self.fog_on || self.team == team
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Controller {
    Human,
    Bot,
    Inactive,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalRole {
    Player(u8),
    Spectator,
}
#[derive(Resource, Clone, Debug)]
pub struct SessionControl {
    pub role: LocalRole,
    pub teams: [Controller; 2],
    /// Incremented on every handoff; UI consumers clear transient gestures.
    pub revision: u64,
}
impl Default for SessionControl {
    fn default() -> Self {
        Self {
            role: LocalRole::Player(0),
            teams: [Controller::Human, Controller::Inactive],
            revision: 0,
        }
    }
}
impl SessionControl {
    pub fn from_cli(ai: &str, ai_team: u8) -> Self {
        if ai == "off" {
            return Self::default();
        }
        if ai == "both" || ai_team == 2 {
            return Self {
                role: LocalRole::Spectator,
                teams: [Controller::Bot; 2],
                revision: 0,
            };
        }
        let bot = ai_team.min(1);
        let mut value = Self {
            role: LocalRole::Player(1 - bot),
            ..Self::default()
        };
        value.teams[bot as usize] = Controller::Bot;
        value.teams[(1 - bot) as usize] = Controller::Human;
        value
    }
    pub fn player_team(&self) -> Option<u8> {
        match self.role {
            LocalRole::Player(t) => Some(t),
            LocalRole::Spectator => None,
        }
    }
    pub fn can_command(&self, team: u8) -> bool {
        self.player_team() == Some(team)
            && self.teams.get(team as usize) == Some(&Controller::Human)
    }
    /// Single ownership transition, also usable by a future disconnect handler.
    /// Existing unit orders and production are deliberately untouched.
    pub fn transfer(&mut self, team: u8, controller: Controller) {
        if team > 1 {
            return;
        }
        if controller == Controller::Human {
            if let Some(previous) = self.player_team().filter(|t| *t != team) {
                self.teams[previous as usize] = Controller::Bot;
            }
            self.role = LocalRole::Player(team);
        } else if self.player_team() == Some(team) {
            self.role = LocalRole::Spectator;
        }
        self.teams[team as usize] = controller;
        self.revision += 1;
    }
}

pub fn bot_controls(control: Option<&SessionControl>, team: u8) -> bool {
    control.is_none_or(|c| c.teams.get(team as usize) == Some(&Controller::Bot))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cli_roles_and_transfer_are_exclusive() {
        assert_eq!(
            SessionControl::from_cli("skirmish", 0).player_team(),
            Some(1)
        );
        assert_eq!(
            SessionControl::from_cli("skirmish", 1).player_team(),
            Some(0)
        );
        assert_eq!(
            SessionControl::from_cli("off", 2).teams[1],
            Controller::Inactive
        );
        for (ai, team) in [("both", 1), ("skirmish", 2)] {
            let mut c = SessionControl::from_cli(ai, team);
            assert_eq!(c.player_team(), None);
            c.transfer(1, Controller::Human);
            assert!(!bot_controls(Some(&c), 1));
            assert!(c.can_command(1));
            c.transfer(0, Controller::Human);
            assert_eq!(c.teams, [Controller::Human, Controller::Bot]);
            c.transfer(0, Controller::Bot);
            assert_eq!(c.role, LocalRole::Spectator);
            assert!(!c.can_command(0));
        }
    }
    #[test]
    fn perspective_is_not_authority() {
        let c = SessionControl::default();
        let v = ViewState {
            team: 1,
            fog_on: false,
        };
        assert!(v.permits_private_data(0));
        assert!(!c.can_command(1));
        assert!(!ViewState::default().permits_private_data(1));
    }
}
