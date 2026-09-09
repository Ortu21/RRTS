//! Scenario: Playground vs Benchmark/Skirmish. Solo dati setup.
//!
//! `center`/`attack_target` = spawn e mete marching. `per_team`/`teams` = size.
//! Niente sistemi: i plugin leggono questa `Resource`.

use bevy::prelude::*;

#[derive(Resource, Clone, Copy, Default)]
pub enum Scenario {
    #[default]
    Playground,
    Benchmark {
        per_team: usize,
    },
    Skirmish {
        per_team: usize,
    },
}

impl Scenario {
    pub fn per_team(self) -> usize {
        match self {
            // Playground: solo il comandante per team (base operativa iniziale).
            Self::Playground => 1,
            Self::Benchmark { per_team } | Self::Skirmish { per_team } => per_team,
        }
    }

    pub fn teams(self) -> usize {
        match self {
            Self::Playground => 2,
            Self::Benchmark { .. } | Self::Skirmish { .. } => 2,
        }
    }

    pub fn center(self, team: usize) -> Vec3 {
        match self {
            // Playground: angoli opposti (blu SW, rosso NE, ~735m tra loro su
            // mappa 600x600): punti protetti dal generatore, con margine per
            // basi ed espansioni. Distanza = tempo di eco/scouting.
            Self::Playground if team == 0 => Vec3::new(-260.0, 0.0, -260.0),
            Self::Playground => Vec3::new(260.0, 0.0, 260.0),
            Self::Benchmark { .. } | Self::Skirmish { .. } => {
                Vec3::new(if team == 0 { -110.0 } else { 110.0 }, 0.0, 0.0)
            }
        }
    }

    /// Attack-move destination: the enemy home side. A fixed center point
    /// would sit inside the middle wall and fail pathfinding; marching at
    /// the enemy base still crosses the map center, where forces collide.
    pub fn attack_target(self, team: usize) -> Vec3 {
        self.center(1 - team)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::navigation::NavGrid;

    #[test]
    fn attack_targets_are_reachable_open_ground() {
        let grid = NavGrid::default();
        for scenario in [
            Scenario::Playground,
            Scenario::Benchmark { per_team: 100 },
            Scenario::Skirmish { per_team: 100 },
        ] {
            for team in 0..scenario.teams() {
                let start = scenario.center(team);
                let goal = scenario.attack_target(team);
                assert!(grid.has_clearance(start));
                assert!(grid.has_clearance(goal));
                assert!(grid.find_path(start, goal).is_some());
            }
        }
    }
}
