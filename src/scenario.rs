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
            Self::Playground => 100,
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
            Self::Playground => Vec3::new(if team == 0 { -35.0 } else { 35.0 }, 0.0, 0.0),
            Self::Benchmark { .. } | Self::Skirmish { .. } => {
                Vec3::new(if team == 0 { -55.0 } else { 55.0 }, 0.0, 0.0)
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
