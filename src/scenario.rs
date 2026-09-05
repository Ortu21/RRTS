use bevy::prelude::*;

#[derive(Resource, Clone, Copy, Default)]
pub enum Scenario {
    #[default]
    Playground,
    Benchmark {
        per_team: usize,
    },
}

impl Scenario {
    pub fn per_team(self) -> usize {
        match self {
            Self::Playground => 100,
            Self::Benchmark { per_team } => per_team,
        }
    }

    pub fn teams(self) -> usize {
        match self {
            Self::Playground => 1,
            Self::Benchmark { .. } => 2,
        }
    }

    pub fn center(self, team: usize) -> Vec3 {
        match self {
            Self::Playground => Vec3::new(-35.0, 0.0, 0.0),
            Self::Benchmark { .. } => Vec3::new(if team == 0 { -55.0 } else { 55.0 }, 0.0, 0.0),
        }
    }
}
